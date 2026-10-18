//! SQLite store: the operation log, its table projections and the read-only snapshot.
//!
//! The log is the source of truth (ADR 0005). Every write appends one operation
//! and applies it to the projection tables in the same `BEGIN IMMEDIATE`
//! transaction; `rebuild` regenerates the projections from the log.

mod apply;
mod claim;
mod project;
mod query;
mod read;
mod schema;
mod validate;
pub mod workspace;

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::path::Path;
use std::time::Duration;

use rusqlite::{
    Connection, ErrorCode, MAIN_DB, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
    params,
};
use tm_core::filter::{Context, FilterError, parse, resolve};
use tm_core::hlc::MAX_MS;
use tm_core::op::CURRENT_VERSION;
use tm_core::{Actor, Change, Hlc, Op, OpError, ProjectId, Scheme};
use uuid::Uuid;

use apply::{apply, project_from, rust_int, sql_int, uuid_from};

pub use claim::Claim;
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
    #[error("Project {0} does not exist")]
    UnknownProject(ProjectId),
    #[error("Project {0} already exists")]
    DuplicateProject(ProjectId),
    #[error("Project key {key} belongs to Project {project}")]
    KeyTaken { key: String, project: ProjectId },
    #[error("a Project operation's subject {subject} must be its own Project {project}")]
    SubjectNotProject { subject: Uuid, project: ProjectId },
    #[error(
        "Project {key} can take no more Issues: operation {op_id} gave it the highest Key number"
    )]
    KeysExhausted { key: String, op_id: Uuid },
    #[error("{} is not a tm store", .0.display())]
    NotAStore(std::path::PathBuf),
    #[error("the store is read-only; check the permissions of the file and its directory")]
    ReadOnly,
    #[error("this store belongs to Workspace {stored}, not {given}")]
    WorkspaceMismatch { stored: Uuid, given: Uuid },
    #[error("unknown field {field:?}; valid fields: {}", valid.join(", "))]
    UnknownField { field: String, valid: Vec<String> },
    #[error("{field} {reason}{}", if valid.is_empty() { String::new() } else { format!("; valid: {}", valid.join(", ")) })]
    Invalid {
        field: String,
        reason: String,
        /// The values that would be accepted, when they can be listed.
        valid: Vec<String>,
    },
    #[error("{relation} would close a cycle: {}", path.iter().map(Uuid::to_string).collect::<Vec<_>>().join(" -> "))]
    Cycle {
        relation: &'static str,
        /// From the Issue written, around the cycle and back to it.
        path: Vec<Uuid>,
    },
    #[error("Issue {issue} is held by {agent} for {on_behalf_of} (Claim {claim}{})", expires_ms.map_or_else(String::new, |ms| format!(", expiring at {ms} ms")))]
    AlreadyClaimed {
        issue: Uuid,
        agent: String,
        on_behalf_of: String,
        claim: Uuid,
        expires_ms: Option<i64>,
    },
    #[error("Claim {claim} does not hold Issue {issue}{}", current.map_or_else(|| "; it is unclaimed".to_owned(), |current| format!("; Claim {current} does")))]
    NotClaimHolder {
        issue: Uuid,
        claim: Uuid,
        current: Option<Uuid>,
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

/// A logged operation replay skipped because its content did not fit, or a
/// create it applied without the Key it was assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unapplied {
    pub seq: i64,
    pub op_id: Uuid,
    pub reason: String,
}

impl Unapplied {
    /// A take or release another won, which sync makes ordinary, rather
    /// than an operation that does not fit.
    #[must_use]
    pub fn is_lost_claim(&self) -> bool {
        self.reason.starts_with(claim::LOST)
    }
}

/// A change to write, before the store stamps it with identity and time.
#[derive(Debug, Clone)]
pub struct Draft {
    pub actor: Actor,
    pub project: ProjectId,
    pub subject: Uuid,
    pub change: Change,
}

pub struct Store {
    conn: Connection,
    clock: Box<dyn Clock + Send>,
    scheme: Scheme,
    opened: bool,
}

impl Drop for Store {
    fn drop(&mut self) {
        // Best effort: a failed optimize costs query planning, never data.
        // A store that failed to open is left as it was found.
        if self.opened {
            let _ = self.conn.execute_batch("PRAGMA optimize");
        }
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
        Self::setup(Connection::open(path)?, clock)
    }

