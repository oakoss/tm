//! What record import does alike for Short-lists and memories (ADR 0013):
//! finding a record by its source, drafting writes that carry the source,
//! writing one record's plan as a guarded batch, and comparing a field three
//! ways.

use rusqlite::{Connection, params};
use serde_json::Value;
use tm_core::{Actor, Change, ProjectId};
use uuid::Uuid;

use crate::apply::uuid_from;
use crate::import::{Decision, base, decide};
use crate::record::TM_SOURCE;
use crate::{Draft, Provenance, Store, StoreError, WallTime};

/// A record's create kind, and its source kind if it is ever matched by
/// name or key.
pub(crate) type Kinds = (&'static str, Option<&'static str>);

pub(crate) trait Planned: PartialEq {
    fn drafts(&self) -> &[(Draft, Provenance)];
}

/// A record `source` names, by its own `tm:` id or on an earlier import's
/// create or source operation.
pub(crate) enum BySource {
    Live(Uuid),
    /// Deleted or forgotten here, so not made again.
    Gone(Uuid),
    None,
}

/// The first live record `source` names, else the first gone one; `gone`
/// says whether a record is gone, or none when it does not exist.
pub(crate) fn by_source(
    conn: &Connection,
    source: &str,
    kinds: Kinds,
    gone: impl Fn(&Connection, Uuid) -> Result<Option<bool>, StoreError>,
) -> Result<BySource, StoreError> {
    let mut held = Vec::new();
    if let Some(Ok(id)) = source.strip_prefix(TM_SOURCE).map(Uuid::parse_str) {
        held.push(id);
    }
    held.extend(sourced(conn, source, kinds)?);
    let mut first_gone = None;
    for id in held {
        match gone(conn, id)? {
            Some(false) => return Ok(BySource::Live(id)),
            Some(true) => first_gone = first_gone.or(Some(id)),
            None => {}
        }
    }
    Ok(first_gone.map_or(BySource::None, BySource::Gone))
}

/// The records an import from `source` wrote one of `kinds` for, oldest
/// first.
pub(crate) fn sourced(
    conn: &Connection,
    source: &str,
    (create, matched): Kinds,
) -> Result<Vec<Uuid>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT issue_id FROM ops WHERE source = ?1 AND kind IN (?2, ?3) ORDER BY seq",
    )?;
    stmt.query_map(params![source, create, matched.unwrap_or(create)], |row| {
        row.get(0)
    })?
    .map(|id: rusqlite::Result<Vec<u8>>| uuid_from(&id?))
    .collect()
}

/// Drafts a change to `subject` carrying `source`.
pub(crate) fn drafter<'a>(
    (runner, now): (&'a Actor, WallTime),
    project: ProjectId,
    subject: Uuid,
    source: &'a str,
) -> impl Fn(Change) -> (Draft, Provenance) + 'a {
    move |change| {
        let draft = Draft {
            actor: runner.clone(),
            project,
            subject,
            change,
        };
        let provenance = Provenance {
            source: source.to_owned(),
            at: now,
        };
        (draft, provenance)
    }
}

/// How `field` of `subject` compares: what the last import from `source`
/// wrote, against `wanted` and `tm`.
pub(crate) fn three_way(
    conn: &Connection,
    (subject, source, field): (Uuid, &str, &str),
    (wanted, tm): (&Value, &Value),
) -> Result<Decision, StoreError> {
    let base = base(conn, source, Some(subject), field)?.remove(&(subject, String::new()));
    let base = base.as_ref().and_then(Change::set_value);
    Ok(decide(wanted, base, tm))
}

impl Store {
    /// The Project a project line's `key` names here.
    pub(crate) fn import_project(&self, key: &str) -> Result<ProjectId, StoreError> {
        self.project_by_key(key)?
            .ok_or_else(|| StoreError::Invalid {
                field: "project".into(),
                reason: format!("{key} is no Project here"),
                valid: Vec::new(),
            })
    }

    /// Writes `planned`'s drafts as one batch, refused when `plan` gives
    /// another plan inside the write, so a change made meanwhile is not
    /// overwritten; the batch, when it wrote one.
    pub(crate) fn write_planned<P: Planned>(
        &mut self,
        planned: &P,
        plan: impl Fn(&Connection) -> Result<P, StoreError>,
        (field, source): (&str, &str),
    ) -> Result<Vec<Uuid>, StoreError> {
        if planned.drafts().is_empty() {
            return Ok(Vec::new());
        }
        let ops = self.append_imported_guarded(planned.drafts().to_vec(), |tx| {
            if plan(tx)? == *planned {
                Ok(())
            } else {
                Err(StoreError::Invalid {
                    field: field.into(),
                    reason: format!("{source} changed while it was being imported"),
                    valid: Vec::new(),
                })
            }
        })?;
        Ok(ops.first().map(|op| op.op_id).into_iter().collect())
    }
}
