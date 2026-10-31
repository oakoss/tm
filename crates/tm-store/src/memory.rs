//! Memories (ADR 0017): applying their operations, the checks a write of one
//! meets, and reading them.

use std::collections::BTreeSet;

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use serde_json::json;
use tm_core::memory::{self as core, Field};
use tm_core::{Actor, Change, Op, ProjectId, Subject};
use uuid::Uuid;

use crate::apply::{Version, Versions, project_from, record_in, sql_int, uuid_from, wins_in};
use crate::{Draft, Store, StoreError};

/// How alike two bodies must read for [`Store::remember`] to name the older
/// one as a likely duplicate.
const ALIKE: f64 = 0.6;

/// A memory as read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Memory {
    pub id: Uuid,
    pub project: ProjectId,
    /// None only when a merge brought the memory without it.
    pub key: Option<String>,
    pub body: Option<String>,
    pub forgotten: bool,
    /// Who wrote the body, the human an Agent wrote it for, and when.
    pub written_by: Option<String>,
    pub written_for: Option<String>,
    pub written_ms: Option<i64>,
}

/// What [`Store::remember`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct Remembered {
    pub memory: Memory,
    /// The body a named key held, and who wrote it, when it was replaced.
    pub replaced: Option<(String, Option<String>)>,
    /// Whether a forgotten memory with the named key came back.
    pub restored: bool,
    /// Live memories whose bodies read much like this one.
    pub alike: Vec<String>,
    /// What could not be read once the write had landed.
    pub warnings: Vec<String>,
    pub ops: Vec<Op>,
}

/// What a remember writes, and the memory it leaves.
struct Plan {
    memory: Memory,
    changes: Vec<Change>,
    replaced: Option<(String, Option<String>)>,
    restored: bool,
}

impl Store {
    /// Remembers `body` in `project`. A named `key`, or a memory's id,
    /// replaces the live memory it names, or brings back a forgotten one
    /// holding that key; without one, a key is made from the body's first
    /// words, numbered when a live memory holds it, so an unnamed write never
    /// replaces anything.
    ///
    /// # Errors
    ///
    /// [`StoreError::Invalid`] for a malformed key or a blank or overlong
    /// body, [`StoreError::MemoryKeyShared`] for a key two live memories
    /// hold, or as [`Store::append_all`].
    pub fn remember(
        &mut self,
        actor: &Actor,
        project: ProjectId,
        body: &str,
        key: Option<&str>,
    ) -> Result<Remembered, StoreError> {
        check_body(body)?;
        if let Some(key) = key
            && !core::is_key(key)
        {
            return Err(invalid(
                Field::Key,
                "must be 1 to 64 lowercase letters, digits or '-', starting with a letter or digit"
                    .into(),
            ));
        }
        let writer = actor.clone();
        let mut planned = None;
        let ops = self.append_with(
            Vec::new(),
            |tx, at_ms| {
                let written = |memory: Memory| Memory {
                    body: Some(body.to_owned()),
                    forgotten: false,
                    written_by: Some(writer.name().to_owned()),
                    written_for: writer.on_behalf_of().map(str::to_owned),
                    written_ms: Some(at_ms),
                    ..memory
                };
                let plan = if let Some(key) = key {
                    named(tx, project, key, body, written)?
                } else {
                    let key = free_key(tx, project, &core::key_from(body))?;
                    let memory = written(Memory {
                        id: Uuid::now_v7(),
                        project,
                        key: Some(key.clone()),
                        ..blank()
                    });
                    Plan {
                        changes: created(&key, body),
                        memory,
                        replaced: None,
                        restored: false,
                    }
                };
                let drafts = drafts(&writer, project, plan.memory.id, plan.changes.clone());
                planned = Some(plan);
                Ok(drafts)
            },
            true,
            None,
        )?;
        let Some(Plan {
            memory,
            replaced,
            restored,
            ..
        }) = planned
        else {
            return Err(StoreError::Corrupt("a remember planned nothing".into()));
        };
        // The write has landed, so a failed read only warns.
        let (alike, warnings) = match self.read(|store| alike(&store.conn, &memory)) {
            Ok(alike) => (alike, Vec::new()),
            Err(err) => (
                Vec::new(),
                vec![format!(
                    "whether another memory reads like this one is unknown: {err}"
                )],
            ),
        };
        Ok(Remembered {
            memory,
            replaced,
            restored,
            alike,
            warnings,
            ops,
        })
    }

