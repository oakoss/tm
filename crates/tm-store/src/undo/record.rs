//! What undo does differently for each kind of record it restores.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use tm_core::{Change, ProjectId, Scheme, Subject, memory, shortlist};
use uuid::Uuid;

use super::{Restore, Target, UndoPlan};
use crate::StoreError;
use crate::apply::issue_project;

/// The record a restore writes to, as read before it.
pub(super) enum Record {
    Issue,
    List(crate::shortlist::Shortlist),
    Memory(crate::memory::Memory),
}

impl Record {
    pub(super) fn read(conn: &Connection, subject: Subject, id: Uuid) -> Result<Self, StoreError> {
        Ok(match subject {
            Subject::Shortlist => Self::List(
                crate::shortlist::read(conn, id)?.ok_or(StoreError::UnknownShortlist(id))?,
            ),
            Subject::Memory => {
                Self::Memory(crate::memory::read(conn, id)?.ok_or(StoreError::UnknownMemory(id))?)
            }
            Subject::Issue | Subject::Project => Self::Issue,
        })
    }

    /// Why a write to `field` waits: a list deleted, or a memory forgotten,
    /// since takes no writes until that is undone.
    pub(super) fn gone(&self, field: &str) -> Option<String> {
        match self {
            Self::List(list) if list.deleted && field != shortlist::Field::Deleted.as_str() => {
                Some(format!(
                    "Short-list {} has been deleted since; undo its delete first",
                    list.name.as_deref().unwrap_or("without a name")
                ))
            }
            Self::Memory(memory)
                if memory.forgotten && field != memory::Field::Forgotten.as_str() =>
            {
                Some(format!(
                    "memory {} has been forgotten since; undo its forget first",
                    memory.key.as_deref().unwrap_or("without a key")
                ))
            }
            _ => None,
        }
    }

    pub(super) fn target(&self, id: Uuid) -> Target {
        match self {
            Self::List(list) => Target::Shortlist {
                shortlist: id,
                name: list.name.clone(),
            },
            Self::Memory(memory) => Target::Memory {
                memory: id,
                key: memory.key.clone(),
            },
            Self::Issue => Target::Issue { issue: id },
        }
    }

    /// What `field` (or its element `elem`) holds now.
    pub(super) fn state(
        &self,
        conn: &Connection,
        scheme: &Scheme,
        (id, field, elem): (Uuid, &str, &str),
    ) -> Result<Value, StoreError> {
        match self {
            Self::List(_) => list_state(conn, id, field, elem),
            Self::Memory(memory) => memory_state(memory, field),
            Self::Issue => super::current_state(conn, scheme, id, field, elem),
        }
    }

    /// The state `prior` left and the change that writes it back; none when
    /// a list or memory field held no value before to restore.
    pub(super) fn inverse(
        &self,
        (field, elem): (&str, &str),
        prior: Option<Change>,
        wrote_comment: bool,
    ) -> Result<Option<(Value, Change)>, StoreError> {
        Ok(match self {
            Self::List(_) => list_inverse(field, elem, prior),
            Self::Memory(_) => memory_inverse(field, prior),
            Self::Issue => Some(super::inverse(field, elem, prior, wrote_comment)?),
        })
    }

    /// What it is called in a refusal.
    pub(super) const fn kind(&self) -> &'static str {
        match self {
            Self::Issue => "Issue",
            Self::List(_) => "Short-list",
            Self::Memory(_) => "memory",
        }
    }

    pub(super) fn project(&self, conn: &Connection, id: Uuid) -> Result<ProjectId, StoreError> {
        match self {
            Self::List(list) => Ok(list.project),
            Self::Memory(memory) => Ok(memory.project),
            Self::Issue => issue_project(conn, id)?.ok_or(StoreError::UnknownIssue(id)),
        }
    }

    /// Undoes a created list or memory by deleting or forgetting it, which
    /// keeps its record and everything since; none when it already is.
    pub(super) fn retire(&self, id: Uuid, plan: &mut UndoPlan) {
        let (project, field, change) = match self {
            Self::List(list) if !list.deleted => (
                list.project,
                shortlist::Field::Deleted.as_str(),
                Change::SetShortlist {
                    field: shortlist::Field::Deleted,
                    value: json!(true),
                },
            ),
            Self::Memory(memory) if !memory.forgotten => (
                memory.project,
                memory::Field::Forgotten.as_str(),
                Change::SetMemory {
                    field: memory::Field::Forgotten,
                    value: json!(true),
                },
            ),
            _ => return,
        };
        plan.restores.push(Restore {
            target: self.target(id),
            field: field.into(),
            elem: None,
            from: json!(false),
            to: json!(true),
        });
        plan.changes.push((project, id, change));
    }
}

/// What a Short-list field holds now, or whether an Actor is on it.
fn list_state(conn: &Connection, list: Uuid, field: &str, elem: &str) -> Result<Value, StoreError> {
    if !elem.is_empty() {
        let present: Option<bool> = conn
            .query_row(
                "SELECT present FROM shortlist_versions
                 WHERE shortlist_id = ?1 AND field = ?2 AND elem = ?3",
                params![list.as_bytes(), field, elem],
                |row| row.get(0),
            )
            .optional()?;
        return Ok(json!(present.unwrap_or(false)));
    }
    let read = crate::shortlist::read(conn, list)?.ok_or(StoreError::UnknownShortlist(list))?;
    Ok(match field {
        "name" => json!(read.name),
        "size" => json!(read.size),
        "deleted" => json!(read.deleted),
        other => return Err(StoreError::Corrupt(format!("Short-list field {other:?}"))),
    })
}

fn memory_state(memory: &crate::memory::Memory, field: &str) -> Result<Value, StoreError> {
    Ok(match field {
        "key" => json!(memory.key),
        "body" => json!(memory.body),
        "forgotten" => json!(memory.forgotten),
        other => return Err(StoreError::Corrupt(format!("memory field {other:?}"))),
    })
}

/// [`super::inverse`] for a memory. With no prior winner it is not
/// forgotten; a key or body has no value to restore, as a create's own are
/// undone by forgetting it.
fn memory_inverse(field: &str, prior: Option<Change>) -> Option<(Value, Change)> {
    let unset = |field, value: Value| Some((value.clone(), Change::SetMemory { field, value }));
    match prior {
        Some(Change::SetMemory { field, value }) => unset(field, value),
        None if field == memory::Field::Forgotten.as_str() => {
            unset(memory::Field::Forgotten, json!(false))
        }
        _ => None,
    }
}

/// [`super::inverse`] for a Short-list. With no prior winner, the list is not
/// deleted, an Actor is absent and the size is the default; a name has no
/// value to restore, which a merge that brought the list without one leaves,
/// so none is returned and the batch cannot be undone.
fn list_inverse(field: &str, elem: &str, prior: Option<Change>) -> Option<(Value, Change)> {
    let unset = |field, value: Value| Some((value.clone(), Change::SetShortlist { field, value }));
    match prior {
        Some(Change::SetShortlist { field, value }) => unset(field, value),
        Some(change @ Change::ShortlistActor { present, .. }) => Some((json!(present), change)),
        None if field == shortlist::ACTORS => Some((
            json!(false),
            Change::ShortlistActor {
                actor: elem.to_owned(),
                present: false,
            },
        )),
        None if field == shortlist::Field::Deleted.as_str() => {
            unset(shortlist::Field::Deleted, json!(false))
        }
        None if field == shortlist::Field::Size.as_str() => {
            unset(shortlist::Field::Size, json!(shortlist::DEFAULT_SIZE))
        }
        _ => None,
    }
}
