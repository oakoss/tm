//! SQLite store: the operation log, its table projections and the read-only snapshot.
//!
//! The log is the source of truth (ADR 0005). Every write appends one operation
//! and applies it to the projection tables in the same `BEGIN IMMEDIATE`
//! transaction; `rebuild` regenerates the projections from the log.

mod apply;
mod query;
mod read;
mod schema;
mod validate;

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::path::Path;
use std::time::Duration;

use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior, params,
};
use tm_core::filter::{Context, FilterError, parse, resolve};
use tm_core::hlc::MAX_MS;
use tm_core::op::CURRENT_VERSION;
use tm_core::{Change, Hlc, Op, OpError, ProjectId, Scheme};
use uuid::Uuid;

use apply::{apply, project_from, rust_int, sql_int, uuid_from};

pub use read::IssueView;

/// How long a writer waits for another writer before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The widest UTC offset jiff can represent, ±25:59.
const MAX_OFFSET_MIN: u16 = 25 * 60 + 59;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(
        "the store is busy: another process holds the write lock, possibly for a rebuild; retry"
    )]
    Busy(#[source] rusqlite::Error),
    #[error("Issue {0} does not exist")]
    UnknownIssue(Uuid),
    #[error("Issue {0} already exists")]
    DuplicateIssue(Uuid),
    #[error("unknown field {field:?}; valid fields: {}", valid.join(", "))]
    UnknownField { field: String, valid: Vec<String> },
    #[error("{field} {reason}{}", if valid.is_empty() { String::new() } else { format!("; valid: {}", valid.join(", ")) })]
    Invalid {
        field: String,
        reason: String,
        /// The values that would be accepted, when they can be listed.
        valid: Vec<String>,
    },
    #[error("Issue {issue} is missing required fields: {}", fields.join(", "))]
    MissingFields { issue: Uuid, fields: Vec<String> },
    #[error("Issue {issue} belongs to Project {expected}, not {found}")]
    ProjectMismatch {
        issue: Uuid,
        expected: ProjectId,
        found: ProjectId,
    },
    #[error(transparent)]
    Op(#[from] OpError),
    #[error("the system clock cannot be used: {0}")]
    Clock(String),
    #[error(
        "the store was written by a newer tm (format {found}, this tm reads {supported}); upgrade tm"
    )]
    NewerStore { found: i64, supported: i64 },
    #[error("the operation at log position {seq} cannot be replayed: {source}")]
    Replay {
        seq: i64,
        /// `None` when the stored id itself is unreadable.
        op_id: Option<Uuid>,
        #[source]
        source: Box<StoreError>,
    },
    #[error("the store is corrupt: {0}")]
    Corrupt(String),
    #[error("SQLite error: {0}")]
    Sqlite(rusqlite::Error),
    #[error(transparent)]
    Filter(#[from] FilterError),
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        if err.sqlite_error_code() == Some(ErrorCode::DatabaseBusy) {
            Self::Busy(err)
        } else {
            Self::Sqlite(err)
        }
    }
}

/// Wall time as the writer sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallTime {
    pub utc_ms: i64,
    pub offset_min: i16,
}

pub trait Clock {
    /// # Errors
    ///
    /// A description of why the time or the local offset cannot be read.
    fn now(&self) -> Result<WallTime, String>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Result<WallTime, String> {
        let zone = jiff::tz::TimeZone::try_system().map_err(|err| err.to_string())?;
        let now = jiff::Timestamp::now();
        let offset = zone.to_offset(now).seconds() / 60;
        Ok(WallTime {
            utc_ms: now.as_millisecond(),
            offset_min: i16::try_from(offset).expect("jiff offsets stay within ±25:59"),
        })
    }
}

/// A logged operation replay skipped because its content did not fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unapplied {
    pub seq: i64,
    pub op_id: Uuid,
    pub reason: String,
}

/// A change to write, before the store stamps it with identity and time.
#[derive(Debug, Clone)]
pub struct Draft {
    pub actor: String,
    pub project: ProjectId,
    pub issue_id: Uuid,
    pub change: Change,
}

pub struct Store {
    conn: Connection,
    clock: Box<dyn Clock + Send>,
    scheme: Scheme,
}

impl Drop for Store {
    fn drop(&mut self) {
        // Best effort: a failed optimize costs query planning, never data.
        let _ = self.conn.execute_batch("PRAGMA optimize");
    }
}