    /// Forgets the live memory `key`, or a memory's id, names in `project`,
    /// keeping its record so an undo brings it back.
    ///
    /// # Errors
    ///
    /// [`StoreError::MemoryNotFound`] when no live memory answers to `key`,
    /// [`StoreError::MemoryKeyShared`] when two do, or as
    /// [`Store::append_all`].
    pub fn forget(
        &mut self,
        actor: &Actor,
        project: ProjectId,
        key: &str,
    ) -> Result<(Memory, Vec<Op>), StoreError> {
        let writer = actor.clone();
        let mut forgotten = None;
        let ops = self.append_with(
            Vec::new(),
            |tx, _| {
                let memory = resolve(tx, project, key)?
                    .ok_or_else(|| StoreError::MemoryNotFound(key.into()))?;
                let id = memory.id;
                forgotten = Some(memory);
                Ok(drafts(
                    &writer,
                    project,
                    id,
                    vec![set(Field::Forgotten, json!(true))],
                ))
            },
            true,
            None,
        )?;
        let memory = forgotten.ok_or_else(|| StoreError::MemoryNotFound(key.into()))?;
        Ok((memory, ops))
    }

    /// The live memories in `project`, the most recently written first.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn memories(&self, project: ProjectId) -> Result<Vec<Memory>, StoreError> {
        self.read(|store| live(&store.conn, project))
    }

    /// The live memory `key`, or a memory's id, names in `project`.
    ///
    /// # Errors
    ///
    /// [`StoreError::MemoryKeyShared`] when two live memories hold `key`, any
    /// SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn memory(&self, project: ProjectId, key: &str) -> Result<Option<Memory>, StoreError> {
        self.read(|store| resolve(&store.conn, project, key))
    }
}

/// The write a named key makes: a new body for the live memory it names, a
/// forgotten one holding it brought back, or a new memory; `written` gives
/// the memory the body this write leaves it.
fn named(
    tx: &Transaction<'_>,
    project: ProjectId,
    key: &str,
    body: &str,
    written: impl Fn(Memory) -> Memory,
) -> Result<Plan, StoreError> {
    if let Some(held) = resolve(tx, project, key)? {
        if held.body.as_deref() == Some(body) {
            return Ok(Plan {
                memory: held,
                changes: Vec::new(),
                replaced: None,
                restored: false,
            });
        }
        let replaced = held.body.clone().map(|old| (old, held.written_by.clone()));
        return Ok(Plan {
            memory: written(held),
            changes: vec![set(Field::Body, json!(body))],
            replaced,
            restored: false,
        });
    }
    if let Some(gone) = forgotten_keyed(tx, project, key)?
        && let Some(gone) = read(tx, gone)?
    {
        return Ok(Plan {
            memory: written(gone),
            changes: vec![
                set(Field::Forgotten, json!(false)),
                set(Field::Body, json!(body)),
            ],
            replaced: None,
            restored: true,
        });
    }
    let memory = written(Memory {
        id: Uuid::now_v7(),
        project,
        key: Some(key.to_owned()),
        ..blank()
    });
    Ok(Plan {
        changes: created(key, body),
        memory,
        replaced: None,
        restored: false,
    })
}

fn blank() -> Memory {
    Memory {
        id: Uuid::nil(),
        project: ProjectId::from_uuid(Uuid::nil()),
        key: None,
        body: None,
        forgotten: false,
        written_by: None,
        written_for: None,
        written_ms: None,
    }
}

