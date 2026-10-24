//! Putting Issues on a Short-list, moving them in it and taking them off
//! (ADR 0016). Each command is one batch, so one undo reverts it, planned
//! inside its write so a concurrent write cannot change what it read.

use rusqlite::Connection;
use serde::Serialize;
use serde_json::{Value, json};
use tm_core::filter::{Context, parse, resolve};
use tm_core::link::BLOCKS;
use tm_core::scheme::{Category, Scheme};
use tm_core::{Actor, Change, Op, ProjectId, rank};
use uuid::Uuid;

use crate::apply::uuid_from;
use crate::read::IssueView;
use crate::{Draft, Named, Shortlist, Store, StoreError, query};

/// Where Issues go on a Short-list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    End,
    Top,
    Before(Uuid),
    After(Uuid),
}

/// Why `tm next` passes over a member: it stays on the list, marked
/// (ADR 0016).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PassedOver {
    Blocked,
    Claimed,
    Started,
    Archived,
    Epic,
    Triage,
}

/// A batch that placed Issues or took them off, with the list each was on as
/// `(issue, list)`, read inside the write so a concurrent write cannot change
/// it. An add leaves out Issues already on the list it places them on.
#[derive(Debug, Clone, Default)]
pub struct Placed {
    pub ops: Vec<Op>,
    pub from: Vec<(Uuid, Uuid)>,
}

/// A Short-list member as read, in the list's order.
#[derive(Debug, Clone)]
pub struct Member {
    pub issue: IssueView,
    /// Empty when `tm next` could take it.
    pub passed_over: Vec<PassedOver>,
}

impl Store {
    /// The live lists in `project` that `actor` works through, oldest first:
    /// an Agent's name matches that Agent whoever it acts for, and a human's
    /// matches that human (ADR 0016).
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn shortlists_of(
        &self,
        project: ProjectId,
        actor: &Actor,
    ) -> Result<Vec<Shortlist>, StoreError> {
        let name = tm_core::shortlist::actor_key(actor.name());
        Ok(self
            .shortlists(project)?
            .into_iter()
            .filter(|list| list.actors.contains(&name))
            .collect())
    }

    /// `list`'s members in order, each with why `tm next` would pass it over.
    ///
    /// # Errors
    ///
    /// [`StoreError::Clock`], any SQLite failure, or [`StoreError::Corrupt`]
    /// for a malformed row.
    pub fn shortlist_view(&self, list: Uuid) -> Result<Vec<Member>, StoreError> {
        self.read(|store| {
            let order: Vec<Uuid> = ranked(&store.conn, list)?
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            store.eligibility(&order)
        })
    }

    /// `issues` in order, each with why `tm next` would pass it over, as
    /// [`Store::take_next`] judges them.
    ///
    /// # Errors
    ///
    /// [`StoreError::Clock`], any SQLite failure, or [`StoreError::Corrupt`]
    /// for a malformed row.
    pub fn eligibility(&self, issues: &[Uuid]) -> Result<Vec<Member>, StoreError> {
        let now_ms = self.utc_ms()?;
        self.read(|store| {
            let views = store.issues(issues)?;
            let blocks = open_blocks(&store.conn, &store.scheme)?;
            let mut members = Vec::with_capacity(views.len());
            for view in views {
                let passed_over = passed_over(&store.conn, &view, &blocks, now_ms)?;
                members.push(Member {
                    issue: view,
                    passed_over,
                });
            }
            Ok(members)
        })
    }
    /// The live Short-list in `project` named `name`, compared without case.
    ///
    /// # Errors
    ///
    /// [`StoreError::ShortlistNotFound`], [`StoreError::ShortlistNameShared`]
    /// when a merge left several lists with the name, or any SQLite failure.
    pub fn shortlist_named(&self, project: ProjectId, name: &str) -> Result<Shortlist, StoreError> {
        let mut named: Vec<Shortlist> = self
            .shortlists(project)?
            .into_iter()
            .filter(|list| {
                list.name
                    .as_deref()
                    .is_some_and(|held| held.eq_ignore_ascii_case(name))
            })
            .collect();
        match named.len() {
            0 => Err(StoreError::ShortlistNotFound(name.to_owned())),
            1 => Ok(named.remove(0)),
            count => Err(StoreError::ShortlistNameShared {
                name: name.to_owned(),
                count,
            }),
        }
    }