impl Store {
    /// Opens or creates the store at `path`, rebuilding the projections when
    /// they are missing or from another `PROJECTION_VERSION`.
    ///
    /// # Errors
    ///
    /// [`StoreError::NewerStore`] for a store this binary cannot read,
    /// [`StoreError::Corrupt`] when its metadata is inconsistent,
    /// [`StoreError::Replay`] when the log cannot be rebuilt, or
    /// [`StoreError::Busy`].
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::open_with_clock(path, Box::new(SystemClock))
    }

    /// [`Store::open`] with an injected clock.
    ///
    /// # Errors
    ///
    /// As [`Store::open`].
    pub fn open_with_clock(path: &Path, clock: Box<dyn Clock + Send>) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // `synchronous=NORMAL` can lose the last writes on power loss but never
        // corrupts; the bundled build defaults to FULL and foreign keys on, so
        // both are set explicitly.
        // `recursive_triggers` makes `INSERT OR REPLACE` fire the log's DELETE
        // trigger; without it a replace silently rewrites a stored operation.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA recursive_triggers = ON;",
        )?;
        // People are typed by hand, so filters and sorts compare them ignoring
        // case; SQLite's NOCASE folds only A to Z. Uppercasing first makes
        // ß match SS and final sigma match sigma.
        conn.create_collation(query::FOLD, |a, b| {
            let fold = |s: &str| s.to_uppercase().to_lowercase();
            fold(a).cmp(&fold(b))
        })?;
        let mut store = Self {
            conn,
            clock,
            scheme: Scheme::built_in(),
        };
        store.init()?;
        Ok(store)
    }

    /// Replaces the Scheme writes are checked against and reads are labelled
    /// with. The projection does not depend on it, so no rebuild follows.
    #[must_use]
    pub fn with_scheme(mut self, scheme: Scheme) -> Self {
        self.scheme = scheme;
        self
    }

    #[must_use]
    pub const fn scheme(&self) -> &Scheme {
        &self.scheme
    }

    fn init(&mut self) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(schema::LOG)?;
        let format: Option<i64> = tx
            .query_row("SELECT v FROM meta WHERE k = 'format'", [], |row| {
                row.get(0)
            })
            .optional()?;
        match format {
            Some(found) if found > schema::STORE_FORMAT => {
                return Err(StoreError::NewerStore {
                    found,
                    supported: schema::STORE_FORMAT,
                });
            }
            Some(found) if found < 1 => {
                return Err(StoreError::Corrupt(format!("unknown store format {found}")));
            }
            Some(_) => {}
            None => {
                tx.execute(
                    "INSERT INTO meta (k, v) VALUES ('format', ?1)",
                    [schema::STORE_FORMAT],
                )?;
            }
        }
        let has_ops: bool =
            tx.query_row("SELECT EXISTS (SELECT 1 FROM ops)", [], |row| row.get(0))?;
        let has_replica: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM meta WHERE k = 'replica_id')",
            [],
            |row| row.get(0),
        )?;
        if !has_replica {
            if has_ops {
                return Err(StoreError::Corrupt(
                    "the Replica identity is missing from a store that has operations".into(),
                ));
            }
            tx.execute(
                "INSERT INTO meta (k, v) VALUES ('replica_id', ?1)",
                [Uuid::now_v7().as_bytes()],
            )?;
        }
        // Raised to this Replica's own maximum only: other Replicas' clocks
        // reach the local clock solely through the capped `Hlc::observe`.
        tx.execute(
            "INSERT INTO meta (k, v)
             SELECT 'last_hlc', coalesce(max(hlc), 0) FROM ops
             WHERE replica_id = (SELECT v FROM meta WHERE k = 'replica_id')
             ON CONFLICT (k) DO UPDATE SET v = max(v, excluded.v)",
            [],
        )?;
        let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        tx.commit()?;
        if version != schema::PROJECTION_VERSION {
            self.rebuild()?;
        }
        Ok(())
    }

    /// Read access for queries; writes go through [`Store::append`], and
    /// triggers refuse changes to stored operations.
    #[must_use]
    pub const fn connection(&self) -> &Connection {
        &self.conn
    }

    /// This store's Replica identity, generated once.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] if the stored id is malformed.
    pub fn replica_id(&self) -> Result<Uuid, StoreError> {
        let bytes: Vec<u8> =
            self.conn
                .query_row("SELECT v FROM meta WHERE k = 'replica_id'", [], |row| {
                    row.get(0)
                })?;
        uuid_from(&bytes)
    }

    /// [`Store::append_all`] for a single change.
    ///
    /// # Errors
    ///
    /// As [`Store::append_all`].
    pub fn append(&mut self, draft: Draft) -> Result<Op, StoreError> {
        let mut ops = self.append_all(vec![draft])?;
        Ok(ops.remove(0))
    }

    /// Creates an Issue in its Workflow's starting Status.
    ///
    /// # Errors
    ///
    /// As [`Store::append_all`].
    pub fn create_issue(
        &mut self,
        actor: &str,
        project: ProjectId,
        issue_type: &str,
        title: &str,
    ) -> Result<(Uuid, Vec<Op>), StoreError> {
        let issue_id = Uuid::now_v7();
        let start = self
            .scheme
            .workflow(issue_type)
            .map(|workflow| workflow.start.clone());
        let mut changes = vec![
            Change::Create,
            set("type", serde_json::json!(issue_type)),
            set("title", serde_json::json!(title)),
        ];
        if let Some(start) = start {
            changes.push(set("status", serde_json::json!(start)));
        }
        let drafts = changes
            .into_iter()
            .map(|change| Draft {
                actor: actor.to_owned(),
                project,
                issue_id,
                change,
            })
            .collect();
        Ok((issue_id, self.append_all(drafts)?))
    }

    /// Checks, stamps, logs and applies every draft in one `BEGIN IMMEDIATE`
    /// transaction. Required fields are checked after the whole batch, so an
    /// Issue's `create` and its required `set_field`s go in together.
    ///
    /// # Errors
    ///
    /// A Scheme error ([`StoreError::UnknownField`], [`StoreError::Invalid`],
    /// [`StoreError::MissingFields`]), an Issue error, [`StoreError::Op`],
    /// [`StoreError::Clock`] or [`StoreError::Busy`]; nothing is written
    /// unless every draft succeeds.
    pub fn append_all(&mut self, drafts: Vec<Draft>) -> Result<Vec<Op>, StoreError> {
        for draft in &drafts {
            draft.change.validate()?;
        }
        let now = self.clock.now().map_err(StoreError::Clock)?;
        let now_ms = u64::try_from(now.utc_ms)
            .ok()
            .filter(|ms| *ms <= MAX_MS)
            .ok_or_else(|| {
                StoreError::Clock(format!("{} ms is outside the supported range", now.utc_ms))
            })?;
        if now.offset_min.unsigned_abs() > MAX_OFFSET_MIN {
            return Err(StoreError::Clock(format!(
                "UTC offset of {} minutes is not a real offset",
                now.offset_min
            )));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let last: i64 = tx.query_row("SELECT v FROM meta WHERE k = 'last_hlc'", [], |row| {
            row.get(0)
        })?;
        let mut hlc = Hlc::from_raw(rust_int(last)?);
        let replica: Vec<u8> =
            tx.query_row("SELECT v FROM meta WHERE k = 'replica_id'", [], |row| {
                row.get(0)
            })?;
        let replica_id = uuid_from(&replica)?;
        let mut ops = Vec::with_capacity(drafts.len());
        // Each Issue's state before the batch is the bar it is held to after:
        // a batch may not make it worse, but an Issue that arrived incomplete
        // or inconsistent stays editable.
        let mut before = BTreeMap::new();
        for draft in drafts {
            if let Entry::Vacant(entry) = before.entry(draft.issue_id) {
                entry.insert((
                    validate::missing_fields(&tx, &self.scheme, draft.issue_id)?,
                    validate::status_fits_type(&tx, &self.scheme, draft.issue_id)?,
                ));
            }
            let change = validate::check(&tx, &self.scheme, draft.issue_id, draft.change)?;
            hlc = hlc.tick(now_ms);
            let base_seq: i64 = tx.query_row(
                "SELECT coalesce(max(server_seq), 0) FROM ops WHERE project = ?1",
                [draft.project.to_string()],
                |row| row.get(0),
            )?;
            let op = Op {
                v: CURRENT_VERSION,
                op_id: Uuid::now_v7(),
                replica_id,
                actor: draft.actor,
                project: draft.project,
                issue_id: draft.issue_id,
                kind: change.kind().to_owned(),
                hlc,
                base_seq: rust_int(base_seq)?,
                utc_ms: now.utc_ms,
                offset_min: now.offset_min,
                payload: serde_json::to_string(&change).map_err(OpError::from)?,
            };
            op.change()?;
            insert_op(&tx, &op, &change)?;
            apply(&tx, &op, None, None)?;
            ops.push(op);
        }
        for (issue, (missing, fitted)) in before {
            let fields: Vec<String> = validate::missing_fields(&tx, &self.scheme, issue)?
                .into_iter()
                .filter(|field| !missing.contains(field))
                .collect();
            if !fields.is_empty() {
                return Err(StoreError::MissingFields { issue, fields });
            }
            if fitted && !validate::status_fits_type(&tx, &self.scheme, issue)? {
                return Err(StoreError::Invalid {
                    field: "status".into(),
                    reason: "is not in the Workflow of the Issue's type".into(),
                    valid: Vec::new(),
                });
            }
        }
        tx.execute(
            "UPDATE meta SET v = ?1 WHERE k = 'last_hlc'",
            [sql_int(hlc.raw())?],
        )?;
        tx.commit()?;
        Ok(ops)
    }

    /// The ids of Issues matching `filter`, in its `sort:` order, else most
    /// recently updated first. Archived Issues are left out unless the filter
    /// names `archived`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Filter`] when the filter does not parse or does not fit
    /// the Scheme, or any SQLite failure.
    pub fn find(&self, filter: &str, context: Context<'_>) -> Result<Vec<Uuid>, StoreError> {
        let resolved = resolve(&parse(filter)?, &self.scheme, context)?;
        let (sql, params) = query::compile(&resolved);
        let mut stmt = self.conn.prepare(&sql)?;
        let ids = stmt.query_map(rusqlite::params_from_iter(params), |row| {
            row.get::<_, Vec<u8>>(0)
        })?;
        ids.map(|id| uuid_from(&id?)).collect()
    }

    /// An Issue as it currently reads, labelled from the Scheme.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row;
    /// `Ok(None)` when the Issue does not exist.
    pub fn issue(&self, issue_id: Uuid) -> Result<Option<IssueView>, StoreError> {
        // One read transaction, so the Issue never mixes two writers' commits.
        let tx = self.conn.unchecked_transaction()?;
        read::issue(&tx, &self.scheme, issue_id)
    }

    /// Logged operations the last rebuild could not apply, in log order.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn unapplied(&self) -> Result<Vec<Unapplied>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq, op_id, reason FROM unapplied ORDER BY seq")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (seq, op_id, reason) = row?;
            Ok(Unapplied {
                seq,
                op_id: uuid_from(&op_id)?,
                reason,
            })
        })
        .collect()
    }

    /// Drops the projection tables and replays the whole log into fresh ones,
    /// in one transaction, stamping `PROJECTION_VERSION` last. Each operation's
    /// stored hash is checked against its content before it is applied; one
    /// whose content does not fit is recorded and counted, see
    /// [`Store::unapplied`].
    ///
    /// # Errors
    ///
    /// [`StoreError::Replay`] naming the first operation that cannot be read
    /// or fails its hash, which leaves the previous projections in place, or
    /// [`StoreError::Busy`].
    pub fn rebuild(&mut self) -> Result<usize, StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(schema::DROP_PROJECTIONS)?;
        tx.execute_batch(schema::PROJECTIONS)?;
        let rows = {
            let mut stmt = tx.prepare(
                "SELECT seq, op_id, op_hash, v, replica_id, actor, project, issue_id, kind, hlc,
                        base_seq, utc_ms, offset_min, payload, server_seq, server_received_ms
                 FROM ops
                 ORDER BY server_seq IS NULL, server_seq, hlc, replica_id, op_id",
            )?;
            let rows = stmt.query_map([], StoredOp::read)?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut unapplied = 0;
        for row in &rows {
            if !replay(&tx, row)? {
                unapplied += 1;
            }
        }
        tx.execute_batch(&format!(
            "PRAGMA user_version = {}",
            schema::PROJECTION_VERSION
        ))?;
        tx.commit()?;
        Ok(unapplied)
    }
}

