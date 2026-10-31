//! Record import of memories (ADR 0017), as Short-lists are imported: one
//! `found` misses is created with the source on its create, one matched by
//! key gains the source in an operation of its own, and a found one's key and
//! body are compared three ways, as ADR 0013 compares an Issue's fields.

use std::collections::BTreeSet;

use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::{Value, json};
use tm_core::memory::{self as core, Field};
use tm_core::{Actor, Change, ProjectId};
use uuid::Uuid;

use crate::apply::uuid_from;
use crate::import::{Conflict, Decision, Parsed, base, decide, refused};
use crate::record::{MemoryRecord, TM_SOURCE};
use crate::{Draft, Provenance, Store, StoreError, WallTime};

/// What an import did with one memory a project line holds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MemoryOutcome {
    pub line: usize,
    pub source: String,
    pub memory: Uuid,
    /// Its key after the import.
    pub key: Option<String>,
    pub created: bool,
    /// Matched by key rather than by source.
    pub by_key: bool,
    /// Forgotten here, so nothing is written to it.
    pub forgotten: bool,
    pub batches: Vec<Uuid>,
    /// Each field set.
    pub written: Vec<String>,
    /// Fields both tm and the source changed; tm's value was kept.
    pub conflicts: Vec<Conflict>,
}

/// One memory's writes as one batch, and what they do.
#[derive(PartialEq)]
struct MemoryPlan {
    memory: Uuid,
    created: bool,
    by_key: bool,
    forgotten: bool,
    drafts: Vec<(Draft, Provenance)>,
    written: Vec<String>,
    conflicts: Vec<Conflict>,
}

enum Found {
    Source(Uuid),
    Key(Uuid),
    Forgotten(Uuid),
    None,
}

impl Store {
    /// Finds or creates each memory `parsed`'s project lines hold; each
    /// memory's writes are one batch, added to `written` for a later refusal
    /// to name.
    pub(crate) fn import_memories(
        &mut self,
        parsed: &Parsed,
        runner: &Actor,
        now: WallTime,
        written: &mut Vec<Uuid>,
    ) -> Result<Vec<MemoryOutcome>, StoreError> {
        let mut outcomes = Vec::new();
        for (line, project) in &parsed.projects {
            for record in &project.memories {
                let unique = unique_in(&project.memories, record);
                let outcome = self
                    .import_memory(*line, (&project.key, record, unique), (runner, now))
                    .map_err(|cause| refused(*line, &record.source, cause, (written, &[])))?;
                written.extend(&outcome.batches);
                outcomes.push(outcome);
            }
        }
        Ok(outcomes)
    }

    fn import_memory(
        &mut self,
        line: usize,
        (project, record, unique): (&str, &MemoryRecord, bool),
        writer: (&Actor, WallTime),
    ) -> Result<MemoryOutcome, StoreError> {
        let project = self
            .project_by_key(project)?
            .ok_or_else(|| StoreError::Invalid {
                field: "project".into(),
                reason: format!("{project} is no Project here"),
                valid: Vec::new(),
            })?;
        let new = Uuid::now_v7();
        let plan = |conn: &Connection| plan_memory(conn, (project, record, unique), new, writer);
        let planned = plan(&self.conn)?;
        let mut batches = Vec::new();
        if !planned.drafts.is_empty() {
            let ops = self.append_imported_guarded(planned.drafts.clone(), |tx| {
                if plan(tx)? == planned {
                    Ok(())
                } else {
                    Err(StoreError::Invalid {
                        field: "memory".into(),
                        reason: format!("{} changed while it was being imported", record.source),
                        valid: Vec::new(),
                    })
                }
            })?;
            batches.extend(ops.first().map(|op| op.op_id));
        }
        let key = crate::memory::read(&self.conn, planned.memory)?.and_then(|memory| memory.key);
        Ok(MemoryOutcome {
            line,
            source: record.source.clone(),
            memory: planned.memory,
            key,
            created: planned.created,
            by_key: planned.by_key,
            forgotten: planned.forgotten,
            batches,
            written: planned.written,
            conflicts: planned.conflicts,
        })
    }
}

/// Refuses a project line's memory that cannot be imported, before anything
/// is written: an unreadable `tm:` source, a malformed key or body, or a
/// source named twice. Two holding one key, as a merge leaves, import.
pub(crate) fn check_memories(parsed: &Parsed) -> Result<(), StoreError> {
    let invalid = |line: usize, reason: String| StoreError::InvalidExport { line, reason };
    let mut sources = BTreeSet::new();
    for (line, project) in &parsed.projects {
        for record in &project.memories {
            let source = &record.source;
            if let Some(id) = source.strip_prefix(TM_SOURCE)
                && Uuid::parse_str(id).is_err()
            {
                return Err(invalid(
                    *line,
                    format!("memory source {source} names no tm id"),
                ));
            }
            if let Some(key) = &record.key
                && !core::is_key(key)
            {
                return Err(invalid(
                    *line,
                    format!("memory {source} has the key {key:?}, which no memory can hold"),
                ));
            }
            if let Some(body) = &record.body
                && (body.trim().is_empty() || body.chars().count() > core::MAX_BODY)
            {
                let reason = format!(
                    "memory {source}'s body is blank or over {} characters",
                    core::MAX_BODY
                );
                return Err(invalid(*line, reason));
            }
            if !sources.insert(source.as_str()) {
                return Err(invalid(*line, format!("memory {source} is named twice")));
            }
        }
    }
    Ok(())
}

