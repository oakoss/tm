//! Write-time checks against the Scheme. They run only when this Replica
//! writes; replay never consults the Scheme, so a Scheme edit or a merge never
//! makes a stored operation unreadable.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};

use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::Value;
use tm_core::op::{CLAIM_FIELD, COMMENTS_FIELD};
use tm_core::scheme::{Field, FieldType, Scheme, is_date, priority_rank};
use tm_core::{Actor, Change, link};
use uuid::Uuid;

use crate::StoreError;
use crate::apply::uuid_from;

/// Who writes a change, and when the operation records it.
pub(crate) struct Writing<'a> {
    pub(crate) actor: &'a Actor,
    pub(crate) at_ms: i64,
}

/// Refuses a change no import may bring in, checked before anything else
/// about the batch.
pub(crate) fn check_importable(change: &Change) -> Result<(), StoreError> {
    match change {
        // A Claim is who works on an Issue now, not history to bring in.
        Change::Claim { .. } => Err(StoreError::Invalid {
            field: CLAIM_FIELD.into(),
            reason: "cannot be imported".into(),
            valid: Vec::new(),
        }),
        _ => Ok(()),
    }
}

/// Checks `change` against its writer, the Scheme and the Issue's current
/// state, and fills in the display labels the operation records.
pub(crate) fn check(
    tx: &Transaction<'_>,
    scheme: &Scheme,
    writing: &Writing<'_>,
    issue_id: Uuid,
    change: Change,
) -> Result<Change, StoreError> {
    check_writer(tx, writing, issue_id, &change)?;
    if change.is_project() {
        return check_project(change);
    }
    if !matches!(change, Change::Create) && !exists(tx, issue_id)? {
        return Err(StoreError::UnknownIssue(issue_id));
    }
    match change {
        Change::Create | Change::Comment { .. } => Ok(change),
        Change::CreateProject { .. } | Change::RenameProject { .. } => {
            unreachable!("Project operations return above")
        }
        Change::SetField { field, value, .. } => {
            // A stored value can be cleared even when the Scheme lacks its field.
            if value.is_null() && scheme.field(&field).is_none() && has_value(tx, issue_id, &field)?
            {
                return Ok(Change::SetField {
                    label: field.clone(),
                    field,
                    value,
                    value_label: None,
                });
            }
            let def = settable(scheme, &field)?;
            if def.field_type.is_multi() {
                return Err(invalid(def, "holds several values; add or remove them", []));
            }
            let value_label = check_value(tx, scheme, issue_id, def, &value)?;
            Ok(Change::SetField {
                label: def.label.clone(),
                field,
                value,
                value_label,
            })
        }
        Change::Add { field, elem, .. } => {
            let (def, elem, elem_label) = member(scheme, &field, &elem)?;
            Ok(Change::Add {
                field,
                label: def.label.clone(),
                elem,
                elem_label,
            })
        }
        // At the time the operation records, so replay reaches the same verdict.
        Change::Claim { action } => {
            crate::claim::check(tx, issue_id, &action, (writing.actor, writing.at_ms))?;
            Ok(Change::Claim { action })
        }
        Change::Retract { comment, retracted } => {
            comment_author(tx, issue_id, comment)?;
            Ok(Change::Retract { comment, retracted })
        }
        Change::Link {
            link_type,
            target,
            present,
        } => {
            check_link(tx, issue_id, &link_type, target, present)?;
            Ok(Change::Link {
                link_type,
                target,
                present,
            })
        }
        Change::Remove { field, elem, .. } => {
            // A held element can be removed even when the Scheme does not allow it.
            let def = scheme.field(&field);
            let stored = match def.map(|def| def.field_type) {
                Some(FieldType::Labels) => elem.trim().to_lowercase(),
                _ => elem.clone(),
            };
            if let Some(present) = stored_member(tx, issue_id, &field, &stored)? {
                return Ok(Change::Remove {
                    label: def.map_or_else(|| field.clone(), |def| def.label.clone()),
                    field,
                    elem: stored,
                    elem_label: present.display,
                });
            }
            let (def, elem, elem_label) = member(scheme, &field, &elem)?;
            Ok(Change::Remove {
                field,
                label: def.label.clone(),
                elem,
                elem_label,
            })
        }
    }
}