/// A `set_field` whose labels the Scheme check fills in.
fn set(field: &str, value: serde_json::Value) -> Change {
    Change::SetField {
        field: field.to_owned(),
        label: String::new(),
        value,
        value_label: None,
    }
}

/// Applies one logged operation, returning whether it fit; one that does not
/// is recorded in `unapplied` rather than stopping the rebuild.
fn replay(tx: &Transaction<'_>, row: &StoredOp) -> Result<bool, StoreError> {
    let wrap = |source: StoreError| StoreError::Replay {
        seq: row.seq,
        op_id: Uuid::from_slice(&row.op_id).ok(),
        source: Box::new(source),
    };
    let (op, server_seq, received) = row.decode().map_err(wrap)?;
    if op.op_hash().as_slice() != row.op_hash.as_slice() {
        return Err(wrap(StoreError::Corrupt(
            "its stored hash does not match its content".into(),
        )));
    }
    let sql = |result: rusqlite::Result<usize>| result.map_err(|err| wrap(err.into()));
    sql(tx.execute("SAVEPOINT replay_op", []))?;
    match apply(tx, &op, server_seq, received) {
        Ok(()) => {
            sql(tx.execute("RELEASE replay_op", []))?;
            Ok(true)
        }
        Err(
            err @ (StoreError::UnknownIssue(_)
            | StoreError::DuplicateIssue(_)
            | StoreError::ProjectMismatch { .. }
            | StoreError::Invalid { .. }),
        ) => {
            sql(tx.execute("ROLLBACK TO replay_op", []))?;
            sql(tx.execute("RELEASE replay_op", []))?;
            sql(tx.execute(
                "INSERT INTO unapplied (seq, op_id, reason) VALUES (?1, ?2, ?3)",
                params![row.seq, op.op_id.as_bytes(), err.to_string()],
            ))?;
            Ok(false)
        }
        Err(err) => Err(wrap(err)),
    }
}