    /// Puts `issues` on `list` at `at`, in the order given, in one batch; an
    /// Issue on another list leaves it, and one already on this list moves.
    /// A placement that changes nothing writes nothing.
    ///
    /// # Errors
    ///
    /// [`StoreError::CannotJoin`] for an epic, an Issue in triage, archived,
    /// done or cancelled, or one whose place breaks blocker order;
    /// [`StoreError::ShortlistFull`]; [`StoreError::Invalid`] for a place
    /// beside an Issue not on the list; or as [`Store::append_all`].
    pub fn shortlist_add(
        &mut self,
        actor: &Actor,
        list: Uuid,
        issues: &[Uuid],
        at: Place,
    ) -> Result<Placed, StoreError> {
        let (scheme, writer) = (self.scheme.clone(), actor.clone());
        let mut from = Vec::new();
        let ops = self.append_with(
            Vec::new(),
            |tx, _| {
                let list = live(tx, list)?;
                let drafts = placing(tx, &scheme, &writer, &list, issues, at, true)?;
                for issue in distinct(issues) {
                    match list_of(tx, &scheme, issue) {
                        Ok(was) if was != list.id => from.push((issue, was)),
                        Ok(_) | Err(StoreError::NotOnShortlist(_)) => {}
                        Err(err) => return Err(err),
                    }
                }
                Ok(drafts)
            },
            true,
            None,
        )?;
        Ok(Placed { ops, from })
    }

    /// Moves `issue` to `at` on the list it is on.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotOnShortlist`], [`StoreError::CannotJoin`] when the
    /// place breaks blocker order, or as [`Store::shortlist_add`].
    pub fn shortlist_move(
        &mut self,
        actor: &Actor,
        issue: Uuid,
        at: Place,
    ) -> Result<Placed, StoreError> {
        let (scheme, writer) = (self.scheme.clone(), actor.clone());
        let mut from = Vec::new();
        let ops = self.append_with(
            Vec::new(),
            |tx, _| {
                let list = live(tx, list_of(tx, &scheme, issue)?)?;
                from.push((issue, list.id));
                placing(tx, &scheme, &writer, &list, &[issue], at, false)
            },
            true,
            None,
        )?;
        Ok(Placed { ops, from })
    }

    /// Takes `issues` off their lists in one batch.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotOnShortlist`] for an Issue on none, or as
    /// [`Store::append_all`].
    pub fn shortlist_remove(
        &mut self,
        actor: &Actor,
        issues: &[Uuid],
    ) -> Result<Placed, StoreError> {
        let (scheme, writer) = (self.scheme.clone(), actor.clone());
        let mut from = Vec::new();
        let ops = self.append_with(
            Vec::new(),
            |tx, _| {
                let mut drafts = Vec::new();
                for issue in distinct(issues) {
                    from.push((issue, list_of(tx, &scheme, issue)?));
                    let project = view(tx, &scheme, issue)?.project;
                    for field in ["shortlist", "shortlist_rank"] {
                        drafts.push(draft(&writer, project, issue, field, Value::Null));
                    }
                }
                Ok(drafts)
            },
            true,
            None,
        )?;
        Ok(Placed { ops, from })
    }
}

fn live(conn: &Connection, list: Uuid) -> Result<Shortlist, StoreError> {
    crate::shortlist::read(conn, list)?
        .filter(|read| !read.deleted)
        .ok_or(StoreError::UnknownShortlist(list))
}

fn view(conn: &Connection, scheme: &Scheme, issue: Uuid) -> Result<IssueView, StoreError> {
    crate::read::issues(conn, scheme, &[issue])?
        .pop()
        .ok_or(StoreError::UnknownIssue(issue))
}

/// The list `issue` reads as a member of.
fn list_of(conn: &Connection, scheme: &Scheme, issue: Uuid) -> Result<Uuid, StoreError> {
    view(conn, scheme, issue)?
        .fields
        .get("shortlist")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or_else(|| StoreError::NotOnShortlist(issue.into()))
}

fn key(conn: &Connection, issue: Uuid) -> Result<String, StoreError> {
    Ok(crate::project::key_of(conn, issue)?.unwrap_or_else(|| issue.to_string()))
}