/// Whether `record`'s key is held by no other memory on its project line.
fn unique_in(line: &[MemoryRecord], record: &MemoryRecord) -> bool {
    record.key.is_some() && line.iter().filter(|other| other.key == record.key).count() == 1
}

fn plan_memory(
    conn: &Connection,
    (project, record, unique): (ProjectId, &MemoryRecord, bool),
    new: Uuid,
    (runner, now): (&Actor, WallTime),
) -> Result<MemoryPlan, StoreError> {
    let found = found(conn, project, record, unique)?;
    let (memory, created, by_key, forgotten) = match found {
        Found::Source(id) => (id, false, false, false),
        Found::Key(id) => (id, false, true, false),
        Found::Forgotten(id) => (id, false, false, true),
        Found::None => (new, true, false, false),
    };
    if let Some(held) = crate::memory::read(conn, memory)?
        && held.project != project
    {
        return Err(StoreError::Invalid {
            field: "memory".into(),
            reason: format!("{} is a memory in another Project here", record.source),
            valid: Vec::new(),
        });
    }
    let draft = |change: Change| {
        let draft = Draft {
            actor: runner.clone(),
            project,
            subject: memory,
            change,
        };
        let provenance = Provenance {
            source: record.source.clone(),
            at: now,
        };
        (draft, provenance)
    };
    let mut plan = MemoryPlan {
        memory,
        created,
        by_key,
        forgotten,
        drafts: Vec::new(),
        written: Vec::new(),
        conflicts: Vec::new(),
    };
    if forgotten {
        return Ok(plan);
    }
    if created {
        let (Some(key), Some(body)) = (&record.key, &record.body) else {
            return Err(StoreError::Invalid {
                field: "memory".into(),
                reason: format!(
                    "{} has no key or no body, so no memory can be made for it",
                    record.source
                ),
                valid: Vec::new(),
            });
        };
        plan.drafts.push(draft(Change::CreateMemory));
        plan.drafts.push(draft(set(Field::Key, json!(key))));
        plan.drafts.push(draft(set(Field::Body, json!(body))));
        plan.written.push("created".into());
        return Ok(plan);
    }
    if by_key {
        plan.drafts.push(draft(Change::MemorySource));
    }
    let held = crate::memory::read(conn, memory)?.ok_or(StoreError::UnknownMemory(memory))?;
    // A field the source lacks, as a merge can leave, is not compared.
    let fields = [
        (
            Field::Key,
            record.key.as_ref().map(|key| json!(key)),
            json!(held.key),
        ),
        (
            Field::Body,
            record.body.as_ref().map(|body| json!(body)),
            json!(held.body),
        ),
    ];
    for (field, wanted, tm) in fields {
        let Some(wanted) = wanted else { continue };
        let base = match base(conn, &record.source, Some(memory), field.as_str())?
            .remove(&(memory, String::new()))
        {
            Some(Change::SetMemory { value, .. }) => Some(value),
            _ => None,
        };
        match decide(&wanted, base.as_ref(), &tm) {
            Decision::Keep => {}
            Decision::Write => {
                if wanted != tm {
                    plan.written.push(field.as_str().into());
                }
                plan.drafts.push(draft(set(field, wanted)));
            }
            Decision::Conflict => plan.conflicts.push(Conflict {
                field: field.as_str().into(),
                tm,
                source: wanted,
                reason: None,
            }),
        }
    }
    Ok(plan)
}

/// The live memory `record` names by its source, as its own id or on an
/// earlier import's create; else one of the source's forgotten here, which
/// is not made again; else the live one holding its key in `project`, when
/// the key is `unique` on its line and one memory here holds it.
fn found(
    conn: &Connection,
    project: ProjectId,
    record: &MemoryRecord,
    unique: bool,
) -> Result<Found, StoreError> {
    let mut held = Vec::new();
    if let Some(Ok(id)) = record.source.strip_prefix(TM_SOURCE).map(Uuid::parse_str) {
        held.push(id);
    }
    let mut stmt = conn.prepare_cached(
        "SELECT issue_id FROM ops WHERE source = ?1 AND kind IN ('create_memory', 'memory_source')
         ORDER BY seq",
    )?;
    let sourced = stmt
        .query_map(params![record.source], |row| row.get(0))?
        .map(|id: rusqlite::Result<Vec<u8>>| uuid_from(&id?))
        .collect::<Result<Vec<_>, StoreError>>()?;
    held.extend(sourced);
    let mut forgotten = None;
    for id in held {
        match crate::memory::read(conn, id)? {
            Some(memory) if !memory.forgotten => return Ok(Found::Source(id)),
            Some(_) => forgotten = forgotten.or(Some(id)),
            None => {}
        }
    }
    if let Some(id) = forgotten {
        return Ok(Found::Forgotten(id));
    }
    let (Some(key), true) = (&record.key, unique) else {
        return Ok(Found::None);
    };
    match crate::memory::live_keyed(conn, project, key)?.as_slice() {
        [held] => Ok(Found::Key(held.id)),
        _ => Ok(Found::None),
    }
}

fn set(field: Field, value: Value) -> Change {
    Change::SetMemory { field, value }
}
