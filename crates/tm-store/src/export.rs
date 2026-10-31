//! The log as JSONL, and its restore (ADR 0013): a header, every Project,
//! every operation verbatim in replay order, then a record per Issue as the
//! Scheme reads it, so one log under one Scheme always exports the same bytes
//! and each operation's hash still verifies. Restore reads only the
//! operations.

use std::collections::HashSet;
use std::io::{BufRead, Write};

use rusqlite::{Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use tm_core::op::CURRENT_VERSION;
use tm_core::{Hlc, Op, ProjectId};
use uuid::Uuid;

use crate::apply::{project_from, rust_int, sql_int, uuid_from};
use crate::record::{IssueRecord, ListRecord, MemoryRecord, issue_records};
use crate::{Store, StoreError, rebuild_in};

pub(crate) const FORMAT: &str = "tm-export";
pub(crate) const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "_type", rename_all = "snake_case")]
enum Record {
    Header(Header),
    Project(ProjectRecord),
    Op(Box<OpRecord>),
    Issue(Box<IssueRecord>),
}

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    format: String,
    version: u32,
    op_format: u16,
    workspace: Uuid,
    exported_ms: i64,
}

/// For a reader and a record import; restore takes Projects from their
/// operations.
#[derive(Debug, Serialize, Deserialize)]
struct ProjectRecord {
    id: ProjectId,
    key: String,
    keys: Vec<String>,
    name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    shortlists: Vec<ListRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    memories: Vec<MemoryRecord>,
}

/// Every logged column but the local `seq`.
#[derive(Debug, Serialize, Deserialize)]
struct OpRecord {
    op_id: Uuid,
    op_hash: String,
    v: u16,
    kind: String,
    replica_id: Uuid,
    actor: String,
    on_behalf_of: Option<String>,
    source: Option<String>,
    project: ProjectId,
    issue_id: Uuid,
    field: Option<String>,
    elem: Option<String>,
    hlc: u64,
    base_seq: u64,
    utc_ms: i64,
    offset_min: i16,
    payload: String,
    server_seq: Option<i64>,
    server_received_ms: Option<i64>,
    assigned_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<String>,
}

impl OpRecord {
    fn op(&self) -> Op {
        Op {
            v: self.v,
            op_id: self.op_id,
            replica_id: self.replica_id,
            actor: self.actor.clone(),
            on_behalf_of: self.on_behalf_of.clone(),
            source: self.source.clone(),
            project: self.project,
            subject: self.issue_id,
            kind: self.kind.clone(),
            hlc: Hlc::from_raw(self.hlc),
            base_seq: self.base_seq,
            utc_ms: self.utc_ms,
            offset_min: self.offset_min,
            payload: self.payload.clone(),
            session: self.session.clone(),
        }
    }
}

/// What an export wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exported {
    pub projects: usize,
    pub ops: usize,
    pub issues: usize,
}

/// What a restore did: wrote the log, or found it already here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Restored {
    pub workspace: Uuid,
    pub ops: usize,
    pub already_held: bool,
    pub unapplied: usize,
}

impl Store {
    /// Writes the log to `out` as JSONL, read in one read transaction.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, [`StoreError::Corrupt`] for a malformed row, or an
    /// I/O failure writing `out` as [`StoreError::Io`].
    pub fn export(&self, out: &mut dyn Write, exported_ms: i64) -> Result<Exported, StoreError> {
        self.read(|store| {
            let workspace = store
                .workspace_id()?
                .ok_or_else(|| StoreError::Corrupt("the store records no Workspace".into()))?;
            let header = Record::Header(Header {
                format: FORMAT.into(),
                version: VERSION,
                op_format: CURRENT_VERSION,
                workspace,
                exported_ms,
            });
            write_line(out, &header)?;
            let projects = projects(&store.conn)?;
            for project in &projects {
                write_line(out, project)?;
            }
            let mut stmt = store.conn.prepare(
                "SELECT op_id, op_hash, v, kind, replica_id, actor, on_behalf_of, source,
                        project, issue_id, field, elem, hlc, base_seq, utc_ms, offset_min,
                        payload, server_seq, server_received_ms, assigned_key, session
                 FROM ops
                 ORDER BY server_seq IS NULL, server_seq, hlc, replica_id, op_id",
            )?;
            let mut rows = stmt.query([])?;
            let mut ops = 0;
            while let Some(row) = rows.next()? {
                write_line(out, &Record::Op(Box::new(op_record(row)?)))?;
                ops += 1;
            }
            let issues = issue_records(&store.conn, &store.scheme)?;
            let count = issues.len();
            for issue in issues {
                write_line(out, &Record::Issue(Box::new(issue)))?;
            }
            Ok(Exported {
                projects: projects.len(),
                ops,
                issues: count,
            })
        })
    }