/// The writes that put `issues` at `at` on `list`. A place with no rank
/// left between its neighbours, as ties from concurrent writes or the rank
/// cap leave, respaces the whole list in the same batch.
fn placing(
    conn: &Connection,
    scheme: &Scheme,
    actor: &Actor,
    list: &Shortlist,
    issues: &[Uuid],
    at: Place,
    adding: bool,
) -> Result<Vec<Draft>, StoreError> {
    let issues = distinct(issues);
    if issues.is_empty() {
        return Err(invalid("issues", "names no Issue to place".into()));
    }
    let members = ranked(conn, list.id)?;
    let rest: Vec<&(Uuid, String)> = members
        .iter()
        .filter(|(id, _)| !issues.contains(id))
        .collect();
    let at = match at {
        Place::End => rest.len(),
        Place::Top => 0,
        Place::Before(beside) | Place::After(beside) => {
            if issues.contains(&beside) {
                let reason = "names an Issue being placed; place it beside another".into();
                return Err(invalid("place", reason));
            }
            let Some(index) = rest.iter().position(|(id, _)| *id == beside) else {
                if crate::apply::issue_project(conn, beside)?.is_none() {
                    return Err(StoreError::UnknownIssue(beside));
                }
                let name = list.name.as_deref().unwrap_or("this list");
                let reason = format!("names {}, which is not on {name}", key(conn, beside)?);
                return Err(invalid("place", reason));
            };
            index + usize::from(matches!(at, Place::After(_)))
        }
    };
    let views = crate::read::issues(conn, scheme, &issues)?;
    if let Some(missing) = issues.iter().find(|id| !views.iter().any(|v| v.id == **id)) {
        return Err(StoreError::UnknownIssue(*missing));
    }
    let joining: Vec<Uuid> = issues
        .iter()
        .copied()
        .filter(|id| !members.iter().any(|(member, _)| member == id))
        .collect();
    if adding {
        for view in views.iter().filter(|view| joining.contains(&view.id)) {
            eligible(view)?;
        }
    }
    let size = usize::try_from(list.size).unwrap_or(usize::MAX);
    if adding && !joining.is_empty() && rest.len() + issues.len() > size {
        return Err(StoreError::ShortlistFull {
            name: list.name.clone().unwrap_or_else(|| list.id.to_string()),
            size: list.size,
        });
    }
    let order: Vec<Uuid> = rest[..at]
        .iter()
        .map(|(id, _)| *id)
        .chain(issues.iter().copied())
        .chain(rest[at..].iter().map(|(id, _)| *id))
        .collect();
    let off_list_refused = if adding { joining.as_slice() } else { &[] };
    blocker_order(conn, scheme, &order, &views, off_list_refused)?;

    let mut drafts = Vec::new();
    for (issue, rank) in ranks(&rest, at, &issues, &order)? {
        let current = members.iter().find(|(id, _)| *id == issue);
        let project = views
            .iter()
            .find(|view| view.id == issue)
            .map_or(list.project, |view| view.project);
        let mut set = |field, value| drafts.push(draft(actor, project, issue, field, value));
        if current.is_none() {
            set("shortlist", json!(list.id.to_string()));
        }
        if current.is_none_or(|(_, held)| *held != rank) {
            set("shortlist_rank", json!(rank));
        }
    }
    Ok(drafts)
}

/// Refuses a place that puts an Issue before an open Issue that blocks it,
/// or after one it blocks, and an Issue in `off_list_refused` with an open
/// blocker off the list. Checked for the Issues placed only, so a member
/// that became blocked later stays where it is.
fn blocker_order(
    conn: &Connection,
    scheme: &Scheme,
    order: &[Uuid],
    placed: &[IssueView],
    off_list_refused: &[Uuid],
) -> Result<(), StoreError> {
    let blocks = open_blocks(conn, scheme)?;
    let index = |id: Uuid| order.iter().position(|member| *member == id);
    let refuse = |view: &IssueView, reason: String| StoreError::CannotJoin {
        issue: named(view),
        reason,
    };
    for view in placed {
        let at = index(view.id);
        for (blocker, _) in blocks.iter().filter(|(_, dst)| *dst == view.id) {
            match index(*blocker) {
                None if off_list_refused.contains(&view.id) => {
                    let key = key(conn, *blocker)?;
                    let reason = format!("its open blocker {key} is not on the list");
                    return Err(refuse(view, reason));
                }
                Some(before) if Some(before) > at => {
                    let key = key(conn, *blocker)?;
                    let reason = format!("it would come before its blocker {key}");
                    return Err(refuse(view, reason));
                }
                _ => {}
            }
        }
        for (_, blocked) in blocks.iter().filter(|(src, _)| *src == view.id) {
            if index(*blocked).is_some_and(|after| Some(after) < at) {
                let key = key(conn, *blocked)?;
                let reason = format!("it would come after {key}, which it blocks");
                return Err(refuse(view, reason));
            }
        }
    }
    Ok(())
}