    /// Opens Workspace `workspace`'s store, which must already exist. A
    /// read-only look first refuses a file that is not a tm store, a store of
    /// another Workspace and one whose format this tm cannot read, so none of
    /// them is written to; reading a WAL database can still leave SQLite's
    /// `-shm` beside it. A store with no Workspace recorded yet, as an
    /// interrupted create leaves it, is given this one.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotAStore`] for a file that is not SQLite or holds no tm
    /// metadata, [`StoreError::WorkspaceMismatch`], [`StoreError::NewerStore`],
    /// [`StoreError::Corrupt`], [`StoreError::ReadOnly`], or as
    /// [`Store::open`].
    pub fn open_existing(path: &Path, workspace: Uuid) -> Result<Self, StoreError> {
        // SQLite falls back to read-only when the file cannot be written, and
        // fails once it needs a `-shm` or `-wal` the directory cannot hold. A
        // file moved or deleted while open fails as read-only too, but is not
        // a permissions problem.
        const DBMOVED: std::ffi::c_int = 1032;
        let read_only = |err: StoreError| match err {
            StoreError::Sqlite(rusqlite::Error::SqliteFailure(ref failure, _))
                if failure.code == ErrorCode::ReadOnly && failure.extended_code != DBMOVED =>
            {
                StoreError::ReadOnly
            }
            other => other,
        };
        Self::check_existing(path, workspace).map_err(read_only)?;
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        if conn.is_readonly(MAIN_DB)? {
            return Err(StoreError::ReadOnly);
        }
        let mut store = Self::setup(conn, Box::new(SystemClock)).map_err(read_only)?;
        store.set_workspace_id(workspace).map_err(read_only)?;
        Ok(store)
    }

