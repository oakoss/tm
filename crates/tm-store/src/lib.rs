//! SQLite store: the operation log, its table projections and the read-only Snapshot.
//!
//! The log is the source of truth (ADR 0005). Every write appends one operation
//! and applies it to the projection tables in the same `BEGIN IMMEDIATE`
//! transaction; `rebuild` regenerates the projections from the log.

mod apply;
mod beads;
mod claim;
mod export;
mod family;
mod history;
mod import;
mod import_list;
mod membership;
mod next;
mod project;
mod query;
mod read;
mod ready;
mod record;
mod schema;
mod shortlist;
mod snapshot;
mod undo;
mod validate;
mod version;
pub mod workspace;

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::time::Duration;

use rusqlite::{
    Connection, ErrorCode, MAIN_DB, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
    params,
};
use tm_core::filter::{Context, FilterError, Query, Term, Value as FilterValue, parse, resolve};
use tm_core::hlc::MAX_MS;
use tm_core::op::CURRENT_VERSION;
use tm_core::{Actor, Change, Hlc, Op, OpError, ProjectId, Scheme};
use uuid::Uuid;

use apply::{apply, project_from, rust_int, sql_int, uuid_from};

pub use beads::{BeadsExport, BeadsImported, read_beads};
pub use claim::{
    AgentClaims, Claim, ClaimState, Holder, PROTECTED_FIELDS, TakenWith, short_session,
};
pub use export::{Exported, Restored, copy_store, read_export};
pub use family::Lineage;
pub use history::HistoryEntry;
pub use import::{Conflict, RecordOutcome, RecordsImported};
pub use import_list::{ListOutcome, Passed};
pub use membership::{Member, PassedOver, Place, Placed};
pub use next::Next;
pub use read::{CommentView, Direction, IssueView, LinkView};
pub use ready::Ready;
pub use record::{
    CommentRecord, IssueRecord, LinkRecord, ListRecord, MemberRecord, list_source, tm_source,
};
pub use shortlist::{Shortlist, ShortlistEdit};
pub use undo::{Restore, Target, UndoPlan};

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
    #[error("Short-list {0} does not exist")]
    UnknownShortlist(Uuid),
    #[error("Short-list {0} already exists")]
    DuplicateShortlist(Uuid),
    #[error("Short-list {list} belongs to Project {expected}, not {found}")]
    ShortlistMismatch {
        list: Uuid,
        expected: ProjectId,
        found: ProjectId,
    },
    #[error("no Short-list in this Project is named {0}")]
    ShortlistNotFound(String),
    #[error("{count} Short-lists in this Project are named {name}; rename one first")]
    ShortlistNameShared { name: String, count: usize },
    #[error("Short-list {name} already holds its size of {size}")]
    ShortlistFull { name: String, size: u64 },
    #[error("Issue {issue} cannot go on a Short-list there: {reason}")]
    CannotJoin { issue: Named, reason: String },
    #[error("Issue {0} is on no Short-list")]
    NotOnShortlist(Named),
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
    #[error(
        "Workspace {workspace} already holds {held} operations that are not this export's; an export restores only into a Workspace with none"
    )]
    WorkspaceNotEmpty { workspace: Uuid, held: i64 },
    #[error("line {line} of the file: {reason}")]
    InvalidExport { line: usize, reason: String },
    #[error("the export's operations are format {found}, and this tm reads {supported}")]
    ExportFormat { found: u16, supported: u16 },
    #[error("reading or writing an export: {0}")]
    Io(#[source] std::io::Error),
    #[error("{}: {source}", path.display())]
    Snapshot {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error(
        "line {line} of the import, {record}: {cause}; {}",
        import::kept(written)
    )]
    RecordRefused {
        line: usize,
        /// The refused record's source.
        record: String,
        cause: Box<StoreError>,
        /// The batches the import wrote before it, which stay.
        written: Vec<Uuid>,
    },
    #[error("Issue {0} changed while it was being imported")]
    ImportChanged(Named),
    #[error("Issue {0}'s Claim was taken, renewed or released while it was being written")]
    ClaimChanged(Named),
    #[error("unknown field {field:?}; valid fields: {}", valid.join(", "))]
    UnknownField { field: String, valid: Vec<String> },
    #[error("{field} {reason}{}", if valid.is_empty() { String::new() } else { format!("; valid: {}", valid.join(", ")) })]
    Invalid {
        field: String,
        reason: String,
        /// The values that would be accepted, when they can be listed.
        valid: Vec<String>,
    },
    #[error("{relation} would close a cycle: {}", path.iter().map(Named::to_string).collect::<Vec<_>>().join(" -> "))]
    Cycle {
        relation: &'static str,
        /// From the Issue written, around the cycle and back to it.
        path: Vec<Named>,
    },
    #[error("no batch {0} was written on this Replica")]
    UnknownBatch(Uuid),
    #[error("batch {batch} cannot be undone: {} was written again since", conflicts.iter().map(|c| c.field.as_str()).collect::<Vec<_>>().join(", "))]
    UndoConflict {
        batch: Uuid,
        conflicts: Vec<undo::Restore>,
    },
    #[error("batch {batch} cannot be undone: {reason}")]
    CannotUndo { batch: Uuid, reason: String },
    #[error("Issue {from} holds no {link_type} Link to {to}")]
    NotLinked {
        link_type: String,
        from: Box<Named>,
        to: Box<Named>,
    },
    #[error("only an Agent can take a Claim, and {actor} writes as a human")]
    NoAgent { actor: String },
    #[error("Issue {issue} is held by {} for {}{}", holder.taker(), holder.on_behalf_of, holder.expires_ms.map_or_else(String::new, |ms| format!(", expiring at {ms} ms")))]
    AlreadyClaimed { issue: Named, holder: Box<Claim> },
    #[error("Issue {issue}'s {field} is protected: {} holds it for {}{}", holder.taker(), holder.on_behalf_of, holder.expires_ms.map_or_else(String::new, |ms| format!(", expiring at {ms} ms")))]
    Protected {
        issue: Named,
        field: String,
        holder: Box<Claim>,
    },
    #[error("Issue {issue}'s {} changed after version {based_on}, so nothing was written; it is at version {version}", current.keys().cloned().collect::<Vec<_>>().join(" and "))]
    Stale {
        issue: Named,
        based_on: i64,
        version: i64,
        current: BTreeMap<String, serde_json::Value>,
    },
    #[error("Claim {claim} does not hold Issue {issue}; {holder}")]
    NotClaimHolder {
        issue: Named,
        claim: Uuid,
        holder: Holder,
    },
    #[error("Issue {issue} is missing required fields: {}", fields.join(", "))]
    MissingFields { issue: Named, fields: Vec<String> },
    #[error("Issue {issue} belongs to Project {expected}, not {found}")]
    ProjectMismatch {
        issue: Named,
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
    #[error(
        "the store's log was written by a different build of tm, before its format last changed; recreate the store"
    )]
    FormatChanged,
    #[error(
        "the Snapshot was published by a different build of tm (projection {found}, this tm reads {expected})"
    )]
    SnapshotVersion { found: i64, expected: i64 },
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

