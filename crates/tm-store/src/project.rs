//! Projects and the Keys their Issues are given (ADR 0008).

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use tm_core::project::parse_issue_key;
use tm_core::{Change, Op, ProjectId};
use uuid::Uuid;

use crate::StoreError;
use crate::apply::project_from;

pub(crate) fn apply(tx: &Transaction<'_>, op: &Op, change: Change) -> Result<(), StoreError> {
    if op.subject != op.project.uuid() {
        return Err(StoreError::SubjectNotProject {
            subject: op.subject,
            project: op.project,
        });
    }
    match change {
        Change::CreateProject { key, name } => {
            if exists(tx, op.project)? {
                return Err(StoreError::DuplicateProject(op.project));
            }
            reserve(tx, &key, op.project, |tx| {
                tx.execute(
                    "INSERT INTO projects (id, key, name, created_ms) VALUES (?1, ?2, ?3, ?4)",
                    params![op.project.to_string(), key, name, op.utc_ms],
                )?;
                Ok(())
            })
        }
        Change::RenameProject { key, name } => {
            if !exists(tx, op.project)? {
                return Err(StoreError::UnknownProject(op.project));
            }
            if let Some(key) = key {
                reserve(tx, &key, op.project, |tx| {
                    tx.execute(
                        "UPDATE projects SET key = ?1 WHERE id = ?2",
                        params![key, op.project.to_string()],
                    )?;
                    Ok(())
                })?;
            }
            if let Some(name) = name {
                tx.execute(
                    "UPDATE projects SET name = ?1 WHERE id = ?2",
                    params![name, op.project.to_string()],
                )?;
            }
            Ok(())
        }
        _ => unreachable!("only Project kinds reach here"),
    }
}

/// Runs `write`, then records `key` as held by `project`. A key another
/// Project has ever held is refused, so an old Key never changes meaning.
fn reserve(
    tx: &Transaction<'_>,
    key: &str,
    project: ProjectId,
    write: impl FnOnce(&Transaction<'_>) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    if let Some(holder) = key_holder(tx, key)?
        && holder != project
    {
        return Err(StoreError::KeyTaken {
            key: key.to_owned(),
            project: holder,
        });
    }
    write(tx)?;
    tx.execute(
        "INSERT OR IGNORE INTO project_keys (key, project) VALUES (?1, ?2)",
        params![key, project.to_string()],
    )?;
    Ok(())
}

pub(crate) fn exists(conn: &Connection, project: ProjectId) -> Result<bool, StoreError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM projects WHERE id = ?1",
            [project.to_string()],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// The Project that holds or once held `key`.
pub(crate) fn key_holder(conn: &Connection, key: &str) -> Result<Option<ProjectId>, StoreError> {
    let holder: Option<String> = conn
        .query_row(
            "SELECT project FROM project_keys WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()?;
    holder.map(|text| project_from(&text)).transpose()
}

/// The next Key for an Issue in `project`: its current key and one past the
/// highest number counted for it.
pub(crate) fn next_key(tx: &Transaction<'_>, project: ProjectId) -> Result<String, StoreError> {
    let (key, counted): (String, Option<(i64, Vec<u8>)>) = tx
        .query_row(
            "SELECT p.key, n.highest, n.op_id
             FROM projects p LEFT JOIN key_numbers n ON n.project = p.id
             WHERE p.id = ?1",
            [project.to_string()],
            |row| {
                let highest: Option<i64> = row.get(1)?;
                let op_id: Option<Vec<u8>> = row.get(2)?;
                Ok((row.get(0)?, highest.zip(op_id)))
            },
        )
        .optional()?
        .ok_or(StoreError::UnknownProject(project))?;
    let Some((highest, op_id)) = counted else {
        return Ok(format!("{key}-1"));
    };
    if let Some(next) = highest.checked_add(1) {
        return Ok(format!("{key}-{next}"));
    }
    let op_id = Uuid::from_slice(&op_id)
        .map_err(|_| StoreError::Corrupt(format!("key_numbers names no operation for {key}")))?;
    Err(StoreError::KeysExhausted { key, op_id })
}

/// Counts `key`'s number as given in `project`, whether or not its create
/// applied, so a number is never handed out twice. A Project key another
/// Project holds never named anything here and is not counted.
pub(crate) fn count_key(
    tx: &Transaction<'_>,
    key: &str,
    project: ProjectId,
    op_id: Uuid,
) -> Result<(), StoreError> {
    let Some((prefix, number)) = parse_issue_key(key) else {
        return Ok(());
    };
    if key_holder(tx, prefix)?.is_some_and(|holder| holder != project) {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO key_numbers (project, highest, op_id) VALUES (?1, ?2, ?3)
         ON CONFLICT (project) DO UPDATE SET highest = excluded.highest, op_id = excluded.op_id
         WHERE excluded.highest > key_numbers.highest",
        params![project.to_string(), number, op_id.as_bytes()],
    )?;
    Ok(())
}

/// Records an Issue's Key as its `create` was assigned it, or says why it
/// cannot: the Key must be written canonically, use a key its own Project
/// holds or held, and carry a number no other Issue there has.
pub(crate) fn record_key(
    tx: &Transaction<'_>,
    key: &str,
    issue: Uuid,
    project: ProjectId,
) -> Result<Option<String>, StoreError> {
    let Some((prefix, number)) = parse_issue_key(key) else {
        return Ok(Some(format!(
            "{key:?} is not written as KEY-n with n from 1 to {}",
            i64::MAX
        )));
    };
    match key_holder(tx, prefix)? {
        Some(holder) if holder == project => {}
        Some(other) => return Ok(Some(format!("{prefix} belongs to Project {other}"))),
        None => {
            return Ok(Some(format!(
                "no Project held {prefix} when this create was applied"
            )));
        }
    }
    if let Some(other) = issue_by_number(tx, project, number)? {
        return Ok(Some(format!("{key} already names Issue {other}")));
    }
    tx.execute(
        "INSERT INTO keys (key, issue_id, project, number, provisional) VALUES (?1, ?2, ?3, ?4, 0)",
        params![key, issue.as_bytes(), project.to_string(), number],
    )?;
    Ok(None)
}

/// The Key `tm show` gives `issue`, if it has one.
pub(crate) fn key_of(conn: &Connection, issue: Uuid) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            // Without fresh statistics the planner scans keys instead.
            "SELECT key FROM keys INDEXED BY keys_by_issue
             WHERE provisional = 0 AND issue_id = ?1",
            [issue.as_bytes()],
            |row| row.get(0),
        )
        .optional()?)
}

/// The Issue holding `number` in `project`, whichever of its keys it was
/// written with.
pub(crate) fn issue_by_number(
    conn: &Connection,
    project: ProjectId,
    number: i64,
) -> Result<Option<Uuid>, StoreError> {
    let id: Option<Vec<u8>> = conn
        .query_row(
            "SELECT issue_id FROM keys WHERE project = ?1 AND number = ?2",
            params![project.to_string(), number],
            |row| row.get(0),
        )
        .optional()?;
    id.map(|id| crate::apply::uuid_from(&id)).transpose()
}
