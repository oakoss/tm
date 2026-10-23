//! Short-lists (ADR 0016): applying their operations, the checks a write of
//! one meets, and reading them.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use serde_json::json;
use tm_core::scheme::Category;
use tm_core::shortlist::{self as core, Field};
use tm_core::{Actor, Change, Op, ProjectId, Scheme, Subject};
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
        let writer = actor.clone();
        self.append_with(
            Vec::new(),
            |tx, _| {
                let list = read(tx, id)?
                    .filter(|list| !list.deleted)
                    .ok_or(StoreError::UnknownShortlist(id))?;
                let changes = edited(&list, edit)?;
                Ok(drafts(&writer, list.project, id, changes))
            },
            true,
            None,
        )
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

    /// The Issues on `list` by rank, ties by Issue id, so every Replica
    /// orders them alike; a deleted list has none.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn shortlist_members(&self, list: Uuid) -> Result<Vec<Uuid>, StoreError> {
        self.read(|store| {
            let mut stmt = store.conn.prepare_cached(
                "SELECT i.id FROM issues i
                 JOIN shortlists l ON l.id = i.shortlist AND l.deleted = 0 AND l.project = i.project
                 WHERE i.shortlist = ?1 AND i.shortlist_rank IS NOT NULL
                 ORDER BY i.shortlist_rank, i.id",
            )?;
            let ids = stmt
                .query_map([list.as_bytes()], |row| row.get::<_, Vec<u8>>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids.iter().map(|id| uuid_from(id)).collect()
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

/// The changes `edit` makes to `list`, leaving out what it already holds, so
/// an edit that changes nothing writes nothing. Removing an Actor the list
/// does not name is refused, as a mistyped name would otherwise pass.
fn edited(list: &Shortlist, edit: &ShortlistEdit<'_>) -> Result<Vec<Change>, StoreError> {
    for who in edit.remove {
        if !list.actors.contains(&core::actor_key(who)) {
            return Err(StoreError::Invalid {
                field: core::ACTORS.into(),
                // Quoted, as the name is not yet checked for control characters.
                reason: format!("does not name {who:?}"),
                valid: list.actors.clone(),
            });
        }
    }
    let mut changes = Vec::new();
    if let Some(name) = edit.name.filter(|name| list.name.as_deref() != Some(*name)) {
        changes.push(set(Field::Name, json!(name)));
    }
    if let Some(size) = edit.size.filter(|size| *size != list.size) {
        changes.push(set(Field::Size, json!(size)));
    }
    // Adds first, so swapping one Actor for another never leaves none.
    let held = |who: &String| list.actors.contains(&core::actor_key(who));
    changes.extend(
        edit.add
            .iter()
            .filter(|who| !held(who))
            .map(|who| actor_change(who, true)),
    );
    changes.extend(edit.remove.iter().map(|who| actor_change(who, false)));
    Ok(changes)
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
/// by draft: a list the batch creates has a name and an Actor, every live
/// list it wrote holds a name no other live list in its Project holds, so
/// an undo may move names between lists in any order, and every Issue it
/// wrote holds a list and a rank together or neither.
pub(crate) fn check_batch(
    tx: &Transaction<'_>,
    ops: &[Op],
    half_held: &BTreeSet<Uuid>,
) -> Result<(), StoreError> {
    let issues: BTreeSet<Uuid> = ops
        .iter()
        .filter(|op| {
            op.change()
                .is_ok_and(|change| change.subject() == Subject::Issue)
        })
        .map(|op| op.subject)
        .collect();
    // An Issue a merge left half on a list stays editable, as `no_worse` keeps
    // any Issue that arrived inconsistent, unless the batch writes its
    // membership; the read rule hides it.
    let membership = |op: &&Op| {
        op.change()
            .is_ok_and(|change| matches!(change.field(), Some("shortlist" | "shortlist_rank")))
    };
    let rewritten: BTreeSet<Uuid> = ops.iter().filter(membership).map(|op| op.subject).collect();
    let exempt: BTreeSet<Uuid> = half_held.difference(&rewritten).copied().collect();
    for issue in issues.difference(&exempt) {
        let (_, fields) = held(tx, *issue)?;
        if let [field] = fields[..] {
            let named = crate::project::key_of(tx, *issue)?.unwrap_or_else(|| issue.to_string());
            return Err(StoreError::Invalid {
                field: field.into(),
                reason: format!("of Issue {named} is held only with both a Short-list and a rank"),
                valid: Vec::new(),
            });
        }
    }
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

/// The writes that take Issues off their lists as a batch calls for: each
/// Issue it moved into a done or cancelled Status, not one already there,
/// and each member of a list it deleted. Each is written by whoever wrote
/// the operation that called for it, and logged, so history shows it and
/// undo restores it.
pub(crate) fn clearing(
    tx: &Transaction<'_>,
    scheme: &Scheme,
    ops: &[Op],
    was_finished: &BTreeSet<Uuid>,
) -> Result<Vec<Draft>, StoreError> {
    let mut issues: BTreeMap<Uuid, &Op> = BTreeMap::new();
    for op in ops {
        match op.change()? {
            change if change.subject() == Subject::Issue => {
                if was_finished.contains(&op.subject) || !finished(tx, scheme, op.subject)? {
                    continue;
                }
                // The Status or type write finished it; another write stands in
                // only until one is found.
                let finishing = matches!(change.field(), Some("status" | "type"));
                let cause = issues.entry(op.subject).or_insert(op);
                if finishing {
                    *cause = op;
                }
            }
            Change::SetShortlist {
                field: Field::Deleted,
                value: serde_json::Value::Bool(true),
            } => {
                for member in members(tx, op.subject)? {
                    issues.entry(member).or_insert(op);
                }
            }
            _ => {}
        }
    }
    let mut drafts = Vec::new();
    for (issue, cause) in issues {
        let (project, fields) = held(tx, issue)?;
        let writer = match &cause.on_behalf_of {
            Some(human) => Actor::agent(&cause.actor, human),
            None => Actor::human(&cause.actor),
        };
        drafts.extend(fields.into_iter().map(|field| Draft {
            actor: writer.clone(),
            project,
            subject: issue,
            change: Change::SetField {
                field: field.into(),
                label: String::new(),
                value: serde_json::Value::Null,
                value_label: None,
            },
        }));
    }
    Ok(drafts)
}

pub(crate) fn finished(
    tx: &Transaction<'_>,
    scheme: &Scheme,
    issue: Uuid,
) -> Result<bool, StoreError> {
    let row: Option<(Option<String>, Option<String>)> = tx
        .query_row(
            "SELECT type, status FROM issues WHERE id = ?1",
            [issue.as_bytes()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let category = row.and_then(|(issue_type, status)| scheme.category(&issue_type?, &status?));
    Ok(matches!(
        category,
        Some(Category::Done | Category::Cancelled)
    ))
}

/// Every Issue that names `list`, whether or not it reads as a member.
fn members(tx: &Transaction<'_>, list: Uuid) -> Result<Vec<Uuid>, StoreError> {
    let mut stmt = tx.prepare_cached("SELECT id FROM issues WHERE shortlist = ?1")?;
    let ids = stmt
        .query_map([list.as_bytes()], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    ids.iter().map(|id| uuid_from(id)).collect()
}

/// A membership field's stored value, which reads hide when its list is not
/// live; none for any other field.
pub(crate) fn stored(
    conn: &Connection,
    issue: Uuid,
    field: &str,
) -> Result<Option<serde_json::Value>, StoreError> {
    let column = match field {
        "shortlist" => "shortlist",
        "shortlist_rank" => "shortlist_rank",
        _ => return Ok(None),
    };
    let value: Option<SqlValue> = conn
        .query_row(
            &format!("SELECT {column} FROM issues WHERE id = ?1"),
            [issue.as_bytes()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(Some(match value {
        Some(SqlValue::Blob(list)) => json!(uuid_from(&list)?.to_string()),
        Some(SqlValue::Text(rank)) => json!(rank),
        _ => serde_json::Value::Null,
    }))
}

/// Whether `subject` is an Issue that stores one membership field without the
/// other, as a merge can leave it.
pub(crate) fn half_held(tx: &Transaction<'_>, subject: Uuid) -> Result<bool, StoreError> {
    let held: Option<i64> = tx
        .query_row(
            "SELECT (shortlist IS NOT NULL) + (shortlist_rank IS NOT NULL) FROM issues WHERE id = ?1",
            [subject.as_bytes()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(held == Some(1))
}

/// An Issue's Project and the membership fields it stores a value in.
fn held(tx: &Transaction<'_>, issue: Uuid) -> Result<(ProjectId, Vec<&'static str>), StoreError> {
    let (project, list, rank): (String, bool, bool) = tx.query_row(
        "SELECT project, shortlist IS NOT NULL, shortlist_rank IS NOT NULL FROM issues WHERE id = ?1",
        [issue.as_bytes()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let fields = [("shortlist", list), ("shortlist_rank", rank)]
        .into_iter()
        .filter_map(|(field, held)| held.then_some(field))
        .collect();
    Ok((project_from(&project)?, fields))
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