/// Who wrote comment `comment` on `issue_id`, and the human it was written
/// for, if an Agent wrote it.
fn comment_author(
    tx: &Transaction<'_>,
    issue_id: Uuid,
    comment: Uuid,
) -> Result<(String, Option<String>), StoreError> {
    tx.query_row(
        "SELECT c.actor, o.on_behalf_of FROM comments c JOIN ops o ON o.op_id = c.op_id
         WHERE c.op_id = ?1 AND c.issue_id = ?2",
        params![comment.as_bytes(), issue_id.as_bytes()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()?
    .ok_or_else(|| StoreError::Invalid {
        field: COMMENTS_FIELD.into(),
        reason: format!("Issue {issue_id} has no comment {comment}"),
        valid: Vec::new(),
    })
}

/// The rules a change kind sets on who writes it and how.
fn check_writer(
    tx: &Transaction<'_>,
    writing: &Writing<'_>,
    issue_id: Uuid,
    change: &Change,
) -> Result<(), StoreError> {
    match change {
        Change::Claim { action } => crate::claim::check_taker(writing.actor, action),
        Change::Retract { comment, .. } => check_retracter(tx, issue_id, writing.actor, *comment),
        _ => Ok(()),
    }
}

/// Only a comment's author, or the human an Agent wrote it for, may retract
/// it or bring it back.
fn check_retracter(
    tx: &Transaction<'_>,
    issue_id: Uuid,
    writer: &Actor,
    comment: Uuid,
) -> Result<(), StoreError> {
    let (author, for_human) = comment_author(tx, issue_id, comment)?;
    let same = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();
    // Agent names are shared across humans, so the author is the name and
    // the human it wrote for together.
    let is_author = same(&author, writer.name())
        && match (for_human.as_deref(), writer.on_behalf_of()) {
            (None, None) => true,
            (Some(human), Some(writer_human)) => same(human, writer_human),
            _ => false,
        };
    let is_their_human = !writer.is_agent()
        && for_human
            .as_deref()
            .is_some_and(|human| same(human, writer.name()));
    if is_author || is_their_human {
        return Ok(());
    }
    Err(StoreError::Invalid {
        field: COMMENTS_FIELD.into(),
        reason: format!(
            "only {author}{} can retract this comment, not {}",
            for_human.map_or_else(String::new, |human| format!(" or {human}")),
            writer.name()
        ),
        valid: Vec::new(),
    })
}

/// A held Link can be removed whatever its type; any other Link must have a
/// built-in type, join two existing Issues and, for `blocks`, not close a
/// cycle. Applying it checks that it is filed under its canonical source.
fn check_link(
    tx: &Transaction<'_>,
    source: Uuid,
    link_type: &str,
    target: Uuid,
    present: bool,
) -> Result<(), StoreError> {
    if !present && has_link(tx, link_type, source, target)? {
        return Ok(());
    }
    // Before the cycle check, which would report an Issue joined to itself as a cycle.
    link_shape(link_type, source, target)?;
    if !exists(tx, target)? {
        return Err(StoreError::UnknownIssue(target));
    }
    if present
        && link_type == link::BLOCKS
        && let Some(path) = blocking_path(tx, target, source)?
    {
        return Err(StoreError::Cycle {
            relation: link::BLOCKS,
            path: [source].into_iter().chain(path).collect(),
        });
    }
    Ok(())
}

/// The Link checks that read no Issue: a built-in type joining two Issues.
pub(crate) fn link_shape(link_type: &str, source: Uuid, target: Uuid) -> Result<(), StoreError> {
    let invalid = |reason: &str, valid: Vec<String>| StoreError::Invalid {
        field: "link".into(),
        reason: reason.to_owned(),
        valid,
    };
    if link::link_type(link_type).is_none() {
        return Err(invalid(
            "is not a Link type",
            link::BUILT_IN.iter().map(|t| t.id.to_owned()).collect(),
        ));
    }
    if target == source {
        return Err(invalid("cannot join an Issue to itself", Vec::new()));
    }
    Ok(())
}

pub(crate) fn has_link(
    conn: &rusqlite::Connection,
    link_type: &str,
    source: Uuid,
    target: Uuid,
) -> Result<bool, StoreError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM links WHERE type = ?1 AND src = ?2 AND dst = ?3",
            params![link_type, source.as_bytes(), target.as_bytes()],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// The `blocks` path from `start` to `goal`, both included, if `goal` is
/// reachable. Detection is one recursive query; `UNION` on the id alone keeps
/// it finite on a merged cycle, and `CROSS JOIN` pins the join order, without
/// which the planner scanned every Link per step (ADR 0009). The path is
/// walked only after the query finds one.
fn blocking_path(
    tx: &Transaction<'_>,
    start: Uuid,
    goal: Uuid,
) -> Result<Option<Vec<Uuid>>, StoreError> {
    let reaches = tx
        .query_row(
            "WITH RECURSIVE reach(id) AS (
               SELECT ?1
               UNION
               SELECT l.dst FROM reach r CROSS JOIN links l
               WHERE l.type = ?3 AND l.src = r.id
             )
             SELECT 1 FROM reach WHERE id = ?2",
            params![start.as_bytes(), goal.as_bytes(), link::BLOCKS],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !reaches {
        return Ok(None);
    }
    let next = format!(
        "SELECT dst FROM links WHERE type = '{}' AND src = ?1",
        link::BLOCKS
    );
    shortest_path(tx, &next, start, goal)?
        .ok_or_else(|| {
            StoreError::Corrupt("a blocks path the recursive query found is missing".into())
        })
        .map(Some)
}

/// The shortest path from `start` to `goal`, both included, following the
/// Issues `next` selects for `?1`. Visiting each Issue once keeps it finite
/// on a cycle a merge left.
fn shortest_path(
    tx: &Transaction<'_>,
    next: &str,
    start: Uuid,
    goal: Uuid,
) -> Result<Option<Vec<Uuid>>, StoreError> {
    let mut stmt = tx.prepare(next)?;
    let mut came_from = HashMap::from([(start, start)]);
    let mut queue = VecDeque::from([start]);
    while let Some(at) = queue.pop_front() {
        if at == goal {
            let mut path = vec![goal];
            let mut step = goal;
            while step != start {
                step = came_from[&step];
                path.push(step);
            }
            path.reverse();
            return Ok(Some(path));
        }
        let ids = stmt
            .query_map([at.as_bytes()], |row| row.get::<_, Vec<u8>>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for bytes in ids {
            let id = uuid_from(&bytes)?;
            if let Entry::Vacant(entry) = came_from.entry(id) {
                entry.insert(at);
                queue.push_back(id);
            }
        }
    }
    Ok(None)
}

/// Project names must not be blank. Key spelling is checked with the
/// operation format, and whether a key is free when the operation applies.
fn check_project(change: Change) -> Result<Change, StoreError> {
    let invalid = |field: &str, reason: &str| StoreError::Invalid {
        field: field.to_owned(),
        reason: reason.to_owned(),
        valid: Vec::new(),
    };
    let name_ok = |name: &str| !name.trim().is_empty();
    match change {
        Change::CreateProject { key, name } => {
            if !name_ok(&name) {
                return Err(invalid("name", "cannot be blank"));
            }
            Ok(Change::CreateProject { key, name })
        }
        Change::RenameProject { key, name } => {
            if key.is_none() && name.is_none() {
                return Err(invalid("key", "or a name is needed to rename a Project"));
            }
            if name.as_deref().is_some_and(|name| !name_ok(name)) {
                return Err(invalid("name", "cannot be blank"));
            }
            Ok(Change::RenameProject { key, name })
        }
        _ => unreachable!("only Project kinds reach here"),
    }
}

/// A set element an Issue holds, with the display form it was added with.
struct Present {
    display: Option<String>,
}

fn stored_member(
    tx: &Transaction<'_>,
    issue_id: Uuid,
    field: &str,
    elem: &str,
) -> Result<Option<Present>, StoreError> {
    Ok(tx
        .query_row(
            "SELECT display FROM set_members WHERE issue_id = ?1 AND field = ?2 AND elem = ?3",
            params![issue_id.as_bytes(), field, elem],
            |row| {
                Ok(Present {
                    display: row.get(0)?,
                })
            },
        )
        .optional()?)
}

fn has_value(tx: &Transaction<'_>, issue_id: Uuid, field: &str) -> Result<bool, StoreError> {
    Ok(tx
        .query_row(
            "SELECT 1 FROM field_values WHERE issue_id = ?1 AND field = ?2",
            params![issue_id.as_bytes(), field],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn has_member(tx: &Transaction<'_>, issue_id: Uuid, field: &str) -> Result<bool, StoreError> {
    Ok(tx
        .query_row(
            "SELECT 1 FROM set_members WHERE issue_id = ?1 AND field = ?2",
            params![issue_id.as_bytes(), field],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Whether the Issue's Status belongs to its type's Workflow; true while either
/// is unset or the Issue does not exist yet.
pub(crate) fn status_fits_type(
    tx: &Transaction<'_>,
    scheme: &Scheme,
    issue_id: Uuid,
) -> Result<bool, StoreError> {
    let (Some(issue_type), Some(status)) = (
        current(tx, issue_id, "type")?,
        current(tx, issue_id, "status")?,
    ) else {
        return Ok(true);
    };
    Ok(scheme.category(&issue_type, &status).is_some())
}

/// The required fields `issue_id` lacks; none for an Issue that does not exist yet.
pub(crate) fn missing_fields(
    tx: &Transaction<'_>,
    scheme: &Scheme,
    issue_id: Uuid,
) -> Result<Vec<String>, StoreError> {
    let columns = ["type", "title", "description", "status", "assignee"];
    let row: Option<Vec<Option<String>>> = tx
        .query_row(
            "SELECT type, title, description, status, assignee, parent_id IS NOT NULL
             FROM issues WHERE id = ?1",
            [issue_id.as_bytes()],
            |row| {
                let mut values: Vec<Option<String>> = (0..columns.len())
                    .map(|i| row.get(i))
                    .collect::<Result<_, _>>()?;
                let has_parent: bool = row.get(columns.len())?;
                values.push(has_parent.then(String::new));
                Ok(values)
            },
        )
        .optional()?;
    let Some(values) = row else {
        return Ok(Vec::new());
    };
    let mut missing = Vec::new();
    for field in scheme.fields.iter().filter(|field| field.required) {
        let name = field.name.as_str();
        let present = if let Some(i) = columns.iter().position(|column| *column == name) {
            values[i]
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
        } else if name == "parent" {
            values[columns.len()].is_some()
        } else if field.field_type.is_multi() {
            has_member(tx, issue_id, name)?
        } else if field.builtin {
            // Priority and archived always hold a value; computed fields are not set.
            true
        } else {
            has_value(tx, issue_id, name)?
        };
        if !present {
            missing.push(field.name.clone());
        }
    }
    Ok(missing)
}

fn settable<'a>(scheme: &'a Scheme, name: &str) -> Result<&'a Field, StoreError> {
    let def = scheme.field(name).ok_or_else(|| StoreError::UnknownField {
        field: name.to_owned(),
        valid: scheme
            .fields
            .iter()
            .filter(|field| !field.derived)
            .map(|field| field.name.clone())
            .collect(),
    })?;
    if def.derived {
        return Err(invalid(def, "is computed and cannot be set", []));
    }
    Ok(def)
}

/// Checks a set element and returns its stored form and display label. Labels
/// compare without case, so they are stored lowercased and keep the spelling
/// they were written with as their display form.
fn member<'a>(
    scheme: &'a Scheme,
    field: &str,
    elem: &str,
) -> Result<(&'a Field, String, Option<String>), StoreError> {
    let def = settable(scheme, field)?;
    match def.field_type {
        FieldType::Labels => {
            let written = elem.trim();
            if written.is_empty() {
                return Err(invalid(def, "cannot hold an empty label", []));
            }
            if written.chars().any(char::is_control) {
                return Err(invalid(def, "cannot hold control characters", []));
            }
            let stored = written.to_lowercase();
            let display = (stored != written).then(|| written.to_owned());
            Ok((def, stored, display))
        }
        FieldType::MultiSelect => {
            let choice = def
                .option(elem)
                .ok_or_else(|| invalid(def, "has no such option", option_ids(def)))?;
            Ok((def, elem.to_owned(), Some(choice.label.clone())))
        }
        _ => Err(invalid(def, "holds one value; set it", [])),
    }
}

fn check_value(
    tx: &Transaction<'_>,
    scheme: &Scheme,
    issue_id: Uuid,
    def: &Field,
    value: &Value,
) -> Result<Option<String>, StoreError> {
    let text = || value.as_str().filter(|text| !text.trim().is_empty());
    match (def.field_type, value) {
        (_, Value::Null)
        | (FieldType::Number, Value::Number(_))
        | (FieldType::Boolean, Value::Bool(_)) => Ok(None),
        (FieldType::IssueType, Value::String(id)) => {
            let issue_type = scheme.issue_type(id).ok_or_else(|| {
                invalid(
                    def,
                    "is not a type",
                    scheme.types.iter().map(|t| t.id.clone()),
                )
            })?;
            let workflow = scheme
                .workflow(id)
                .ok_or_else(|| invalid(def, "has no Workflow in the Scheme", []))?;
            // Refused only when a valid Status would become invalid, so an Issue
            // whose type and Status are both unknown can be repaired by its type.
            if let Some(status) = current(tx, issue_id, "status")? {
                let valid_now = current(tx, issue_id, "type")?
                    .and_then(|current_type| scheme.workflow(&current_type))
                    .is_some_and(|current| current.status(&status).is_some());
                if valid_now && workflow.status(&status).is_none() {
                    return Err(invalid(
                        def,
                        "would leave the current Status outside the type's Workflow",
                        [],
                    ));
                }
            }
            Ok(Some(issue_type.label.clone()))
        }
        (FieldType::Status, Value::String(id)) => {
            let issue_type = current(tx, issue_id, "type")?
                .ok_or_else(|| invalid(def, "needs the Issue's type to be set first", []))?;
            let workflow = scheme.workflow(&issue_type).ok_or_else(|| {
                invalid(
                    def,
                    "needs a type the Scheme defines; set the type first",
                    [],
                )
            })?;
            let from = current(tx, issue_id, "status")?;
            if !workflow.allows(from.as_deref(), id) {
                return Err(invalid(
                    def,
                    "is not allowed from the current Status",
                    workflow.statuses.iter().map(|status| status.id.clone()),
                ));
            }
            Ok(workflow.status(id).map(|status| status.label.clone()))
        }
        (FieldType::Priority, Value::String(id)) => scheme
            .priority(id)
            .filter(|_| priority_rank(id).is_some())
            .map(|level| Some(level.label.clone()))
            .ok_or_else(|| {
                invalid(
                    def,
                    "is not a priority level",
                    scheme.priorities.iter().map(|level| level.id.clone()),
                )
            }),
        (FieldType::SingleSelect, Value::String(id)) => def
            .option(id)
            .map(|choice| Some(choice.label.clone()))
            .ok_or_else(|| invalid(def, "has no such option", option_ids(def))),
        (FieldType::Text | FieldType::RichText | FieldType::Person, Value::String(_)) => text()
            .map(|_| None)
            .ok_or_else(|| invalid(def, "cannot be blank", [])),
        (FieldType::IssueReference, Value::String(id)) => {
            let target = Uuid::parse_str(id).map_err(|_| invalid(def, "is not an Issue id", []))?;
            if target == issue_id {
                return Err(invalid(def, "cannot refer to the Issue itself", []));
            }
            if !exists(tx, target)? {
                return Err(StoreError::UnknownIssue(target));
            }
            if def.name == "parent"
                && let Some(path) = ancestry_reaches(tx, target, issue_id)?
            {
                return Err(StoreError::Cycle {
                    relation: "parent",
                    path: [issue_id].into_iter().chain(path).collect(),
                });
            }
            Ok(None)
        }
        (FieldType::Date, Value::String(date)) if is_date(date) => Ok(None),
        (FieldType::Date, Value::String(_)) => {
            Err(invalid(def, "is not a date written YYYY-MM-DD", []))
        }
        (FieldType::Datetime, Value::String(time)) => time
            .parse::<jiff::Timestamp>()
            .map(|_| None)
            .map_err(|_| invalid(def, "is not an RFC 3339 timestamp", [])),
        _ => Err(invalid(def, "has the wrong value type", [])),
    }
}

/// The parent chain from `start` up to `goal`, both included, if `goal` is an
/// ancestor of `start`. Stops at a cycle a merge left among the ancestors.
fn ancestry_reaches(
    tx: &Transaction<'_>,
    start: Uuid,
    goal: Uuid,
) -> Result<Option<Vec<Uuid>>, StoreError> {
    shortest_path(
        tx,
        "SELECT parent_id FROM issues WHERE id = ?1 AND parent_id IS NOT NULL",
        start,
        goal,
    )
}

fn current(
    tx: &Transaction<'_>,
    issue_id: Uuid,
    column: &str,
) -> Result<Option<String>, StoreError> {
    Ok(tx
        .query_row(
            &format!("SELECT {column} FROM issues WHERE id = ?1"),
            [issue_id.as_bytes()],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

fn exists(tx: &Transaction<'_>, issue_id: Uuid) -> Result<bool, StoreError> {
    Ok(tx
        .query_row(
            "SELECT 1 FROM issues WHERE id = ?1",
            [issue_id.as_bytes()],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn option_ids(def: &Field) -> Vec<String> {
    def.options.iter().map(|choice| choice.id.clone()).collect()
}

fn invalid(def: &Field, reason: &str, valid: impl IntoIterator<Item = String>) -> StoreError {
    StoreError::Invalid {
        field: def.name.clone(),
        reason: reason.to_owned(),
        valid: valid.into_iter().collect(),
    }
}