/// An Issue as an error names it: by its Key, else by the import record
/// that created it, else by its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub id: Uuid,
    pub key: Option<String>,
    /// The `source` of the record an import created the Issue from, which
    /// the file being imported holds where a Key or id would mean nothing.
    pub source: Option<String>,
}

impl From<Uuid> for Named {
    fn from(id: Uuid) -> Self {
        Self {
            id,
            key: None,
            source: None,
        }
    }
}

impl std::fmt::Display for Named {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.key, &self.source) {
            (Some(key), _) => f.write_str(key),
            (None, Some(source)) => f.write_str(source),
            (None, None) => self.id.fmt(f),
        }
    }
}

impl StoreError {
    /// This error with `name` applied to each Issue it names.
    fn map_named(self, mut name: impl FnMut(Named) -> Named) -> Self {
        match self {
            Self::Cycle { relation, path } => Self::Cycle {
                relation,
                path: path.into_iter().map(name).collect(),
            },
            Self::MissingFields { issue, fields } => Self::MissingFields {
                issue: name(issue),
                fields,
            },
            Self::ImportChanged(issue) => Self::ImportChanged(name(issue)),
            Self::NotOnShortlist(issue) => Self::NotOnShortlist(name(issue)),
            Self::CannotJoin { issue, reason } => Self::CannotJoin {
                issue: name(issue),
                reason,
            },
            Self::ClaimChanged(issue) => Self::ClaimChanged(name(issue)),
            Self::NotLinked {
                link_type,
                from,
                to,
            } => Self::NotLinked {
                link_type,
                from: Box::new(name(*from)),
                to: Box::new(name(*to)),
            },
            Self::AlreadyClaimed { issue, holder } => Self::AlreadyClaimed {
                issue: name(issue),
                holder,
            },
            Self::Protected {
                issue,
                field,
                holder,
            } => Self::Protected {
                issue: name(issue),
                field,
                holder,
            },
            Self::Stale {
                issue,
                based_on,
                version,
                current,
            } => Self::Stale {
                issue: name(issue),
                based_on,
                version,
                current,
            },
            Self::NotClaimHolder {
                issue,
                claim,
                holder,
            } => Self::NotClaimHolder {
                issue: name(issue),
                claim,
                holder,
            },
            Self::ProjectMismatch {
                issue,
                expected,
                found,
            } => Self::ProjectMismatch {
                issue: name(issue),
                expected,
                found,
            },
            other => other,
        }
    }