/// The live memory `given` names in `project`: the one with that id, else
/// the one holding that key; a key two live memories hold, as a merge can
/// leave, is refused so the caller names one by its id.
pub(crate) fn resolve(
    conn: &Connection,
    project: ProjectId,
    given: &str,
) -> Result<Option<Memory>, StoreError> {
    if let Ok(id) = Uuid::parse_str(given)
        && let Some(memory) = read(conn, id)?
        && !memory.forgotten
        && memory.project == project
    {
        return Ok(Some(memory));
    }
    let mut held = live_keyed(conn, project, given)?;
    if held.len() > 1 {
        return Err(StoreError::MemoryKeyShared {
            key: given.to_owned(),
            ids: held.iter().map(|memory| memory.id).collect(),
        });
    }
    Ok(held.pop())
}

fn created(key: &str, body: &str) -> Vec<Change> {
    vec![
        Change::CreateMemory,
        set(Field::Key, json!(key)),
        set(Field::Body, json!(body)),
    ]
}

/// `key`, or the first of `key-2`, `key-3`... no live memory in `project`
/// holds.
fn free_key(tx: &Transaction<'_>, project: ProjectId, key: &str) -> Result<String, StoreError> {
    let mut candidate = key.to_owned();
    let mut n = 1;
    while !live_keyed(tx, project, &candidate)?.is_empty() {
        n += 1;
        let suffix = format!("-{n}");
        let mut base = key.to_owned();
        base.truncate(64 - suffix.len());
        candidate = format!("{}{suffix}", base.trim_end_matches('-'));
    }
    Ok(candidate)
}

/// The keys of the other live memories in its Project that read much like
/// `memory`.
fn alike(conn: &Connection, memory: &Memory) -> Result<Vec<String>, StoreError> {
    let body = memory.body.as_deref().unwrap_or_default();
    Ok(live(conn, memory.project)?
        .into_iter()
        .filter(|other| other.id != memory.id)
        .filter(|other| core::likeness(body, other.body.as_deref().unwrap_or_default()) >= ALIKE)
        .filter_map(|other| other.key)
        .collect())
}

fn check_body(body: &str) -> Result<(), StoreError> {
    if body.trim().is_empty() {
        return Err(invalid(Field::Body, "is blank".into()));
    }
    let count = body.chars().count();
    if count > core::MAX_BODY {
        return Err(invalid(
            Field::Body,
            format!(
                "is {count} characters; the limit is {}, so a longer note belongs in a comment (tm comment KEY) or a doc",
                core::MAX_BODY
            ),
        ));
    }
    Ok(())
}

fn invalid(field: Field, reason: String) -> StoreError {
    StoreError::Invalid {
        field: field.as_str().into(),
        reason,
        valid: Vec::new(),
    }
}

fn set(field: Field, value: serde_json::Value) -> Change {
    Change::SetMemory { field, value }
}

fn drafts(actor: &Actor, project: ProjectId, id: Uuid, changes: Vec<Change>) -> Vec<Draft> {
    changes
        .into_iter()
        .map(|change| Draft {
            actor: actor.clone(),
            project,
            subject: id,
            change,
        })
        .collect()
}

const COLUMNS: &str = "id, project, key, body, forgotten, written_by, written_for, written_ms";

/// A `memories` row as selected in [`COLUMNS`] order, before its ids are checked.
struct Row {
    id: Vec<u8>,
    project: String,
    key: Option<String>,
    body: Option<String>,
    forgotten: bool,
    written_by: Option<String>,
    written_for: Option<String>,
    written_ms: Option<i64>,
}

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        id: row.get(0)?,
        project: row.get(1)?,
        key: row.get(2)?,
        body: row.get(3)?,
        forgotten: row.get(4)?,
        written_by: row.get(5)?,
        written_for: row.get(6)?,
        written_ms: row.get(7)?,
    })
}

fn finish(row: Row) -> Result<Memory, StoreError> {
    Ok(Memory {
        id: uuid_from(&row.id)?,
        project: project_from(&row.project)?,
        key: row.key,
        body: row.body,
        forgotten: row.forgotten,
        written_by: row.written_by,
        written_for: row.written_for,
        written_ms: row.written_ms,
    })
}

