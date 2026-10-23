//! Short-lists (ADR 0016): applying their operations, the checks a write of
//! one meets, and reading them.

use std::collections::BTreeSet;

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use serde_json::json;
use tm_core::shortlist::{self as core, Field};
use tm_core::{Actor, Change, Op, ProjectId, Subject};
use uuid::Uuid;

use crate::apply::{Version, Versions, project_from, record_in, sql_int, uuid_from, wins_in};
use crate::{Draft, Store, StoreError};

/// A Short-list as read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Shortlist {
    pub id: Uuid,
    pub project: ProjectId,
    /// None only when a merge brought the list without its name.
    pub name: Option<String>,
    pub size: u64,
    /// As [`core::actor_key`] files them.
    pub actors: Vec<String>,
    pub deleted: bool,
}

/// What [`Store::edit_shortlist`] changes; whatever is left empty stays.
#[derive(Debug, Clone, Default)]
pub struct ShortlistEdit<'a> {
    pub name: Option<&'a str>,
    pub size: Option<u64>,
    pub add: &'a [String],
    pub remove: &'a [String],
}

impl Store {
    /// Creates a Short-list in `project` worked through by `actors`, in one
    /// batch; `size` defaults to [`core::DEFAULT_SIZE`].
    ///
    /// # Errors
    ///
    /// [`StoreError::Invalid`] for no Actors, a malformed name or size, or a
    /// name another list in the Project holds; or as [`Store::append_all`].
    pub fn create_shortlist(
        &mut self,
        actor: &Actor,
        project: ProjectId,
        name: &str,
        actors: &[String],
        size: Option<u64>,
    ) -> Result<(Uuid, Vec<Op>), StoreError> {
        if actors.is_empty() {
            return Err(no_actors());
        }
        let id = Uuid::now_v7();
        let mut changes = vec![
            Change::CreateShortlist,
            set(Field::Name, json!(name)),
            set(Field::Size, json!(size.unwrap_or(core::DEFAULT_SIZE))),
        ];
        changes.extend(actors.iter().map(|who| actor_change(who, true)));
        let ops = self.append_all(drafts(actor, project, id, changes))?;
        Ok((id, ops))
    }

    /// Changes a Short-list's name, size or Actors in one batch; an empty
    /// edit writes nothing.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownShortlist`] for a missing or deleted list,
    /// [`StoreError::Invalid`] as for [`Store::create_shortlist`] or for
    /// removing its last Actor, or as [`Store::append_all`].
    pub fn edit_shortlist(
        &mut self,
        actor: &Actor,
        id: Uuid,
        edit: &ShortlistEdit<'_>,
    ) -> Result<Vec<Op>, StoreError> {
        let project = live(&self.conn, id)?;
        let mut changes = Vec::new();
        changes.extend(edit.name.map(|name| set(Field::Name, json!(name))));
        changes.extend(edit.size.map(|size| set(Field::Size, json!(size))));
        // Adds first, so swapping one Actor for another never leaves none.
        changes.extend(edit.add.iter().map(|who| actor_change(who, true)));
        changes.extend(edit.remove.iter().map(|who| actor_change(who, false)));
        self.append_all(drafts(actor, project, id, changes))
    }

    /// Deletes a Short-list, keeping its record so an undo restores it.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownShortlist`] for a missing or deleted list, or as
    /// [`Store::append_all`].
    pub fn delete_shortlist(&mut self, actor: &Actor, id: Uuid) -> Result<Vec<Op>, StoreError> {
        let project = live(&self.conn, id)?;
        let changes = vec![set(Field::Deleted, json!(true))];
        self.append_all(drafts(actor, project, id, changes))
    }

    /// The lists in `project` that are not deleted, oldest first.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn shortlists(&self, project: ProjectId) -> Result<Vec<Shortlist>, StoreError> {
        self.read(|store| {
            let mut stmt = store.conn.prepare_cached(
                "SELECT id FROM shortlists WHERE project = ?1 AND deleted = 0
                 ORDER BY created_hlc, id",
            )?;
            let ids = stmt
                .query_map([project.to_string()], |row| row.get::<_, Vec<u8>>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids.iter()
                .map(|id| {
                    let id = uuid_from(id)?;
                    read(&store.conn, id)?.ok_or(StoreError::UnknownShortlist(id))
                })
                .collect()
        })
    }