    /// This error with each Issue it names given its Key, read from `conn`
    /// once the refused write rolled back, so a Key the write would have
    /// assigned is never named. Past a Key that cannot be read, the rest keep
    /// their ids.
    fn named(self, conn: &Connection) -> Self {
        if !conn.is_autocommit() {
            return self;
        }
        let mut readable = true;
        self.map_named(|issue| {
            if readable {
                match project::key_of(conn, issue.id) {
                    Ok(key) => return Named { key, ..issue },
                    Err(_) => readable = false,
                }
            }
            issue
        })
    }

    /// This error with `issue`, which an import created from the record
    /// `source`, named by that record.
    pub(crate) fn sourced(self, issue: Uuid, source: &str) -> Self {
        self.map_named(|named| {
            if named.id == issue {
                Named {
                    key: None,
                    source: Some(source.to_owned()),
                    ..named
                }
            } else {
                named
            }
        })
    }

    /// Whether opening an existing store failed because this process may not
    /// open it, as in a sandbox that denies every write. SQLite reports a
    /// missing file the same way, so the caller checks it exists first.
    #[must_use]
    pub fn is_refused(&self) -> bool {
        match self {
            Self::ReadOnly => true,
            Self::Sqlite(err) => err.sqlite_error_code() == Some(ErrorCode::CannotOpen),
            _ => false,
        }
    }
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

    /// The UTC time alone, for reads, which need no local offset.
    ///
    /// # Errors
    ///
    /// A description of why the time cannot be read.
    fn utc_ms(&self) -> Result<i64, String> {
        self.now().map(|now| now.utc_ms)
    }
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

    // Skips the system time zone, so a broken TZ fails writes but not reads.
    fn utc_ms(&self) -> Result<i64, String> {
        Ok(jiff::Timestamp::now().as_millisecond())
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub actor: Actor,
    pub project: ProjectId,
    pub subject: Uuid,
    pub change: Change,
}

/// Where an imported change came from, and when it happened there (ADR 0013).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub source: String,
    pub at: WallTime,
}

pub struct Store {
    conn: Connection,
    clock: Box<dyn Clock + Send>,
    scheme: Scheme,
    opened: bool,
    last_write: Option<LastWrite>,
    forcing: bool,
    /// Claims a human's or a forced write went past, for the caller to warn of.
    passed: Vec<Claim>,
    based_on: Option<(Uuid, i64)>,
}

/// Whether undoing the last batch this Store wrote would succeed, as tried
/// inside its write; it holds only while nothing has been written since.
#[derive(Debug, Clone, Copy)]
struct LastWrite {
    batch: Uuid,
    undoable: bool,
    data_version: i64,
    max_seq: i64,
}

impl Drop for Store {
    fn drop(&mut self) {
        // Best effort: a failed optimize costs query planning, never data.
        // A store that failed to open, and a Snapshot, are left as found.
        if self.opened {
            // Skipped, not waited for, while another process writes.
            let _ = self.conn.busy_timeout(Duration::ZERO);
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
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Self::check(&conn, path, workspace)
    }

    /// Whether `conn` holds Workspace `workspace`'s tm store in a format
    /// this build reads, checked without writing.
    fn check(conn: &Connection, path: &Path, workspace: Uuid) -> Result<(), StoreError> {
        let not_a_store = || StoreError::NotAStore(path.to_path_buf());
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
        Self::collate(&conn)?;
        let mut store = Self {
            conn,
            clock,
            scheme: Scheme::built_in(),
            opened: false,
            last_write: None,
            forcing: false,
            passed: Vec::new(),
            based_on: None,
        };
        store.init()?;
        store.opened = true;
        Ok(store)
    }

    /// People are typed by hand, so filters and sorts compare them ignoring
    /// case; SQLite's NOCASE folds only A to Z. Uppercasing first makes ß
    /// match SS and final sigma match sigma.
    fn collate(conn: &Connection) -> Result<(), StoreError> {
        conn.create_collation(query::FOLD, |a, b| {
            let fold = |s: &str| s.to_uppercase().to_lowercase();
            fold(a).cmp(&fold(b))
        })?;
        Ok(())
    }

    /// Replaces the Scheme writes are checked against and reads are labelled
    /// with. The projection does not depend on it, so no rebuild follows.
    #[must_use]
    pub fn with_scheme(mut self, scheme: Scheme) -> Self {
        self.scheme = scheme;
        self.last_write = None;
        self
    }

    #[must_use]
    pub const fn scheme(&self) -> &Scheme {
        &self.scheme
    }

    fn init(&mut self) -> Result<(), StoreError> {
        self.check_log_columns()?;
        if self.is_set_up()? {
            // Creates tables and indexes an older build lacked, never columns;
            // with nothing to create it takes no lock.
            self.conn.execute_batch(schema::LOG)?;
            return Ok(());
        }
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

    /// The key a Project holds now.
    ///
    /// # Errors
    ///
    /// Any SQLite failure.
    pub fn project_key(&self, project: ProjectId) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT key FROM projects WHERE id = ?1",
                [project.to_string()],
                |row| row.get(0),
            )
            .optional()?)
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