pub(crate) fn read(conn: &Connection, id: Uuid) -> Result<Option<Memory>, StoreError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM memories WHERE id = ?1"),
        [id.as_bytes()],
        row,
    )
    .optional()?
    .map(finish)
    .transpose()
}

pub(crate) fn live(conn: &Connection, project: ProjectId) -> Result<Vec<Memory>, StoreError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM memories WHERE project = ?1 AND forgotten = 0
         ORDER BY written_ms DESC, key, id"
    ))?;
    let rows = stmt.query_map([project.to_string()], row)?;
    rows.map(|memory| finish(memory?)).collect()
}

/// The live memories holding `key` in `project`; more than one only after a
/// merge.
pub(crate) fn live_keyed(
    conn: &Connection,
    project: ProjectId,
    key: &str,
) -> Result<Vec<Memory>, StoreError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM memories WHERE project = ?1 AND key = ?2 AND forgotten = 0
         ORDER BY created_hlc, id"
    ))?;
    let rows = stmt.query_map(params![project.to_string(), key], row)?;
    rows.map(|memory| finish(memory?)).collect()
}

fn forgotten_keyed(
    conn: &Connection,
    project: ProjectId,
    key: &str,
) -> Result<Option<Uuid>, StoreError> {
    let id: Option<Vec<u8>> = conn
        .query_row(
            "SELECT id FROM memories WHERE project = ?1 AND key = ?2 AND forgotten = 1
             ORDER BY created_hlc DESC, id DESC LIMIT 1",
            params![project.to_string(), key],
            |row| row.get(0),
        )
        .optional()?;
    id.map(|id| uuid_from(&id)).transpose()
}

pub(crate) fn project_of(conn: &Connection, id: Uuid) -> Result<Option<ProjectId>, StoreError> {
    let text: Option<String> = conn
        .query_row(
            "SELECT project FROM memories WHERE id = ?1",
            [id.as_bytes()],
            |row| row.get(0),
        )
        .optional()?;
    text.map(|text| project_from(&text)).transpose()
}

/// Applies a memory operation. A merge may leave two live memories sharing a
/// key; only a write refuses that.
pub(crate) fn apply(
    tx: &Transaction<'_>,
    op: &Op,
    change: &Change,
    version: &Version,
) -> Result<(), StoreError> {
    if matches!(change, Change::CreateMemory) {
        if !crate::project::exists(tx, op.project)? {
            return Err(StoreError::UnknownProject(op.project));
        }
        if project_of(tx, op.subject)?.is_some() {
            return Err(StoreError::DuplicateMemory(op.subject));
        }
        tx.execute(
            "INSERT INTO memories (id, project, created_hlc) VALUES (?1, ?2, ?3)",
            params![
                op.subject.as_bytes(),
                op.project.to_string(),
                sql_int(op.hlc.raw())?
            ],
        )?;
        return Ok(());
    }
    let expected = project_of(tx, op.subject)?.ok_or(StoreError::UnknownMemory(op.subject))?;
    if expected != op.project {
        return Err(StoreError::MemoryMismatch {
            memory: op.subject,
            expected,
            found: op.project,
        });
    }
    // Its source is on the operation, which every reader looks up there.
    if matches!(change, Change::MemorySource) {
        return Ok(());
    }
    let Change::SetMemory { field, value } = change else {
        unreachable!("only memory kinds reach here")
    };
    let stored = column_value(*field, value)?;
    if !wins_in(
        tx,
        Versions::Memory,
        op.subject,
        field.as_str(),
        "",
        version,
    )? {
        return Ok(());
    }
    record_in(
        tx,
        Versions::Memory,
        op.subject,
        (field.as_str(), ""),
        true,
        version,
    )?;
    let column = field.as_str();
    tx.execute(
        &format!("UPDATE memories SET {column} = ?2 WHERE id = ?1"),
        params![op.subject.as_bytes(), stored],
    )?;
    if *field == Field::Body {
        tx.execute(
            "UPDATE memories SET written_by = ?2, written_for = ?3, written_ms = ?4 WHERE id = ?1",
            params![op.subject.as_bytes(), op.actor, op.on_behalf_of, op.utc_ms],
        )?;
    }
    Ok(())
}