    /// Restores an export into this store: into one with no operations, or
    /// one already holding exactly them, which writes nothing. Operations are
    /// inserted verbatim and hash-checked, the projections rebuilt, and the
    /// clock advanced past them under the drift cap, in one transaction.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvalidExport`] for a file that is not a tm export or an
    /// operation whose hash does not match, [`StoreError::ExportFormat`] for
    /// one of another operation format, [`StoreError::WorkspaceMismatch`] when
    /// it holds another Workspace, [`StoreError::WorkspaceNotEmpty`] when this
    /// store holds other operations, or as [`Store::rebuild`].
    pub fn restore(&mut self, input: &mut dyn BufRead) -> Result<Restored, StoreError> {
        let (header, ops) = parse(input)?;
        let workspace = self
            .workspace_id()?
            .ok_or_else(|| StoreError::Corrupt("the store records no Workspace".into()))?;
        if header.workspace != workspace {
            return Err(StoreError::WorkspaceMismatch {
                stored: workspace,
                given: header.workspace,
            });
        }
        let (_, now_ms) = self.now()?;
        self.last_write = None;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let held: i64 = tx.query_row("SELECT count(*) FROM ops", [], |row| row.get(0))?;
        if held > 0 {
            return if holds_exactly(&tx, &ops, held)? {
                Ok(Restored {
                    workspace,
                    ops: ops.len(),
                    already_held: true,
                    unapplied: 0,
                })
            } else {
                Err(StoreError::WorkspaceNotEmpty { workspace, held })
            };
        }
        for (line, record) in &ops {
            insert(&tx, record).map_err(|err| match err {
                StoreError::Sqlite(err) => StoreError::InvalidExport {
                    line: *line,
                    reason: err.to_string(),
                },
                other => other,
            })?;
        }
        let unapplied = rebuild_in(&tx)?;
        let newest = ops
            .iter()
            .map(|(_, record)| Hlc::from_raw(record.hlc))
            .max()
            .unwrap_or(Hlc::ZERO);
        let clock = crate::last_hlc(&tx)?.observe(newest, now_ms);
        tx.execute(
            "UPDATE meta SET v = ?1 WHERE k = 'last_hlc'",
            [sql_int(clock.raw())?],
        )?;
        tx.commit()?;
        Ok(Restored {
            workspace,
            ops: ops.len(),
            already_held: false,
            unapplied,
        })
    }
}

/// The Workspace an export holds and its operation count, once the file
/// passes restore's own checks, so a caller can find or make the store first.
///
/// # Errors
///
/// [`StoreError::InvalidExport`] or [`StoreError::ExportFormat`], as
/// [`Store::restore`] would refuse the file.
pub fn read_export(input: &mut dyn BufRead) -> Result<(Uuid, usize), StoreError> {
    parse(input).map(|(header, ops)| (header.workspace, ops.len()))
}