    /// [`Store::unlink`], refused with [`StoreError::NotLinked`] and nothing
    /// written unless the Link is held when the write begins.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotLinked`], or as [`Store::unlink`].
    pub fn unlink_held(
        &mut self,
        actor: &Actor,
        link_type: &str,
        from: Uuid,
        to: Uuid,
    ) -> Result<Op, StoreError> {
        let draft = self.link_draft(actor.clone(), link_type, from, to, false)?;
        let (source, target) = tm_core::link::link_type(link_type)
            .map_or((from, to), |def| tm_core::link::canonical(def, from, to));
        let mut ops = self.append_guarded(vec![draft], |tx| {
            if validate::has_link(tx, link_type, source, target)? {
                Ok(())
            } else {
                Err(StoreError::NotLinked {
                    link_type: link_type.to_owned(),
                    from: Box::new(source.into()),
                    to: Box::new(target.into()),
                })
            }
        })?;
        Ok(ops.remove(0))
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
        self.append_guarded(drafts, |_| Ok(()))
    }

    /// [`Store::append_all`] for imported changes: each operation carries its
    /// source and the time it happened there, while its clock is still this
    /// Replica's. A bare undo never picks the batch (ADR 0013).
    ///
    /// # Errors
    ///
    /// [`StoreError::Invalid`] for a time outside what an operation can hold,
    /// or as [`Store::append_all`].
    pub fn append_imported(
        &mut self,
        drafts: Vec<(Draft, Provenance)>,
    ) -> Result<Vec<Op>, StoreError> {
        self.append_imported_guarded(drafts, |_| Ok(()))
    }

