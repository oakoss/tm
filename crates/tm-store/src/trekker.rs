//! The Trekker adapter: a `.trekker/trekker.db`'s epics and tasks as `issue`
//! records (ADR 0013), read without writing to it. Trekker has no export, so
//! the database is the source.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use rusqlite::{Connection, ErrorCode, OpenFlags, Row};
use serde::Serialize;
use tm_core::{Actor, entities, link};

use crate::beads::add_label;
use crate::import::Parsed;
use crate::record::{CommentRecord, IssueRecord, LinkRecord};
use crate::{RecordsImported, Store, StoreError};

const SOURCE: &str = "trekker:";

/// What a Trekker import did, and what it could not keep.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrekkerImported {
    pub imported: RecordsImported,
    /// Each thing tm has no place for, such as `rank`, with how many times
    /// the database holds it.
    pub dropped: Dropped,
}

type Dropped = BTreeMap<String, usize>;

/// A Trekker database read for Project `project`, nothing written yet.
#[derive(Debug)]
pub struct TrekkerExport {
    issues: Vec<(usize, IssueRecord)>,
    dropped: Dropped,
}

/// Reads a Trekker database's epics and tasks as records for Project
/// `project`, read-only and in one read transaction, so a running
/// Trekker's writes land wholly before or after.
///
/// # Errors
///
/// [`StoreError::InvalidExport`] for a file that is not a Trekker database,
/// one that is locked or in WAL mode, one holding several Trekker projects,
/// or a row tm cannot read; its `line` is the row's place in its table.
pub fn read_trekker(path: &Path, project: &str) -> Result<TrekkerExport, StoreError> {
    // Trekker never turns WAL on, and even a read-only open of a WAL database
    // writes its -wal and -shm beside it without locking a writer out.
    if wal(path)? {
        return Err(StoreError::InvalidExport {
            line: 0,
            reason: format!(
                "it is in WAL mode, which Trekker does not use; with Trekker closed, run sqlite3 '{}' 'PRAGMA journal_mode=DELETE' and import again",
                path.display().to_string().replace('\'', r"'\''")
            ),
        });
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|err| failed(&err, "cannot open it as a Trekker database"))?;
    read_all(&conn, project)
}

/// Whether the file's header says WAL: bytes 18 and 19 of a SQLite file
/// are 2 in WAL mode and 1 otherwise. One too short to be SQLite is left
/// for the open to refuse.
fn wal(path: &Path) -> Result<bool, StoreError> {
    use std::io::Read as _;
    let mut header = [0; 20];
    let mut file = std::fs::File::open(path).map_err(|err| StoreError::InvalidExport {
        line: 0,
        reason: format!("cannot open it as a Trekker database: {err}"),
    })?;
    Ok(file.read_exact(&mut header).is_ok() && (header[18] == 2 || header[19] == 2))
}

impl Store {
    /// Writes a Trekker database's epics and tasks into Project `project` as
    /// records, so a re-import compares three ways like any other.
    ///
    /// # Errors
    ///
    /// As [`read_trekker`], before anything is written; otherwise as
    /// [`Store::import_records`].
    pub fn import_trekker(
        &mut self,
        path: &Path,
        project: &str,
        runner: &Actor,
    ) -> Result<TrekkerImported, StoreError> {
        let export = read_trekker(path, project)?;
        let imported = self.write_records(&Parsed::issues(export.issues), runner)?;
        Ok(TrekkerImported {
            imported,
            dropped: export.dropped,
        })
    }
}

// The last column says the row is an epic, which has no tags or parents.
const EPICS: (&str, &str) = (
    "epics",
    "SELECT id, title, description, status, priority, created_at, updated_at,
       rank, NULL, NULL, NULL, 1 FROM epics ORDER BY created_at, id",
);
const TASKS: (&str, &str) = (
    "tasks",
    "SELECT id, title, description, status, priority, created_at, updated_at,
       rank, tags, parent_task_id, epic_id, 0 FROM tasks ORDER BY created_at, id",
);
const COMMENTS: (&str, &str) = (
    "comments",
    "SELECT id, task_id, author, content, created_at, updated_at FROM comments
     ORDER BY created_at, id",
);
const DEPENDENCIES: (&str, &str) = (
    "dependencies",
    "SELECT id, task_id, depends_on_id FROM dependencies ORDER BY id",
);