/// Copies the store at `from` into a new file `to` in one locked read, which
/// no writer can tear; like any reader it may leave SQLite's lock files.
///
/// # Errors
///
/// Any SQLite failure reading `from` or writing `to`.
pub fn copy_store(from: &std::path::Path, to: &std::path::Path) -> Result<(), StoreError> {
    let conn = rusqlite::Connection::open_with_flags(
        from,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.execute("VACUUM INTO ?1", [to.to_string_lossy()])?;
    Ok(())
}

fn write_line(out: &mut dyn Write, record: &Record) -> Result<(), StoreError> {
    let mut line = serde_json::to_vec(record)
        .map_err(|err| StoreError::Corrupt(format!("an export record: {err}")))?;
    line.push(b'\n');
    out.write_all(&line).map_err(StoreError::Io)
}

fn projects(conn: &rusqlite::Connection) -> Result<Vec<Record>, StoreError> {
    let mut stmt = conn.prepare("SELECT id, key, name FROM projects ORDER BY id")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut keys = conn.prepare("SELECT key FROM project_keys WHERE project = ?1 ORDER BY key")?;
    rows.into_iter()
        .map(|(id, key, name)| {
            let held = keys
                .query_map([&id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let shortlists = crate::record::lists(conn, &id)?;
            let memories = crate::record::memories(conn, &id)?;
            Ok(Record::Project(ProjectRecord {
                id: project_from(&id)?,
                key,
                keys: held,
                name,
                shortlists,
                memories,
            }))
        })
        .collect()
}

fn op_record(row: &rusqlite::Row<'_>) -> Result<OpRecord, StoreError> {
    Ok(OpRecord {
        op_id: uuid_from(&row.get::<_, Vec<u8>>(0)?)?,
        op_hash: hex(&row.get::<_, Vec<u8>>(1)?),
        v: u16::try_from(row.get::<_, i64>(2)?)
            .map_err(|_| StoreError::Corrupt("an operation version is out of range".into()))?,
        kind: row.get(3)?,
        replica_id: uuid_from(&row.get::<_, Vec<u8>>(4)?)?,
        actor: row.get(5)?,
        on_behalf_of: row.get(6)?,
        source: row.get(7)?,
        project: project_from(&row.get::<_, String>(8)?)?,
        issue_id: uuid_from(&row.get::<_, Vec<u8>>(9)?)?,
        field: row.get(10)?,
        elem: row.get(11)?,
        hlc: rust_int(row.get(12)?)?,
        base_seq: rust_int(row.get(13)?)?,
        utc_ms: row.get(14)?,
        offset_min: i16::try_from(row.get::<_, i64>(15)?)
            .map_err(|_| StoreError::Corrupt("a UTC offset is out of range".into()))?,
        payload: row.get(16)?,
        server_seq: row.get(17)?,
        server_received_ms: row.get(18)?,
        assigned_key: row.get(19)?,
        session: row.get(20)?,
    })
}

/// The header and the operations, each with its line number; Project and
/// Issue lines are checked as records and otherwise skipped.
fn parse(input: &mut dyn BufRead) -> Result<(Header, Vec<(usize, OpRecord)>), StoreError> {
    let invalid = |line: usize, reason: String| StoreError::InvalidExport { line, reason };
    let mut header = None;
    let mut ops = Vec::new();
    let mut seen = HashSet::new();
    for (index, text) in input.lines().enumerate() {
        let line = index + 1;
        let text = text.map_err(StoreError::Io)?;
        let record: Record =
            serde_json::from_str(&text).map_err(|err| invalid(line, unreadable(&err)))?;
        match (record, &header) {
            (Record::Header(found), None) if line == 1 => {
                if found.format != FORMAT || found.version != VERSION {
                    return Err(invalid(
                        line,
                        format!(
                            "{} version {} is not a {FORMAT} version {VERSION} file",
                            found.format, found.version
                        ),
                    ));
                }
                if found.op_format != CURRENT_VERSION {
                    return Err(StoreError::ExportFormat {
                        found: found.op_format,
                        supported: CURRENT_VERSION,
                    });
                }
                header = Some(found);
            }
            (_, None) => return Err(invalid(line, "the first line is not a header".into())),
            (Record::Header(_), Some(_)) => {
                return Err(invalid(line, "a second header".into()));
            }
            (Record::Project(_) | Record::Issue(_), Some(_)) => {}
            (Record::Op(record), Some(_)) => {
                check_op(&record, &mut seen).map_err(|reason| invalid(line, reason))?;
                ops.push((line, *record));
            }
        }
    }
    let header = header.ok_or_else(|| invalid(0, "the file is empty".into()))?;
    Ok((header, ops))
}

/// Why a line is not a record, placed by column only, since serde counts
/// lines within the one line it was given.
pub(crate) fn unreadable(err: &serde_json::Error) -> String {
    let what = match err.classify() {
        serde_json::error::Category::Eof => "the line ends early".to_owned(),
        serde_json::error::Category::Syntax => "the line is not JSON".to_owned(),
        serde_json::error::Category::Data | serde_json::error::Category::Io => {
            let text = err.to_string();
            let reason = text
                .rsplit_once(" at line ")
                .map_or(text.as_str(), |(reason, _)| reason);
            format!("not a tm export record: {reason}")
        }
    };
    format!("{what}, at column {}", err.column())
}

/// An operation line as restore trusts it: its hash fits its content, its id
/// is not repeated, its time is one a write may record, its payload parses,
/// and its `field` and `elem` columns, which the hash does not cover and undo
/// reads, are what the payload names.
fn check_op(record: &OpRecord, seen: &mut HashSet<Uuid>) -> Result<(), String> {
    if hex(&record.op().op_hash()) != record.op_hash {
        return Err(format!(
            "operation {} does not match its hash",
            record.op_id
        ));
    }
    if !seen.insert(record.op_id) {
        return Err(format!("operation {} appears twice", record.op_id));
    }
    // Held to what a write may record, so the restored store can export it.
    crate::check_wall(crate::WallTime {
        utc_ms: record.utc_ms,
        offset_min: record.offset_min,
    })
    .map_err(|reason| format!("operation {}'s time: {reason}", record.op_id))?;
    let change = record
        .op()
        .change()
        .map_err(|err| format!("operation {} does not parse: {err}", record.op_id))?;
    let elem = change.elem();
    if change.field() != record.field.as_deref() || elem.as_deref() != record.elem.as_deref() {
        return Err(format!(
            "operation {}'s field and element do not match its payload",
            record.op_id
        ));
    }
    Ok(())
}

/// Whether the store holds exactly `ops`, each with the same hash.
fn holds_exactly(
    tx: &Transaction<'_>,
    ops: &[(usize, OpRecord)],
    held: i64,
) -> Result<bool, StoreError> {
    if usize::try_from(held).ok() != Some(ops.len()) {
        return Ok(false);
    }
    let mut stmt = tx.prepare_cached("SELECT op_hash FROM ops WHERE op_id = ?1")?;
    for (_, record) in ops {
        let hash: Option<Vec<u8>> = stmt
            .query_row([record.op_id.as_bytes()], |row| row.get(0))
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                err => Err(err),
            })?;
        if hash.as_deref().map(hex) != Some(record.op_hash.clone()) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn insert(tx: &Transaction<'_>, record: &OpRecord) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO ops (op_id, op_hash, v, kind, replica_id, actor, on_behalf_of, source,
                          project, issue_id, field, elem, hlc, base_seq, utc_ms, offset_min,
                          payload, server_seq, server_received_ms, assigned_key, session)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                 ?18, ?19, ?20, ?21)",
        params![
            record.op_id.as_bytes(),
            record.op().op_hash(),
            record.v,
            record.kind,
            record.replica_id.as_bytes(),
            record.actor,
            record.on_behalf_of,
            record.source,
            record.project.to_string(),
            record.issue_id.as_bytes(),
            record.field,
            record.elem,
            sql_int(record.hlc)?,
            sql_int(record.base_seq)?,
            record.utc_ms,
            record.offset_min,
            record.payload,
            record.server_seq,
            record.server_received_ms,
            record.assigned_key,
            record.session,
        ],
    )?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}