    /// A Short-list by id, deleted or not.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn shortlist(&self, id: Uuid) -> Result<Option<Shortlist>, StoreError> {
        self.read(|store| read(&store.conn, id))
    }
}

fn actor_change(who: &str, present: bool) -> Change {
    Change::ShortlistActor {
        actor: core::actor_key(who),
        present,
    }
}

/// The Project of a list a write may change: one that exists and is not deleted.
fn live(conn: &Connection, id: Uuid) -> Result<ProjectId, StoreError> {
    match read(conn, id)? {
        Some(list) if !list.deleted => Ok(list.project),
        _ => Err(StoreError::UnknownShortlist(id)),
    }
}

/// The checks a batch's end state meets, which [`check`] cannot make draft
/// by draft: a list the batch creates has a name and an Actor, and every
/// live list it wrote holds a name no other live list in its Project holds,
/// so an undo may move names between lists in any order.
pub(crate) fn check_batch(tx: &Transaction<'_>, ops: &[Op]) -> Result<(), StoreError> {
    let written: BTreeSet<Uuid> = ops
        .iter()
        .filter(|op| {
            op.change()
                .is_ok_and(|change| change.subject() == Subject::Shortlist)
        })
        .map(|op| op.subject)
        .collect();
    for id in written {
        let list = read(tx, id)?.ok_or(StoreError::UnknownShortlist(id))?;
        let created = ops
            .iter()
            .any(|op| op.subject == id && op.kind == "create_shortlist");
        if created && list.name.is_none() {
            return Err(StoreError::Invalid {
                field: Field::Name.as_str().into(),
                reason: "is needed for a new Short-list".into(),
                valid: Vec::new(),
            });
        }
        if created && list.actors.is_empty() {
            return Err(no_actors());
        }
        if let (false, Some(name)) = (list.deleted, &list.name) {
            name_free(tx, list.project, name, id)?;
        }
    }
    Ok(())
}