fn read_all(conn: &Connection, project: &str) -> Result<TrekkerExport, StoreError> {
    conn.execute_batch("BEGIN")
        .map_err(|err| failed(&err, "cannot read it"))?;
    let projects = count(conn, "projects")?;
    if projects > 1 {
        return Err(StoreError::InvalidExport {
            line: 0,
            reason: format!("it holds {projects} Trekker projects, and tm imports one"),
        });
    }
    let mut items = rows(conn, EPICS, item)?;
    items.extend(rows(conn, TASKS, item)?);
    let known: HashMap<String, Option<String>> = items
        .iter()
        .map(|(_, item)| (item.id.clone(), item.epic_id.clone()))
        .collect();
    let mut dropped = Dropped::new();
    let mut issues = Vec::new();
    for (line, item) in items {
        issues.push((line, item.record(project, &known, &mut dropped)?));
    }
    attach_comments(&mut issues, rows(conn, COMMENTS, comment)?, &mut dropped)?;
    let edges = rows(conn, DEPENDENCIES, |row| Ok((row.get(1)?, row.get(2)?)))?;
    place(&mut issues, edges, &mut dropped);
    let events = count(conn, "events")?;
    if events > 0 {
        dropped.insert("events".into(), events);
    }
    issues.sort_by_cached_key(|(_, record)| creation_order(record));
    Ok(TrekkerExport { issues, dropped })
}

/// An error reading the database: a lock says so, since a running Trekker
/// holds one; anything else is `what` went wrong.
fn failed(err: &rusqlite::Error, what: &str) -> StoreError {
    let reason = match err.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => {
            "it is locked, likely by a running Trekker; close it and import again".to_owned()
        }
        _ => format!("{what}: {err}"),
    };
    StoreError::InvalidExport { line: 0, reason }
}

/// The rows of `table`, none when this Trekker has no such table.
fn count(conn: &Connection, table: &str) -> Result<usize, StoreError> {
    match conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get::<_, i64>(0)
    }) {
        Ok(rows) => Ok(usize::try_from(rows).unwrap_or(0)),
        Err(rusqlite::Error::SqliteFailure(_, Some(message)))
            if message.starts_with("no such table") =>
        {
            Ok(0)
        }
        Err(err) => Err(failed(&err, &format!("cannot count its {table}"))),
    }
}

/// An epic or task as its table holds it.
struct Item {
    id: String,
    title: String,
    description: Option<String>,
    status: String,
    priority: i64,
    created: i64,
    updated: i64,
    rank: Option<String>,
    tags: Option<String>,
    parent_task: Option<String>,
    epic_id: Option<String>,
    epic: bool,
}

fn item(row: &Row<'_>) -> rusqlite::Result<Item> {
    Ok(Item {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        status: row.get(3)?,
        priority: row.get(4)?,
        created: row.get(5)?,
        updated: row.get(6)?,
        rank: row.get(7)?,
        tags: row.get(8)?,
        parent_task: row.get(9)?,
        epic_id: row.get(10)?,
        epic: row.get(11)?,
    })
}

/// A Trekker comment, by the task it is on.
struct Said {
    id: String,
    task: String,
    author: String,
    body: String,
    at: i64,
    edited: i64,
}

fn comment(row: &Row<'_>) -> rusqlite::Result<Said> {
    Ok(Said {
        id: row.get(0)?,
        task: row.get(1)?,
        author: row.get(2)?,
        body: row.get(3)?,
        at: row.get(4)?,
        edited: row.get(5)?,
    })
}