    /// [`Store::append_imported`] after `guard` passes inside the write.
    fn append_imported_guarded(
        &mut self,
        drafts: Vec<(Draft, Provenance)>,
        guard: impl FnOnce(&Transaction<'_>) -> Result<(), StoreError>,
    ) -> Result<Vec<Op>, StoreError> {
        for (draft, provenance) in &drafts {
            validate::check_importable(&draft.change)?;
            check_wall(provenance.at).map_err(|reason| StoreError::Invalid {
                field: "time".into(),
                reason,
                valid: Vec::new(),
            })?;
        }
        let drafts = drafts
            .into_iter()
            .map(|(draft, provenance)| (draft, Some(provenance)))
            .collect();
        self.append_with(drafts, |tx, _| guard(tx).map(|()| Vec::new()), true, None)
    }

    /// Lets an Agent's later writes past another session's Claim, as `--force` asks.
    pub fn force_past_claims(&mut self) {
        self.forcing = true;
    }

    /// The Claims writes went past since last asked.
    pub fn passed_claims(&mut self) -> Vec<Claim> {
        std::mem::take(&mut self.passed)
    }

    /// [`Store::append_all`], refused with [`StoreError::ClaimChanged`] unless
    /// `issue`'s Claim is still exactly `held`, expiry included, so what was
    /// judged from it holds when the write begins.
    ///
    /// # Errors
    ///
    /// [`StoreError::ClaimChanged`], or as [`Store::append_all`].
    pub fn append_if_claim(
        &mut self,
        drafts: Vec<Draft>,
        issue: Uuid,
        held: Option<&Claim>,
    ) -> Result<Vec<Op>, StoreError> {
        self.append_guarded(drafts, |tx| {
            if claim::current(tx, issue)?.as_ref() == held {
                Ok(())
            } else {
                Err(StoreError::ClaimChanged(issue.into()))
            }
        })
    }

    /// [`Store::append_all`] after `guard` passes inside the same write
    /// transaction, so no other writer can change what it checked.
    fn append_guarded(
        &mut self,
        drafts: Vec<Draft>,
        guard: impl FnOnce(&Transaction<'_>) -> Result<(), StoreError>,
    ) -> Result<Vec<Op>, StoreError> {
        let drafts = drafts.into_iter().map(|draft| (draft, None)).collect();
        self.append_with(drafts, |tx, _| guard(tx).map(|()| Vec::new()), true, None)
    }

    /// [`Store::append_guarded`], rolled back instead of committed when
    /// `commit` is false: every check runs, and nothing is kept. With
    /// `undid`, the batch is recorded as that batch's undo.
    /// `guard` may add drafts read from inside the write, given the UTC
    /// milliseconds the write is stamped with, so a Claim it judges lapsed or
    /// held is judged as the write will be.
    fn append_with(
        &mut self,
        drafts: Vec<(Draft, Option<Provenance>)>,
        guard: impl FnOnce(&Transaction<'_>, i64) -> Result<Vec<Draft>, StoreError>,
        commit: bool,
        undid: Option<Uuid>,
    ) -> Result<Vec<Op>, StoreError> {
        self.append_unnamed(drafts, guard, commit, undid)
            .map_err(|err| err.named(&self.conn))
    }

    fn append_unnamed(
        &mut self,
        mut drafts: Vec<(Draft, Option<Provenance>)>,
        guard: impl FnOnce(&Transaction<'_>, i64) -> Result<Vec<Draft>, StoreError>,
        commit: bool,
        undid: Option<Uuid>,
    ) -> Result<Vec<Op>, StoreError> {
        for (draft, _) in &drafts {
            draft.change.validate()?;
        }
        let now = self.now()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for draft in guard(&tx, now.0.utc_ms)? {
            draft.change.validate()?;
            drafts.push((draft, None));
        }
        let passed = claim::protected(&tx, &drafts, now.0.utc_ms, self.forcing)?;
        if let Some((issue, based_on)) = self.based_on {
            let fields = version::written(&drafts, issue);
            version::check(&tx, &self.scheme, (issue, based_on), fields)?;
        }
        if let Some(writer) = claim::renewing(&drafts) {
            let claimed = claim::claimed_in(&drafts);
            let due = |held: &Claim| held.due(now.0.utc_ms) && !claimed.contains(&held.issue);
            for draft in claim::renewals(&tx, &writer, due)? {
                draft.change.validate()?;
                drafts.push((draft, None));
            }
        }
        // A batch of several writers has no one undoer to try it as. An import
        // writes a batch per record and names each, so none is probed.
        let writer = drafts
            .first()
            .map(|(first, _)| first.actor.clone())
            .filter(|writer| {
                drafts
                    .iter()
                    .all(|(draft, provenance)| draft.actor == *writer && provenance.is_none())
            });
        let undoing = undid.is_some();
        let (ops, hlc) = write_drafts(&tx, &self.scheme, drafts, (now, undoing), last_hlc(&tx)?)?;
        if let (Some(undid), Some(first)) = (undid, ops.first()) {
            tx.execute(
                "INSERT INTO undo_batches (batch, undid) VALUES (?1, ?2)",
                [first.op_id.as_bytes(), undid.as_bytes()],
            )?;
        }
        let probed = match (commit, writer, ops.first()) {
            (true, Some(writer), Some(first)) => Some((
                first.op_id,
                undo::probe(&tx, &self.scheme, &writer, first.op_id, now, hlc)?,
            )),
            _ => None,
        };
        tx.execute(
            "UPDATE meta SET v = ?1 WHERE k = 'last_hlc'",
            [sql_int(hlc.raw())?],
        )?;
        if commit {
            tx.commit()?;
            // The write is committed; a mark that cannot be read only loses
            // the answer, never the write.
            self.last_write = probed.zip(self.write_mark().ok()).map(
                |((batch, undoable), (data_version, max_seq))| LastWrite {
                    batch,
                    undoable,
                    data_version,
                    max_seq,
                },
            );
        }
        self.passed.extend(passed);
        Ok(ops)
    }

    /// What changes when anyone writes: another connection's commit moves
    /// `data_version`, and this one's moves the log's last seq.
    fn write_mark(&self) -> Result<(i64, i64), StoreError> {
        let data_version = self
            .conn
            .query_row("PRAGMA data_version", [], |row| row.get(0))?;
        Ok((data_version, self.log_end()?))
    }

    /// The log's last position: being append-only, only a committed write
    /// moves it, never one rolled back.
    ///
    /// # Errors
    ///
    /// Any SQLite failure.
    pub fn log_end(&self) -> Result<i64, StoreError> {
        Ok(self
            .conn
            .query_row("SELECT coalesce(max(seq), 0) FROM ops", [], |row| {
                row.get(0)
            })?)
    }

    /// Refuses a log another build wrote with other columns: until v1 the
    /// format changes in place, and such a store is recreated (ADR 0010).
    fn check_log_columns(&self) -> Result<(), StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM pragma_table_info('ops') ORDER BY cid")?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if columns.is_empty() || columns == schema::OPS_COLUMNS {
            Ok(())
        } else {
            Err(StoreError::FormatChanged)
        }
    }

    /// Whether the store is already set up for this binary. Read without the
    /// write lock, so opening such a store never waits on a writer.
    fn is_set_up(&self) -> Result<bool, StoreError> {
        let has_meta: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'meta')",
            [],
            |row| row.get(0),
        )?;
        if !has_meta {
            return Ok(false);
        }
        let (format, set_up): (Option<i64>, bool) = self.conn.query_row(
            "SELECT (SELECT v FROM meta WHERE k = 'format'),
                    (SELECT count(*) FROM meta WHERE k IN ('replica_id', 'last_hlc')) = 2",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        Ok(format == Some(schema::STORE_FORMAT) && set_up && version == schema::PROJECTION_VERSION)
    }

    /// The clock's reading, and its milliseconds as the clock counts them.
    fn now(&self) -> Result<(WallTime, u64), StoreError> {
        let now = self.clock.now().map_err(StoreError::Clock)?;
        let now_ms = check_wall(now).map_err(StoreError::Clock)?;
        Ok((now, now_ms))
    }

    /// The time in UTC milliseconds, as Claims are judged lapsed by; it reads
    /// no time zone.
    ///
    /// # Errors
    ///
    /// [`StoreError::Clock`] when the clock cannot be read.
    pub fn utc_ms(&self) -> Result<i64, StoreError> {
        self.clock.utc_ms().map_err(StoreError::Clock)
    }

    /// The ids of Issues matching `filter`, in its `sort:` order, else most
    /// recently updated first. Archived Issues are left out unless the filter
    /// names `archived`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Filter`] when the filter does not parse, does not fit
    /// the Scheme or names a parent no Issue is, or any SQLite failure.
    pub fn find(&self, filter: &str, context: Context<'_>) -> Result<Vec<Uuid>, StoreError> {
        let mut query = parse(filter)?;
        let issues = self.issues_by_id(&mut query)?;
        let lists = self.shortlists_by_id(&mut query)?;
        // The one earlier in the filter text, as a reader meets it.
        let missing = issues.into_iter().chain(lists).min_by_key(|err| match err {
            FilterError::NoIssue { span, .. }
            | FilterError::NoShortlist { span, .. }
            | FilterError::ShortlistNameShared { span, .. } => span.0,
            _ => usize::MAX,
        });
        let resolved = resolve(&query, &self.scheme, context)?;
        // After resolving, so a filter that does not fit the Scheme is named first.
        if let Some(missing) = missing {
            return Err(missing.into());
        }
        let (sql, params) = query::compile(&resolved);
        let mut stmt = self.conn.prepare(&sql)?;
        let ids = stmt.query_map(rusqlite::params_from_iter(params), |row| {
            row.get::<_, Vec<u8>>(0)
        })?;
        ids.map(|id| uuid_from(&id?)).collect()
    }

    /// Rewrites each Issue a `parent:` term names as its id, returning the
    /// first that names none. An id children point at counts, since `tm show`
    /// prints it even when a merge left the parent out.
    fn issues_by_id(&self, query: &mut Query) -> Result<Option<FilterError>, StoreError> {
        let mut missing = None;
        for term in &mut query.terms {
            let Term::Match { field, values, .. } = term else {
                continue;
            };
            if !field.0.eq_ignore_ascii_case("parent") {
                continue;
            }
            for (value, span) in values {
                let FilterValue::Text(text) = value else {
                    continue;
                };
                let reference = text.trim();
                let found = if let Ok(id) = Uuid::parse_str(reference) {
                    let known = self.issue(id)?.is_some() || !self.children(id)?.is_empty();
                    known.then_some(id)
                } else if tm_core::project::parse_issue_key(&reference.to_ascii_uppercase())
                    .is_some()
                {
                    self.issue_by_key(reference)?
                } else {
                    continue;
                };
                if found.is_none() {
                    missing.get_or_insert(FilterError::NoIssue {
                        value: reference.to_owned(),
                        span: *span,
                    });
                }
                *text = found.unwrap_or_else(Uuid::nil).to_string();
            }
        }
        Ok(missing)
    }

    /// Rewrites each list a `shortlist:` term names as the ids of the live
    /// lists it names, returning the first that names none or is shared. An
    /// id names its list; a name matches without case in every Project, as
    /// the filter spans them, and one a merge left on two lists in a Project
    /// is refused until one is renamed (ADR 0016).
    fn shortlists_by_id(&self, query: &mut Query) -> Result<Option<FilterError>, StoreError> {
        let mut missing = None;
        for term in &mut query.terms {
            let Term::Match { field, values, .. } = term else {
                continue;
            };
            if !field.0.eq_ignore_ascii_case("shortlist") {
                continue;
            }
            let mut ids = Vec::with_capacity(values.len());
            for (value, span) in values.drain(..) {
                let FilterValue::Text(text) = &value else {
                    ids.push((value, span));
                    continue;
                };
                // Left for resolving to refuse as blank.
                if text.trim().is_empty() {
                    ids.push((value, span));
                    continue;
                }
                let value = text.trim().to_owned();
                let found = match shortlist::live_named(&self.conn, &value)? {
                    Ok(found) if found.is_empty() => {
                        missing.get_or_insert(FilterError::NoShortlist { value, span });
                        vec![Uuid::nil()]
                    }
                    Ok(found) => found,
                    Err(count) => {
                        let shared = FilterError::ShortlistNameShared { value, span, count };
                        missing.get_or_insert(shared);
                        vec![Uuid::nil()]
                    }
                };
                ids.extend(
                    found
                        .into_iter()
                        .map(|id| (FilterValue::Text(id.to_string()), span)),
                );
            }
            *values = ids;
        }
        Ok(missing)
    }

    /// Refuses `parent` for `issue` ahead of the write, which checks again.
    ///
    /// # Errors
    ///
    /// As setting it would: itself, or a descendant.
    pub fn check_parent(&self, issue: Uuid, parent: Uuid) -> Result<(), StoreError> {
        validate::parent_allowed(&self.conn, issue, parent).map_err(|err| err.named(&self.conn))
    }

    /// An Issue as it currently reads, labelled from the Scheme.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row;
    /// `Ok(None)` when the Issue does not exist.
    pub fn issue(&self, issue_id: Uuid) -> Result<Option<IssueView>, StoreError> {
        Ok(self.issues(&[issue_id])?.pop())
    }

    /// The Issues `ids` names, in that order, read together; an id with no
    /// Issue is left out.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn issues(&self, ids: &[Uuid]) -> Result<Vec<IssueView>, StoreError> {
        self.read(|store| read::issues(&store.conn, &store.scheme, ids))
    }