    fn check_existing(path: &Path, workspace: Uuid) -> Result<(), StoreError> {
        let not_a_store = || StoreError::NotAStore(path.to_path_buf());
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        let has_meta: bool = match conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'meta')",
            [],
            |row| row.get(0),
        ) {
            Ok(found) => found,
            Err(err) if err.sqlite_error_code() == Some(ErrorCode::NotADatabase) => {
                return Err(not_a_store());
            }
            Err(err) => return Err(err.into()),
        };
        if !has_meta {
            return Err(not_a_store());
        }
        let malformed = |what: &'static str| {
            move |err: rusqlite::Error| match err {
                rusqlite::Error::InvalidColumnType(..) => {
                    StoreError::Corrupt(format!("the store's {what} is malformed"))
                }
                other => other.into(),
            }
        };
        let format: Option<i64> = conn
            .query_row("SELECT v FROM meta WHERE k = 'format'", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(malformed("format"))?;
        let Some(format) = format else {
            return Err(not_a_store());
        };
        if format > schema::STORE_FORMAT {
            return Err(StoreError::NewerStore {
                found: format,
                supported: schema::STORE_FORMAT,
            });
        }
        if format < 1 {
            return Err(StoreError::Corrupt(format!(
                "unknown store format {format}"
            )));
        }
        // Every store tm has set up guards its log with these triggers; a view
        // or virtual table cannot carry them. Without them this is another
        // program's database, or a store that lost its source of truth, and
        // setup would write to it.
        let guarded_log: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_schema
             WHERE type = 'trigger' AND tbl_name = 'ops'
               AND name IN ('ops_immutable', 'ops_append_only')",
            [],
            |row| row.get(0),
        )?;
        if guarded_log != 2 {
            return Err(StoreError::Corrupt(
                "the store's operation log is missing or not tm's".into(),
            ));
        }
        let stored: Option<Vec<u8>> = conn
            .query_row("SELECT v FROM meta WHERE k = 'workspace_id'", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(malformed("Workspace id"))?;
        if let Some(bytes) = stored {
            let stored = uuid_from(&bytes)?;
            if stored != workspace {
                return Err(StoreError::WorkspaceMismatch {
                    stored,
                    given: workspace,
                });
            }
        }
        Ok(())
    }

    fn setup(conn: Connection, clock: Box<dyn Clock + Send>) -> Result<Self, StoreError> {
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
            opened: false,
        };
        store.init()?;
        store.opened = true;
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
            Some(1) => {
                tx.execute_batch(schema::FORMAT_2)?;
                tx.execute_batch(schema::LOG)?;
                tx.execute("UPDATE meta SET v = 2 WHERE k = 'format'", [])?;
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

    /// The Workspace this store belongs to, once recorded. Every Replica of a
    /// Workspace shares it, unlike [`Store::replica_id`].
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] if the stored id is malformed.
    pub fn workspace_id(&self) -> Result<Option<Uuid>, StoreError> {
        let bytes: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT v FROM meta WHERE k = 'workspace_id'", [], |row| {
                row.get(0)
            })
            .optional()?;
        bytes.map(|bytes| uuid_from(&bytes)).transpose()
    }

    /// Records the Workspace this store belongs to, once: a store never moves
    /// between Workspaces.
    ///
    /// # Errors
    ///
    /// [`StoreError::WorkspaceMismatch`] when another id is recorded,
    /// [`StoreError::Corrupt`] if the stored id is malformed, any SQLite
    /// failure, or [`StoreError::Busy`].
    pub fn set_workspace_id(&mut self, id: Uuid) -> Result<(), StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored: Option<Vec<u8>> = tx
            .query_row("SELECT v FROM meta WHERE k = 'workspace_id'", [], |row| {
                row.get(0)
            })
            .optional()?;
        match stored.map(|bytes| uuid_from(&bytes)).transpose()? {
            Some(stored) if stored == id => return Ok(()),
            Some(stored) => return Err(StoreError::WorkspaceMismatch { stored, given: id }),
            None => {
                tx.execute(
                    "INSERT INTO meta (k, v) VALUES ('workspace_id', ?1)",
                    [id.as_bytes()],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
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

    /// Creates a Project. Its key is accepted in any case and stored uppercase.
    ///
    /// # Errors
    ///
    /// [`StoreError::Op`] for a malformed key, [`StoreError::KeyTaken`] when
    /// any Project holds or once held the key, or as [`Store::append_all`].
    pub fn create_project(
        &mut self,
        actor: &Actor,
        key: &str,
        name: &str,
    ) -> Result<(ProjectId, Op), StoreError> {
        let project = ProjectId::new();
        let op = self.append(Draft {
            actor: actor.clone(),
            project,
            subject: project.uuid(),
            change: Change::CreateProject {
                key: key.to_ascii_uppercase(),
                name: name.to_owned(),
            },
        })?;
        Ok((project, op))
    }

    /// Gives a Project a new key, name or both. Its old key keeps resolving to
    /// it and is never given to another Project.
    ///
    /// # Errors
    ///
    /// As [`Store::create_project`], or [`StoreError::UnknownProject`].
    pub fn rename_project(
        &mut self,
        actor: &Actor,
        project: ProjectId,
        key: Option<&str>,
        name: Option<&str>,
    ) -> Result<Op, StoreError> {
        self.append(Draft {
            actor: actor.clone(),
            project,
            subject: project.uuid(),
            change: Change::RenameProject {
                key: key.map(str::to_ascii_uppercase),
                name: name.map(str::to_owned),
            },
        })
    }

    /// The Project holding `key` now or in the past, matched without case.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn project_by_key(&self, key: &str) -> Result<Option<ProjectId>, StoreError> {
        project::key_holder(&self.conn, &key.to_ascii_uppercase())
    }

    /// The Issue a Key names, matched without case. Any key its Project holds
    /// or held finds it, so `CPL-42` still resolves after `CPL` becomes `KERN`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Invalid`] when `key` is not written as a Key, any SQLite
    /// failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn issue_by_key(&self, key: &str) -> Result<Option<Uuid>, StoreError> {
        let upper = key.to_ascii_uppercase();
        let (prefix, number) =
            tm_core::project::parse_issue_key(&upper).ok_or_else(|| StoreError::Invalid {
                field: "key".into(),
                reason: format!("{key:?} is not a Key such as CPL-42"),
                valid: Vec::new(),
            })?;
        let Some(project) = project::key_holder(&self.conn, prefix)? else {
            return Ok(None);
        };
        project::issue_by_number(&self.conn, project, number)
    }

    /// Creates an Issue in its Workflow's starting Status.
    ///
    /// # Errors
    ///
    /// As [`Store::append_all`].
    pub fn create_issue(
        &mut self,
        actor: &Actor,
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
        let actor = actor.clone();
        let drafts = changes
            .into_iter()
            .map(|change| Draft {
                actor: actor.clone(),
                project,
                subject: issue_id,
                change,
            })
            .collect();
        Ok((issue_id, self.append_all(drafts)?))
    }

    /// Links `from` to `to`: "`from` blocks `to`" for `blocks`. The Link is
    /// filed under its canonical source, so the same Link written from either
    /// end is one Link (ADR 0009).
    ///
    /// # Errors
    ///
    /// [`StoreError::Invalid`] for an unknown type or an Issue joined to
    /// itself, [`StoreError::UnknownIssue`], [`StoreError::Cycle`] for a
    /// `blocks` Link that would close a cycle, or as [`Store::append_all`].
    pub fn link(
        &mut self,
        actor: &Actor,
        link_type: &str,
        from: Uuid,
        to: Uuid,
    ) -> Result<Op, StoreError> {
        let draft = self.link_draft(actor.clone(), link_type, from, to, true)?;
        self.append(draft)
    }

    /// Removes the Link [`Store::link`] would add.
    ///
    /// # Errors
    ///
    /// As [`Store::link`], except that a held Link of any type can be removed.
    pub fn unlink(
        &mut self,
        actor: &Actor,
        link_type: &str,
        from: Uuid,
        to: Uuid,
    ) -> Result<Op, StoreError> {
        let draft = self.link_draft(actor.clone(), link_type, from, to, false)?;
        self.append(draft)
    }

    fn link_draft(
        &self,
        actor: Actor,
        link_type: &str,
        from: Uuid,
        to: Uuid,
        present: bool,
    ) -> Result<Draft, StoreError> {
        let (source, target) = tm_core::link::link_type(link_type)
            .map_or((from, to), |def| tm_core::link::canonical(def, from, to));
        let Some(project) = apply::issue_project(&self.conn, source)? else {
            // A missing source holds no Link, so a type or pairing that cannot
            // be written is the error to report.
            validate::link_shape(link_type, from, to)?;
            return Err(StoreError::UnknownIssue(source));
        };
        Ok(Draft {
            actor,
            project,
            subject: source,
            change: Change::Link {
                link_type: link_type.to_owned(),
                target,
                present,
            },
        })
    }

    /// Checks, stamps, logs and applies every draft in one `BEGIN IMMEDIATE`
    /// transaction. Required fields are checked after the whole batch, so an
    /// Issue's `create` and its required `set_field`s go in together. The
    /// batch is named by its first operation's id, which `tm undo` takes
    /// (ADR 0010).
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
        let (now, now_ms) = self.now()?;
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
        let mut ops: Vec<Op> = Vec::with_capacity(drafts.len());
        // Each Issue's state before the batch is the bar it is held to after:
        // a batch may not make it worse, but an Issue that arrived incomplete
        // or inconsistent stays editable.
        let mut before = BTreeMap::new();
        for draft in drafts {
            if let Entry::Vacant(entry) = before.entry(draft.subject) {
                entry.insert((
                    validate::missing_fields(&tx, &self.scheme, draft.subject)?,
                    validate::status_fits_type(&tx, &self.scheme, draft.subject)?,
                ));
            }
            let change = validate::check(&tx, &self.scheme, draft.subject, draft.change)?;
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
                actor: draft.actor.name().to_owned(),
                on_behalf_of: draft.actor.on_behalf_of().map(str::to_owned),
                project: draft.project,
                subject: draft.subject,
                kind: change.kind().to_owned(),
                hlc,
                base_seq: rust_int(base_seq)?,
                utc_ms: now.utc_ms,
                offset_min: now.offset_min,
                payload: serde_json::to_string(&change).map_err(OpError::from)?,
            };
            op.change()?;
            // Inside the write transaction, so concurrent creates never share a Key.
            let assigned_key = match change {
                Change::Create => Some(project::next_key(&tx, op.project)?),
                _ => None,
            };
            insert_op(&tx, &op, &change, assigned_key.as_deref())?;
            let batch = ops.first().map_or(op.op_id, |first| first.op_id);
            tx.execute(
                "INSERT INTO op_batches (op_id, batch) VALUES (?1, ?2)",
                [op.op_id.as_bytes(), batch.as_bytes()],
            )?;
            let effects = apply::Effects {
                assigned_key: assigned_key.as_deref(),
                ..apply::Effects::default()
            };
            if let Some(reason) = apply(&tx, &op, effects)? {
                return Err(StoreError::Corrupt(format!(
                    "a write this store checked did not apply in full: {reason}"
                )));
            }
            if let Some(key) = &assigned_key {
                project::count_key(&tx, key, op.project, op.op_id)?;
            }
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

    /// The clock's reading, and its milliseconds as the clock counts them.
    fn now(&self) -> Result<(WallTime, u64), StoreError> {
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
        Ok((now, now_ms))
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
    /// whose content does not fit, or a create whose Key cannot be recorded,
    /// is recorded and counted, see [`Store::unapplied`].
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
                        base_seq, utc_ms, offset_min, payload, server_seq, server_received_ms,
                        assigned_key, on_behalf_of
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
        unapplied += record_dangling_links(&tx)?;
        tx.execute_batch(&format!(
            "PRAGMA user_version = {}",
            schema::PROJECTION_VERSION
        ))?;
        tx.commit()?;
        Ok(unapplied)
    }
}