/// Every row of `table` that `sql` selects, read by `read`, each with its
/// place in the table. A missing table or column says the file is not a
/// Trekker database this adapter knows; a row it cannot read is named by its
/// id, the first column.
fn rows<T>(
    conn: &Connection,
    (table, sql): (&str, &str),
    mut read: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<(usize, T)>, StoreError> {
    let unknown = |err| failed(&err, "not a Trekker database tm reads");
    let mut stmt = conn.prepare(sql).map_err(unknown)?;
    let mut found = stmt.query([]).map_err(unknown)?;
    let mut all = Vec::new();
    while let Some(row) = found
        .next()
        .map_err(|err| failed(&err, &format!("cannot read its {table}")))?
    {
        let line = all.len() + 1;
        let value = read(row).map_err(|err| {
            let id: String = row.get(0).unwrap_or_default();
            StoreError::InvalidExport {
                line,
                reason: format!("{table} row {id}: {err}"),
            }
        })?;
        all.push((line, value));
    }
    Ok(all)
}

impl Item {
    fn record(
        self,
        project: &str,
        known: &HashMap<String, Option<String>>,
        dropped: &mut Dropped,
    ) -> Result<IssueRecord, StoreError> {
        let mut labels = Vec::new();
        for tag in self.tags.iter().flat_map(|tags| tags.split(',')) {
            if !tag.trim().is_empty() {
                add_label(&mut labels, tag.trim().to_owned());
            }
        }
        // A parent task not imported leaves its epic as the parent.
        let parent_task = self.parent_task.filter(|parent| {
            let found = known.contains_key(parent);
            if !found {
                note(dropped, "parent task not imported");
            }
            found
        });
        let epic = self.epic_id.filter(|epic| {
            let found = known.contains_key(epic);
            if !found {
                note(dropped, "epic not imported");
            }
            found
        });
        // A sub_task's parent is its task, so an epic other than the task's is lost.
        if let Some(parent) = &parent_task
            && epic.is_some()
            && known.get(parent).is_some_and(|theirs| *theirs != epic)
        {
            note(dropped, "sub-task's epic other than its task's");
        }
        let has = |tag: &str| labels.iter().any(|label| label.to_lowercase() == tag);
        // A sub_task stays one whatever its tags, and bug wins over spike.
        let issue_type = if self.epic {
            "epic"
        } else if parent_task.is_some() {
            "sub_task"
        } else if has("bug") {
            "bug"
        } else if has("spike") {
            "spike"
        } else {
            "task"
        };
        let status = tm_status(&self.status).unwrap_or_else(|| {
            add_label(&mut labels, format!("trekker-status:{}", self.status));
            "todo"
        });
        let priority = tm_priority(self.priority);
        if priority.is_none() {
            note(dropped, "priority");
        }
        if self.rank.as_deref().is_some_and(|rank| !rank.is_empty()) {
            note(dropped, "rank");
        }
        let source = format!("{SOURCE}{}", self.id);
        let time = |seconds, field| {
            rfc3339(seconds).map_err(|reason| StoreError::InvalidExport {
                line: 0,
                reason: format!("{source} {field}: {reason}"),
            })
        };
        let (created, updated) = (
            time(self.created, "created_at")?,
            time(self.updated, "updated_at")?,
        );
        Ok(IssueRecord {
            project: project.to_owned(),
            key: None,
            issue_type: Some(issue_type.to_owned()),
            title: Some(entities::decoded(&self.title).into_owned()),
            description: self
                .description
                .filter(|text| !text.is_empty())
                .map(|text| entities::decoded(&text).into_owned()),
            status: Some(status.to_owned()),
            priority: priority.map(str::to_owned),
            assignee: None,
            reporter: None,
            updated_by: None,
            labels,
            archived: false,
            parent: parent_task
                .or(epic)
                .map(|parent| format!("{SOURCE}{parent}")),
            links: Vec::new(),
            comments: Vec::new(),
            fields: BTreeMap::new(),
            shortlist: None,
            created: Some(created),
            updated: Some(updated),
            source,
        })
    }
}

fn note(dropped: &mut Dropped, what: &str) {
    *dropped.entry(what.to_owned()).or_default() += 1;
}

/// Trekker's Statuses as the default Workflow's.
fn tm_status(status: &str) -> Option<&'static str> {
    match status {
        "todo" => Some("todo"),
        "in_progress" => Some("in_progress"),
        "completed" => Some("done"),
        "wont_fix" => Some("cancelled"),
        _ => None,
    }
}