    /// The Links `issue` holds at either end, by type, outward before
    /// inward.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownIssue`], any SQLite failure, or
    /// [`StoreError::Corrupt`] for a malformed row.
    pub fn links(&self, issue: Uuid) -> Result<Vec<LinkView>, StoreError> {
        self.read(|store| {
            apply::issue_project(&store.conn, issue)?.ok_or(StoreError::UnknownIssue(issue))?;
            read::links(&store.conn, issue)
        })
    }

    /// The comments on `issue`, oldest first.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownIssue`], any SQLite failure, or
    /// [`StoreError::Corrupt`] for a malformed row.
    pub fn comments(&self, issue: Uuid) -> Result<Vec<CommentView>, StoreError> {
        self.read(|store| {
            apply::issue_project(&store.conn, issue)?.ok_or(StoreError::UnknownIssue(issue))?;
            read::comments(&store.conn, issue)
        })
    }

    /// Runs `reads` in one read transaction, so what they return never mixes
    /// two writers' commits. Nested calls share the outer transaction.
    ///
    /// # Errors
    ///
    /// What `reads` returns, or any SQLite failure starting the transaction.
    pub fn read<T, E: From<StoreError>>(
        &self,
        reads: impl FnOnce(&Self) -> Result<T, E>,
    ) -> Result<T, E> {
        let _tx = if self.conn.is_autocommit() {
            Some(
                self.conn
                    .unchecked_transaction()
                    .map_err(StoreError::from)?,
            )
        } else {
            None
        };
        reads(self)
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
        self.last_write = None;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let unapplied = rebuild_in(&tx)?;
        tx.commit()?;
        Ok(unapplied)
    }
}

