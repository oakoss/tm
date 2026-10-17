//! SQLite store: the operation log, its table projections and the read-only snapshot.
//!
//! The log is the source of truth (ADR 0005). Every write appends one operation
//! and applies it to the projection tables in the same `BEGIN IMMEDIATE`
//! transaction; `rebuild` regenerates the projections from the log.

mod apply;
mod schema;

use std::path::Path;
use std::time::Duration;

use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior, params,
};
use tm_core::hlc::MAX_MS;
use tm_core::op::CURRENT_VERSION;
use tm_core::{Change, Hlc, Op, OpError};
use uuid::Uuid;

use apply::{apply, rust_int, sql_int, uuid_from};

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
    #[error("Issue {issue} belongs to Project {expected}, not {found}")]
    ProjectMismatch {
        issue: Uuid,
        expected: String,
        found: String,
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

/// A change to write, before the store stamps it with identity and time.
#[derive(Debug, Clone)]
pub struct Draft {
    pub actor: String,
    pub project: String,
    pub issue_id: Uuid,
    pub change: Change,
}

pub struct Store {
    conn: Connection,
    clock: Box<dyn Clock + Send>,
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
        let mut store = Self { conn, clock };
        store.init()?;
        Ok(store)
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

    /// Stamps `draft` as an operation, appends it to the log and applies it,
    /// all in one `BEGIN IMMEDIATE` transaction.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownIssue`], [`StoreError::DuplicateIssue`] or
    /// [`StoreError::ProjectMismatch`] when the change does not fit the Issue,
    /// [`StoreError::Op`] when it breaks the
    /// operation format, [`StoreError::Clock`], or [`StoreError::Busy`].
    pub fn append(&mut self, draft: Draft) -> Result<Op, StoreError> {
        draft.change.validate()?;
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
        let hlc = Hlc::from_raw(rust_int(last)?).tick(now_ms);
        let base_seq: i64 = tx.query_row(
            "SELECT coalesce(max(server_seq), 0) FROM ops WHERE project = ?1",
            [&draft.project],
            |row| row.get(0),
        )?;
        let replica: Vec<u8> =
            tx.query_row("SELECT v FROM meta WHERE k = 'replica_id'", [], |row| {
                row.get(0)
            })?;
        let op = Op {
            v: CURRENT_VERSION,
            op_id: Uuid::now_v7(),
            replica_id: uuid_from(&replica)?,
            actor: draft.actor,
            project: draft.project,
            issue_id: draft.issue_id,
            kind: draft.change.kind().to_owned(),
            hlc,
            base_seq: rust_int(base_seq)?,
            utc_ms: now.utc_ms,
            offset_min: now.offset_min,
            payload: serde_json::to_string(&draft.change).map_err(OpError::from)?,
        };
        op.change()?;
        insert_op(&tx, &op, &draft.change)?;
        apply(&tx, &op, None, None)?;
        tx.execute(
            "UPDATE meta SET v = ?1 WHERE k = 'last_hlc'",
            [sql_int(hlc.raw())?],
        )?;
        tx.commit()?;
        Ok(op)
    }

    /// Drops the projection tables and replays the whole log into fresh ones,
    /// in one transaction, stamping `PROJECTION_VERSION` last. Each operation's
    /// stored hash is checked against its content before it is applied.
    ///
    /// # Errors
    ///
    /// [`StoreError::Replay`] naming the first operation that cannot be
    /// replayed, which leaves the previous projections in place, or
    /// [`StoreError::Busy`].
    pub fn rebuild(&mut self) -> Result<(), StoreError> {
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
        for row in &rows {
            replay(&tx, row)?;
        }
        tx.execute_batch(&format!(
            "PRAGMA user_version = {}",
            schema::PROJECTION_VERSION
        ))?;
        tx.commit()?;
        Ok(())
    }
}

fn replay(tx: &Transaction<'_>, row: &StoredOp) -> Result<(), StoreError> {
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
    apply(tx, &op, server_seq, received).map_err(wrap)
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
            op.project,
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
            project: self.project.clone(),
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

/// Reads an Issue's current field values as a JSON object.
///
/// # Errors
///
/// Any SQLite failure; `Ok(None)` when the Issue does not exist.
pub fn issue_fields(conn: &Connection, issue_id: Uuid) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT fields FROM issues WHERE id = ?1",
            [issue_id.as_bytes()],
            |row| row.get(0),
        )
        .optional()?)
}