/// Why `tm next` passes over `view` at `now_ms`, given the open `blocks`;
/// empty when it could take it. The marks `tm shortlist` shows and the
/// take read this one function, so they cannot disagree.
pub(crate) fn passed_over(
    conn: &Connection,
    view: &IssueView,
    blocks: &[(Uuid, Uuid)],
    now_ms: i64,
) -> Result<Vec<PassedOver>, StoreError> {
    let mut passed_over = Vec::new();
    if blocks.iter().any(|(_, blocked)| *blocked == view.id) {
        passed_over.push(PassedOver::Blocked);
    }
    let claim = crate::claim::current(conn, view.id)?;
    if claim.is_some_and(|claim| !claim.lapsed(now_ms)) {
        passed_over.push(PassedOver::Claimed);
    }
    match view.category {
        Some(Category::Active) => passed_over.push(PassedOver::Started),
        Some(Category::Triage) | None => passed_over.push(PassedOver::Triage),
        Some(Category::Todo | Category::Done | Category::Cancelled) => {}
    }
    if view.fields.get("archived") == Some(&json!(true)) {
        passed_over.push(PassedOver::Archived);
    }
    if view.fields.get("type").and_then(Value::as_str) == Some("epic") {
        passed_over.push(PassedOver::Epic);
    }
    Ok(passed_over)
}

/// `(blocker, blocked)` for each `blocks` Link whose blocker is open,
/// whatever the Issue it blocks.
pub(crate) fn open_blocks(
    conn: &Connection,
    scheme: &Scheme,
) -> Result<Vec<(Uuid, Uuid)>, StoreError> {
    let open = resolve(
        &parse("-category:done,cancelled")?,
        scheme,
        Context { me: None },
    )?;
    let (sql, params) = query::blocks_from_open(&open, BLOCKS);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;
    rows.map(|pair| {
        let (src, dst) = pair?;
        Ok((uuid_from(&src)?, uuid_from(&dst)?))
    })
    .collect()
}

/// Ranks for `issues` placed at `at` among `rest`, or, with no rank left
/// between their neighbours, fresh ranks for the whole of `order`.
fn ranks(
    rest: &[&(Uuid, String)],
    at: usize,
    issues: &[Uuid],
    order: &[Uuid],
) -> Result<Vec<(Uuid, String)>, StoreError> {
    let prev = at.checked_sub(1).map(|i| rest[i].1.as_str());
    let next = rest.get(at).map(|member| member.1.as_str());
    if let Some(placed) = rank::spread(prev, next, issues.len()) {
        return Ok(issues.iter().copied().zip(placed).collect());
    }
    let respaced = rank::spread(None, None, order.len())
        .ok_or_else(|| invalid("issues", "make the list too long to rank".into()))?;
    Ok(order.iter().copied().zip(respaced).collect())
}

/// An add's refusals (ADR 0016): an epic's work is its children, and the
/// rest are not work to do next.
fn eligible(view: &IssueView) -> Result<(), StoreError> {
    let reason = if view.fields.get("type").and_then(Value::as_str) == Some("epic") {
        "it is an epic; add its children instead"
    } else if view.fields.get("archived") == Some(&json!(true)) {
        "it is archived"
    } else {
        match view.category {
            Some(Category::Triage) | None => "it is in triage; accept it first",
            Some(Category::Done | Category::Cancelled) => "it is done or cancelled",
            Some(Category::Todo | Category::Active) => return Ok(()),
        }
    };
    Err(StoreError::CannotJoin {
        issue: named(view),
        reason: reason.into(),
    })
}

fn invalid(field: &str, reason: String) -> StoreError {
    StoreError::Invalid {
        field: field.into(),
        reason,
        valid: Vec::new(),
    }
}

fn named(view: &IssueView) -> Named {
    Named {
        id: view.id,
        key: view.key.clone(),
        source: None,
    }
}

/// `issues` without repeats, first mention kept.
fn distinct(issues: &[Uuid]) -> Vec<Uuid> {
    let mut seen = Vec::with_capacity(issues.len());
    for issue in issues {
        if !seen.contains(issue) {
            seen.push(*issue);
        }
    }
    seen
}

fn draft(actor: &Actor, project: ProjectId, issue: Uuid, field: &str, value: Value) -> Draft {
    Draft {
        actor: actor.clone(),
        project,
        subject: issue,
        change: Change::SetField {
            field: field.into(),
            label: String::new(),
            value,
            value_label: None,
        },
    }
}

/// A list's members with their ranks, in order.
pub(crate) fn ranked(conn: &Connection, list: Uuid) -> Result<Vec<(Uuid, String)>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT i.id, i.shortlist_rank FROM issues i
         JOIN shortlists l ON l.id = i.shortlist AND l.deleted = 0 AND l.project = i.project
         WHERE i.shortlist = ?1 AND i.shortlist_rank IS NOT NULL
         ORDER BY i.shortlist_rank, i.id",
    )?;
    let rows = stmt.query_map([list.as_bytes()], |row| {
        Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.map(|row| {
        let (id, rank) = row?;
        Ok((uuid_from(&id)?, rank))
    })
    .collect()
}