fn insert_op(tx: &Transaction<'_>, op: &Op, change: &Change) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO ops (op_id, op_hash, v, kind, replica_id, actor, project, issue_id, field, elem,
                          hlc, base_seq, utc_ms, offset_min, payload)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            op.op_id.as_bytes(),
            op.op_hash(),
            op.v,
            op.kind,
            op.replica_id.as_bytes(),
            op.actor,
            op.project.to_string(),
            op.issue_id.as_bytes(),
            change.field(),
            change.elem(),
            sql_int(op.hlc.raw())?,
            sql_int(op.base_seq)?,
            op.utc_ms,
            op.offset_min,
            op.payload,
        ],
    )?;
    Ok(())
}

/// One row of the log as SQLite returns it, before conversion can fail.
struct StoredOp {
    seq: i64,
    op_id: Vec<u8>,
    op_hash: Vec<u8>,
    v: i64,
    replica_id: Vec<u8>,
    actor: String,
    project: String,
    issue_id: Vec<u8>,
    kind: String,
    hlc: i64,
    base_seq: i64,
    utc_ms: i64,
    offset_min: i64,
    payload: String,
    server_seq: Option<i64>,
    server_received_ms: Option<i64>,
}

impl StoredOp {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            seq: row.get(0)?,
            op_id: row.get(1)?,
            op_hash: row.get(2)?,
            v: row.get(3)?,
            replica_id: row.get(4)?,
            actor: row.get(5)?,
            project: row.get(6)?,
            issue_id: row.get(7)?,
            kind: row.get(8)?,
            hlc: row.get(9)?,
            base_seq: row.get(10)?,
            utc_ms: row.get(11)?,
            offset_min: row.get(12)?,
            payload: row.get(13)?,
            server_seq: row.get(14)?,
            server_received_ms: row.get(15)?,
        })
    }

    fn decode(&self) -> Result<(Op, Option<u64>, Option<u64>), StoreError> {
        let narrow =
            |name: &str, value: i64| StoreError::Corrupt(format!("{name} {value} is out of range"));
        let op = Op {
            v: u16::try_from(self.v).map_err(|_| narrow("version", self.v))?,
            op_id: uuid_from(&self.op_id)?,
            replica_id: uuid_from(&self.replica_id)?,
            actor: self.actor.clone(),
            project: project_from(&self.project)?,
            issue_id: uuid_from(&self.issue_id)?,
            kind: self.kind.clone(),
            hlc: Hlc::from_raw(rust_int(self.hlc)?),
            base_seq: rust_int(self.base_seq)?,
            utc_ms: self.utc_ms,
            offset_min: i16::try_from(self.offset_min)
                .map_err(|_| narrow("UTC offset", self.offset_min))?,
            payload: self.payload.clone(),
        };
        let server_seq = self.server_seq.map(rust_int).transpose()?;
        let received = self.server_received_ms.map(rust_int).transpose()?;
        Ok((op, server_seq, received))
    }
}