/// Trekker's 0 (critical) to 5; 4 and 5 are both tm's lowest.
fn tm_priority(level: i64) -> Option<&'static str> {
    ["urgent", "high", "medium", "low", "none", "none"]
        .get(usize::try_from(level).ok()?)
        .copied()
}

/// Unix seconds, as Trekker stores them, in RFC 3339.
fn rfc3339(seconds: i64) -> Result<String, String> {
    jiff::Timestamp::from_second(seconds)
        .map(|at| at.to_string())
        .map_err(|_| format!("{seconds} is out of range as Unix seconds; is it milliseconds?"))
}

/// Files each comment on its task's record, keeping its Trekker id, which
/// gives it the source `trekker:TREK-n#CMT-n` a lookup finds it by.
fn attach_comments(
    issues: &mut [(usize, IssueRecord)],
    comments: Vec<(usize, Said)>,
    dropped: &mut Dropped,
) -> Result<(), StoreError> {
    let index = index(issues);
    for (line, said) in comments {
        let invalid = |reason: String| StoreError::InvalidExport { line, reason };
        let Some(&at) = index.get(&format!("{SOURCE}{}", said.task)) else {
            note(dropped, "comment on a task not imported");
            continue;
        };
        if said.edited != said.at {
            note(dropped, "comment edit time");
        }
        let comments = &mut issues[at].1.comments;
        if comments.iter().any(|held| held.id == said.id) {
            return Err(invalid(format!("comment {} appears twice", said.id)));
        }
        let at =
            rfc3339(said.at).map_err(|reason| invalid(format!("comment {}: {reason}", said.id)))?;
        comments.push(CommentRecord {
            id: said.id,
            actor: said.author,
            on_behalf_of: None,
            body: said.body,
            at: Some(at),
        });
    }
    Ok(())
}

/// Each dependency, "`task` depends on `on`", as a `blocks` Link filed on
/// `on`, its outward end.
fn place(
    issues: &mut [(usize, IssueRecord)],
    edges: Vec<(usize, (String, String))>,
    dropped: &mut Dropped,
) {
    let index = index(issues);
    for (_, (task, on)) in edges {
        let (task, on) = (format!("{SOURCE}{task}"), format!("{SOURCE}{on}"));
        let (Some(_), Some(&on_at)) = (index.get(&task), index.get(&on)) else {
            note(dropped, "dependency on a task not imported");
            continue;
        };
        if task == on {
            note(dropped, "dependency on itself");
            continue;
        }
        let link = LinkRecord {
            link_type: link::BLOCKS.to_owned(),
            target: task,
        };
        let links = &mut issues[on_at].1.links;
        if !links.contains(&link) {
            links.push(link);
        }
    }
}

fn index(issues: &[(usize, IssueRecord)]) -> HashMap<String, usize> {
    issues
        .iter()
        .enumerate()
        .map(|(at, (_, record))| (record.source.clone(), at))
        .collect()
}

/// Oldest first, so Keys follow creation; an epic comes before a task made
/// in the same second, and the id's number breaks a tie.
fn creation_order(record: &IssueRecord) -> (Option<String>, bool, u64) {
    let number = record
        .source
        .rsplit_once('-')
        .and_then(|(_, number)| number.parse().ok())
        .unwrap_or(0);
    let task = record.issue_type.as_deref() != Some("epic");
    (record.created.clone(), task, number)
}