fn column_value(field: Field, value: &serde_json::Value) -> Result<SqlValue, StoreError> {
    match field {
        Field::Key | Field::Body => value.as_str().map(|text| SqlValue::Text(text.to_owned())),
        Field::Forgotten => value
            .as_bool()
            .map(|forgotten| SqlValue::Integer(forgotten.into())),
    }
    .ok_or_else(|| StoreError::Corrupt(format!("memory {} value {value}", field.as_str())))
}

/// The checks a write of a memory operation meets, which replay does not
/// repeat: a memory written to exists and is not forgotten, unless the write
/// brings it back, and its body is not blank or overlong. Keys are checked by
/// [`check_batch`].
pub(crate) fn check(tx: &Transaction<'_>, id: Uuid, change: Change) -> Result<Change, StoreError> {
    if let Change::SetMemory {
        field: Field::Body,
        value,
    } = &change
    {
        check_body(value.as_str().unwrap_or_default())?;
    }
    if matches!(change, Change::CreateMemory) {
        if project_of(tx, id)?.is_some() {
            return Err(StoreError::DuplicateMemory(id));
        }
        return Ok(change);
    }
    let memory = read(tx, id)?.ok_or(StoreError::UnknownMemory(id))?;
    let restoring = matches!(
        &change,
        Change::SetMemory { field: Field::Forgotten, value } if value == &json!(false)
    );
    if memory.forgotten && !restoring {
        return Err(StoreError::UnknownMemory(id));
    }
    Ok(change)
}

/// The checks a batch's end state meets: a memory it creates has a key and a
/// body, and a memory whose key it makes live (creating it, setting its key
/// or bringing it back) holds one no other live memory in its Project holds,
/// so an undo may move keys in any order. A key a merge left shared stays
/// writable otherwise.
pub(crate) fn check_batch(tx: &Transaction<'_>, ops: &[Op]) -> Result<(), StoreError> {
    let mut written = BTreeSet::new();
    let mut keyed = BTreeSet::new();
    for op in ops {
        let Ok(change) = op.change() else { continue };
        if change.subject() != Subject::Memory {
            continue;
        }
        written.insert(op.subject);
        let keys = match &change {
            Change::CreateMemory
            | Change::SetMemory {
                field: Field::Key, ..
            } => true,
            Change::SetMemory {
                field: Field::Forgotten,
                value,
            } => value == &json!(false),
            _ => false,
        };
        if keys {
            keyed.insert(op.subject);
        }
    }
    // An import may create a memory sharing a key, as a merge would bring
    // one; changing an existing memory's key is checked whoever writes it.
    for op in ops {
        if op.kind == "create_memory" && op.source.is_some() {
            keyed.remove(&op.subject);
        }
    }
    for id in written {
        let memory = read(tx, id)?.ok_or(StoreError::UnknownMemory(id))?;
        let created = ops
            .iter()
            .any(|op| op.subject == id && op.kind == "create_memory");
        if created && memory.key.is_none() {
            return Err(invalid(Field::Key, "is needed for a new memory".into()));
        }
        if created && memory.body.is_none() {
            return Err(invalid(Field::Body, "is needed for a new memory".into()));
        }
        if let (false, Some(key), true) = (memory.forgotten, &memory.key, keyed.contains(&id)) {
            key_free(tx, memory.project, key, id)?;
        }
    }
    Ok(())
}

fn key_free(
    tx: &Transaction<'_>,
    project: ProjectId,
    key: &str,
    except: Uuid,
) -> Result<(), StoreError> {
    let taken: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM memories
         WHERE project = ?1 AND forgotten = 0 AND id <> ?2 AND key = ?3)",
        params![project.to_string(), except.as_bytes(), key],
        |row| row.get(0),
    )?;
    if taken {
        return Err(invalid(
            Field::Key,
            format!("{key} is held by another memory in this Project"),
        ));
    }
    Ok(())
}