/// Replay keeps a Link whose target has not replayed, since it may be in a
/// later chain; once every chain has, one still missing is recorded against
/// the operation that added the Link.
fn record_dangling_links(tx: &Transaction<'_>) -> Result<usize, StoreError> {
    let dangling = {
        let mut stmt = tx
            .prepare("SELECT type, src, dst FROM links WHERE dst NOT IN (SELECT id FROM issues)")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (link_type, src, dst) in &dangling {
        let target = uuid_from(dst)?;
        let (seq, op_id): (i64, Vec<u8>) = tx.query_row(
            "SELECT o.seq, o.op_id FROM field_versions v JOIN ops o ON o.op_id = v.op_id
             WHERE v.issue_id = ?1 AND v.field = ?2 AND v.elem = ?3",
            params![
                src,
                tm_core::link::FIELD,
                tm_core::link::elem(link_type, target)
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        tx.execute(
            "INSERT INTO unapplied (seq, op_id, reason) VALUES (?1, ?2, ?3)",
            params![
                seq,
                op_id,
                format!("applied, but the Issue {target} it links to does not exist")
            ],
        )?;
    }
    Ok(dangling.len())
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
    let effects = apply::Effects {
        server_seq,
        server_received_ms: received,
        assigned_key: row.assigned_key.as_deref(),
    };
    let applied = match apply(tx, &op, effects) {
        Ok(None) => {
            sql(tx.execute("RELEASE replay_op", []))?;
            true
        }
        Ok(Some(reason)) => {
            sql(tx.execute("RELEASE replay_op", []))?;
            sql(tx.execute(
                "INSERT INTO unapplied (seq, op_id, reason) VALUES (?1, ?2, ?3)",
                params![row.seq, op.op_id.as_bytes(), reason],
            ))?;
            false
        }
        Err(
            err @ (StoreError::UnknownIssue(_)
            | StoreError::DuplicateIssue(_)
            | StoreError::UnknownProject(_)
            | StoreError::DuplicateProject(_)
            | StoreError::KeyTaken { .. }
            | StoreError::ProjectMismatch { .. }
            | StoreError::SubjectNotProject { .. }
            | StoreError::Invalid { .. }),
        ) => {
            sql(tx.execute("ROLLBACK TO replay_op", []))?;
            sql(tx.execute("RELEASE replay_op", []))?;
            sql(tx.execute(
                "INSERT INTO unapplied (seq, op_id, reason) VALUES (?1, ?2, ?3)",
                params![row.seq, op.op_id.as_bytes(), err.to_string()],
            ))?;
            false
        }
        Err(err) => return Err(wrap(err)),
    };
    if let Some(key) = row.assigned_key.as_deref() {
        project::count_key(tx, key, op.project, op.op_id).map_err(wrap)?;
    }
    Ok(applied)
}

fn insert_op(
    tx: &Transaction<'_>,
    op: &Op,
    change: &Change,
    assigned_key: Option<&str>,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO ops (op_id, op_hash, v, kind, replica_id, actor, on_behalf_of, project,
                          issue_id, field, elem, hlc, base_seq, utc_ms, offset_min, payload,
                          assigned_key)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            op.op_id.as_bytes(),
            op.op_hash(),
            op.v,
            op.kind,
            op.replica_id.as_bytes(),
            op.actor,
            op.on_behalf_of,
            op.project.to_string(),
            op.subject.as_bytes(),
            change.field(),
            change.elem(),
            sql_int(op.hlc.raw())?,
            sql_int(op.base_seq)?,
            op.utc_ms,
            op.offset_min,
            op.payload,
            assigned_key,
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
    assigned_key: Option<String>,
    on_behalf_of: Option<String>,
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
            assigned_key: row.get(16)?,
            on_behalf_of: row.get(17)?,
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
            on_behalf_of: self.on_behalf_of.clone(),
            project: project_from(&self.project)?,
            subject: uuid_from(&self.issue_id)?,
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