/// [`Store::rebuild`] inside a transaction the caller commits.
fn rebuild_in(tx: &Transaction<'_>) -> Result<usize, StoreError> {
    tx.execute_batch(schema::DROP_PROJECTIONS)?;
    tx.execute_batch(schema::PROJECTIONS)?;
    let rows = {
        let mut stmt = tx.prepare(
            "SELECT seq, op_id, op_hash, v, replica_id, actor, project, issue_id, kind, hlc,
                    base_seq, utc_ms, offset_min, payload, server_seq, server_received_ms,
                    assigned_key, on_behalf_of, source
             FROM ops
             ORDER BY server_seq IS NULL, server_seq, hlc, replica_id, op_id",
        )?;
        let rows = stmt.query_map([], StoredOp::read)?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut unapplied = 0;
    for row in &rows {
        if !replay(tx, row)? {
            unapplied += 1;
        }
    }
    unapplied += record_dangling_links(tx)?;
    tx.execute_batch(&format!(
        "PRAGMA user_version = {}",
        schema::PROJECTION_VERSION
    ))?;
    Ok(unapplied)
}

/// A wall time's milliseconds, if an operation can hold it.
fn check_wall(wall: WallTime) -> Result<u64, String> {
    let ms = u64::try_from(wall.utc_ms)
        .ok()
        .filter(|ms| *ms <= MAX_MS)
        .ok_or_else(|| format!("{} ms is outside the supported range", wall.utc_ms))?;
    if wall.offset_min.unsigned_abs() > MAX_OFFSET_MIN {
        return Err(format!(
            "UTC offset of {} minutes is not a real offset",
            wall.offset_min
        ));
    }
    Ok(ms)
}

/// Checks, stamps, logs and applies `drafts` inside `tx`, after the clock's
/// `hlc`; returns the operations and the clock after them. An imported draft
/// keeps its source and its time there; its clock is still now.
fn write_drafts(
    tx: &Transaction<'_>,
    scheme: &Scheme,
    drafts: Vec<(Draft, Option<Provenance>)>,
    ((now, now_ms), undoing): ((WallTime, u64), bool),
    mut hlc: Hlc,
) -> Result<(Vec<Op>, Hlc), StoreError> {
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
    let mut half_held = BTreeSet::new();
    let mut was_finished = BTreeSet::new();
    let mut pending: VecDeque<_> = drafts.into();
    let mut cleared = false;
    loop {
        let Some((draft, provenance)) = pending.pop_front() else {
            // Whatever finished an Issue or deleted its list, membership
            // clears in the same batch (ADR 0016), once.
            if cleared {
                break;
            }
            cleared = true;
            pending.extend(shortlist::clearing(tx, scheme, &ops, &was_finished)?);
            continue;
        };
        if let Entry::Vacant(entry) = before.entry(draft.subject) {
            entry.insert((
                validate::missing_fields(tx, scheme, draft.subject)?,
                validate::status_fits_type(tx, scheme, draft.subject)?,
            ));
            if shortlist::half_held(tx, draft.subject)? {
                half_held.insert(draft.subject);
            }
            if shortlist::finished(tx, scheme, draft.subject)? {
                was_finished.insert(draft.subject);
            }
        }
        let (source, at) = provenance.map_or((None, now), |p| (Some(p.source), p.at));
        if matches!(draft.change, Change::ShortlistSource) && source.is_none() {
            return Err(StoreError::Invalid {
                field: "source".into(),
                reason: "a Short-list's source is written only by an import".into(),
                valid: Vec::new(),
            });
        }
        let writing = validate::Writing {
            actor: &draft.actor,
            at_ms: at.utc_ms,
            undoing,
        };
        let change = validate::check(tx, scheme, &writing, draft.subject, draft.change)?;
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
            source,
            project: draft.project,
            subject: draft.subject,
            kind: change.kind().to_owned(),
            hlc,
            base_seq: rust_int(base_seq)?,
            utc_ms: at.utc_ms,
            offset_min: at.offset_min,
            payload: serde_json::to_string(&change).map_err(OpError::from)?,
        };
        op.change()?;
        // Inside the write transaction, so concurrent creates never share a Key.
        let assigned_key = match change {
            Change::Create => Some(project::next_key(tx, op.project)?),
            _ => None,
        };
        insert_op(tx, &op, &change, assigned_key.as_deref())?;
        let batch = ops.first().map_or(op.op_id, |first| first.op_id);
        tx.execute(
            "INSERT INTO op_batches (op_id, batch) VALUES (?1, ?2)",
            [op.op_id.as_bytes(), batch.as_bytes()],
        )?;
        let effects = apply::Effects {
            assigned_key: assigned_key.as_deref(),
            ..apply::Effects::default()
        };
        if let Some(reason) = apply(tx, &op, effects)? {
            return Err(StoreError::Corrupt(format!(
                "a write this store checked did not apply in full: {reason}"
            )));
        }
        if let Some(key) = &assigned_key {
            project::count_key(tx, key, op.project, op.op_id)?;
        }
        ops.push(op);
    }
    no_worse(tx, scheme, before)?;
    shortlist::check_batch(tx, &ops, &half_held)?;
    Ok((ops, hlc))
}