fn set(field: Field, value: serde_json::Value) -> Change {
    Change::SetShortlist { field, value }
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

fn no_actors() -> StoreError {
    StoreError::Invalid {
        field: core::ACTORS.into(),
        reason: "needs at least one Actor to work through it".into(),
        valid: Vec::new(),
    }
}

pub(crate) fn read(conn: &Connection, id: Uuid) -> Result<Option<Shortlist>, StoreError> {
    let row = conn
        .query_row(
            "SELECT project, name, size, deleted FROM shortlists WHERE id = ?1",
            [id.as_bytes()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((project, name, size, deleted)) = row else {
        return Ok(None);
    };
    let mut stmt = conn.prepare_cached(
        "SELECT actor FROM shortlist_actors WHERE shortlist_id = ?1 ORDER BY actor",
    )?;
    let actors = stmt
        .query_map([id.as_bytes()], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(Some(Shortlist {
        id,
        project: project_from(&project)?,
        name,
        size: crate::apply::rust_int(size)?,
        actors,
        deleted,
    }))
}

pub(crate) fn project_of(conn: &Connection, id: Uuid) -> Result<Option<ProjectId>, StoreError> {
    let text: Option<String> = conn
        .query_row(
            "SELECT project FROM shortlists WHERE id = ?1",
            [id.as_bytes()],
            |row| row.get(0),
        )
        .optional()?;
    text.map(|text| project_from(&text)).transpose()
}

/// Applies a Short-list operation. A merge may leave two lists sharing a
/// name or one with no Actors; only a write refuses those.
pub(crate) fn apply(
    tx: &Transaction<'_>,
    op: &Op,
    change: &Change,
    version: &Version,
) -> Result<(), StoreError> {
    if matches!(change, Change::CreateShortlist) {
        if !crate::project::exists(tx, op.project)? {
            return Err(StoreError::UnknownProject(op.project));
        }
        if project_of(tx, op.subject)?.is_some() {
            return Err(StoreError::DuplicateShortlist(op.subject));
        }
        tx.execute(
            "INSERT INTO shortlists (id, project, created_hlc) VALUES (?1, ?2, ?3)",
            params![
                op.subject.as_bytes(),
                op.project.to_string(),
                sql_int(op.hlc.raw())?
            ],
        )?;
        return Ok(());
    }
    let expected = project_of(tx, op.subject)?.ok_or(StoreError::UnknownShortlist(op.subject))?;
    if expected != op.project {
        return Err(StoreError::ShortlistMismatch {
            list: op.subject,
            expected,
            found: op.project,
        });
    }
    let list = op.subject;
    let (field, elem, present) = match change {
        Change::SetShortlist { field, .. } => (field.as_str(), "", true),
        Change::ShortlistActor { actor, present } => (core::ACTORS, actor.as_str(), *present),
        _ => unreachable!("only Short-list kinds reach here"),
    };
    // Checked before the version, so a value that does not fit changes nothing.
    let stored = match change {
        Change::SetShortlist { field, value } => Some(column_value(*field, value)?),
        _ => None,
    };
    if !wins_in(tx, Versions::Shortlist, list, field, elem, version)? {
        return Ok(());
    }
    record_in(
        tx,
        Versions::Shortlist,
        list,
        (field, elem),
        present,
        version,
    )?;
    let sql = match stored {
        Some(_) => format!("UPDATE shortlists SET {field} = ?2 WHERE id = ?1"),
        None if present => {
            "INSERT OR IGNORE INTO shortlist_actors (shortlist_id, actor) VALUES (?1, ?2)".into()
        }
        None => "DELETE FROM shortlist_actors WHERE shortlist_id = ?1 AND actor = ?2".into(),
    };
    let value = stored.unwrap_or_else(|| SqlValue::Text(elem.to_owned()));
    tx.execute(&sql, params![list.as_bytes(), value])?;
    Ok(())
}

fn column_value(field: Field, value: &serde_json::Value) -> Result<SqlValue, StoreError> {
    match field {
        Field::Name => value.as_str().map(|name| SqlValue::Text(name.to_owned())),
        Field::Size => value.as_i64().map(SqlValue::Integer),
        Field::Deleted => value
            .as_bool()
            .map(|deleted| SqlValue::Integer(deleted.into())),
    }
    .ok_or_else(|| StoreError::Corrupt(format!("Short-list {} value {value}", field.as_str())))
}

/// The checks a write of a Short-list operation meets, which replay does not
/// repeat: a list written to exists and is not deleted, unless the write
/// restores it, and keeps an Actor. Names are checked by [`check_batch`].
pub(crate) fn check(tx: &Transaction<'_>, id: Uuid, change: Change) -> Result<Change, StoreError> {
    if matches!(change, Change::CreateShortlist) {
        if project_of(tx, id)?.is_some() {
            return Err(StoreError::DuplicateShortlist(id));
        }
        return Ok(change);
    }
    let list = read(tx, id)?.ok_or(StoreError::UnknownShortlist(id))?;
    let restoring = matches!(
        &change,
        Change::SetShortlist { field: Field::Deleted, value } if value == &json!(false)
    );
    if list.deleted && !restoring {
        return Err(StoreError::UnknownShortlist(id));
    }
    if let Change::ShortlistActor {
        actor,
        present: false,
    } = &change
        && list.actors == [actor.as_str()]
    {
        return Err(no_actors());
    }
    Ok(change)
}

/// Refuses `name` when another live list in `project` holds it, compared
/// without case.
fn name_free(
    tx: &Transaction<'_>,
    project: ProjectId,
    name: &str,
    except: Uuid,
) -> Result<(), StoreError> {
    let taken: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM shortlists
         WHERE project = ?1 AND deleted = 0 AND id <> ?2 AND name = ?3 COLLATE NOCASE)",
        params![project.to_string(), except.as_bytes(), name],
        |row| row.get(0),
    )?;
    if taken {
        return Err(StoreError::Invalid {
            field: Field::Name.as_str().into(),
            reason: format!("{name:?} is taken by another Short-list in this Project"),
            valid: Vec::new(),
        });
    }
    Ok(())
}