/// Refuses a batch that left an Issue missing a field it had, or with a
/// Status outside its type's Workflow when it fitted before.
fn no_worse(
    tx: &Transaction<'_>,
    scheme: &Scheme,
    before: BTreeMap<Uuid, (Vec<String>, bool)>,
) -> Result<(), StoreError> {
    for (issue, (missing, fitted)) in before {
        let fields: Vec<String> = validate::missing_fields(tx, scheme, issue)?
            .into_iter()
            .filter(|field| !missing.contains(field))
            .collect();
        if !fields.is_empty() {
            return Err(StoreError::MissingFields {
                issue: issue.into(),
                fields,
            });
        }
        if fitted && !validate::status_fits_type(tx, scheme, issue)? {
            return Err(StoreError::Invalid {
                field: "status".into(),
                reason: "is not in the Workflow of the Issue's type".into(),
                valid: Vec::new(),
            });
        }
    }
    Ok(())
}

/// The clock's last value. The log bounds it as well as `last_hlc`, so a lost
/// or stale clock state never moves it back; only this Replica's operations
/// count, since others reach the clock solely through the capped `observe`.
fn last_hlc(tx: &Transaction<'_>) -> Result<Hlc, StoreError> {
    let last: i64 = tx.query_row(
        "SELECT max(
           coalesce((SELECT v FROM meta WHERE k = 'last_hlc'), 0),
           coalesce((SELECT max(hlc) FROM ops
                     WHERE replica_id = (SELECT v FROM meta WHERE k = 'replica_id')), 0))",
        [],
        |row| row.get(0),
    )?;
    Ok(Hlc::from_raw(rust_int(last)?))
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
            | StoreError::UnknownShortlist(_)
            | StoreError::DuplicateShortlist(_)
            | StoreError::ShortlistMismatch { .. }
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
        "INSERT INTO ops (op_id, op_hash, v, kind, replica_id, actor, on_behalf_of, source,
                          project, issue_id, field, elem, hlc, base_seq, utc_ms, offset_min,
                          payload, assigned_key)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                 ?18)",
        params![
            op.op_id.as_bytes(),
            op.op_hash(),
            op.v,
            op.kind,
            op.replica_id.as_bytes(),
            op.actor,
            op.on_behalf_of,
            op.source,
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
    source: Option<String>,
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
            source: row.get(18)?,
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
            source: self.source.clone(),
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
