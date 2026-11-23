//! Parents and children: the Issues above and below one by `parent`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension};
use tm_core::scheme::{Category, MoveGap};
use tm_core::{Actor, Change, Scheme};
use uuid::Uuid;

use crate::apply::uuid_from;
use crate::validate::Writing;
use crate::{Claim, Draft, IssueView, Op, Store, StoreError, WriteOptions, Written, claim, read};

impl Store {
    /// The Issues whose parent is `issue`, archived ones included.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn children(&self, issue: Uuid) -> Result<Vec<IssueView>, StoreError> {
        self.read(|store| {
            let mut stmt = store
                .conn
                .prepare_cached("SELECT id FROM issues WHERE parent_id = ?1")?;
            let ids = stmt
                .query_map([issue.as_bytes()], |row| row.get::<_, Vec<u8>>(0))?
                .map(|id| uuid_from(&id?))
                .collect::<Result<Vec<_>, _>>()?;
            read::issues(&store.conn, &store.scheme, &ids)
        })
    }

    /// [`Store::write`] with the drafts `extend` makes from `issue` and its
    /// ancestors, nearest first, read inside the same write so no other
    /// writer can change them in between. An ancestor that does not exist is
    /// left out, and the walk stops at an Issue it has seen, since a merge can
    /// leave a cycle.
    ///
    /// # Errors
    ///
    /// As [`Store::write`].
    pub fn append_with_lineage<T: Default>(
        &mut self,
        drafts: Vec<Draft>,
        issue: Uuid,
        options: &WriteOptions,
        extend: impl FnOnce(&Lineage) -> (Vec<Draft>, T),
    ) -> Result<(Written<Vec<Op>>, T), StoreError> {
        let scheme = self.scheme.clone();
        let mut made = T::default();
        let drafts = drafts.into_iter().map(|draft| (draft, None)).collect();
        let ops = self.write_with(
            drafts,
            options,
            |tx, at_ms| {
                let (more, out) = extend(&lineage(tx, &scheme, issue, at_ms)?);
                made = out;
                Ok(more)
            },
            true,
            None,
        )?;
        Ok((ops, made))
    }

    /// [`Store::write`] for a move of `issue` into an active Status by
    /// `actor`, starting its ancestors as [`Lineage::start_parents`] does,
    /// read inside the same write.
    ///
    /// # Errors
    ///
    /// As [`Store::write`].
    pub fn write_starting_parents(
        &mut self,
        actor: &Actor,
        drafts: Vec<Draft>,
        issue: Uuid,
        options: &WriteOptions,
    ) -> Result<(Written<Vec<Op>>, Vec<ParentStart>), StoreError> {
        let scheme = self.scheme.clone();
        self.append_with_lineage(drafts, issue, options, |lineage| {
            lineage.start_parents(&scheme, actor)
        })
    }
}

/// What starting one parent came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentStart {
    pub id: Uuid,
    pub key: Option<String>,
    pub outcome: ParentOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParentOutcome {
    /// Moved from `from` into `to`, the first active Status its Workflow
    /// allows from there.
    Started { from: String, to: String },
    /// Archived, which wins over a Claim on it.
    Archived,
    /// Another session's live Claim, left to its holder.
    Held(Claim),
    /// Its Issue type, whose Workflow has no active Status.
    NoActiveStatus(String),
    /// Its Status, from which its Workflow allows no active one.
    NoActiveMove(String),
    /// The active Status `to`, which requires `fields` it holds no value for.
    Lacking { to: String, fields: Vec<String> },
}

/// The moves starting each of `parents` still in triage or todo, as a child
/// entering an active Status does. An Agent leaves one another session's
/// Claim holds in `lineage` to its holder.
pub(crate) fn start(
    scheme: &Scheme,
    parents: &[IssueView],
    lineage: &Lineage,
    actor: &Actor,
) -> (Vec<Draft>, Vec<ParentStart>) {
    let mut drafts = Vec::new();
    let mut starts = Vec::new();
    for parent in parents {
        let status = parent
            .fields
            .get("status")
            .and_then(serde_json::Value::as_str);
        let (Some(Category::Triage | Category::Todo), Some(from)) = (parent.category, status)
        else {
            continue;
        };
        let issue_type = parent.issue_type().unwrap_or_default();
        let to = scheme
            .workflow(issue_type)
            .map_or(Err(MoveGap::NoStatus), |workflow| {
                workflow.first_move(Some(from), Category::Active)
            });
        let held = lineage
            .held(parent.id)
            .filter(|holder| actor.is_agent() && !holder.taken_by(actor));
        let outcome = if parent.fields.get("archived") == Some(&serde_json::json!(true)) {
            ParentOutcome::Archived
        } else if let Some(holder) = held {
            ParentOutcome::Held(holder.clone())
        } else {
            let lacking = to
                .as_ref()
                .map(|to| parent.lacking(scheme, &to.id))
                .unwrap_or_default();
            match to {
                Ok(to) if !lacking.is_empty() => ParentOutcome::Lacking {
                    to: to.id.clone(),
                    fields: lacking,
                },
                Ok(to) => {
                    drafts.push(Draft {
                        actor: actor.clone(),
                        project: parent.project,
                        subject: parent.id,
                        change: crate::set("status", serde_json::json!(to.id)),
                    });
                    ParentOutcome::Started {
                        from: from.to_owned(),
                        to: to.id.clone(),
                    }
                }
                Err(MoveGap::Unreachable(_)) => ParentOutcome::NoActiveMove(from.to_owned()),
                Err(MoveGap::NoStatus) => ParentOutcome::NoActiveStatus(issue_type.to_owned()),
            }
        };
        starts.push(ParentStart {
            id: parent.id,
            key: parent.key.clone(),
            outcome,
        });
    }
    (drafts, starts)
}

/// An Issue and its ancestors, nearest first, with the live Claims on them,
/// as read inside a write.
pub struct Lineage {
    issues: Vec<IssueView>,
    held: HashMap<Uuid, Claim>,
}

impl Lineage {
    /// The Issue, then its ancestors.
    #[must_use]
    pub fn issues(&self) -> &[IssueView] {
        &self.issues
    }

    /// The Claim holding `issue` at the write, unless it has lapsed.
    #[must_use]
    pub fn held(&self, issue: Uuid) -> Option<&Claim> {
        self.held.get(&issue)
    }

    /// The moves starting the Issue's ancestors, as its entering an active
    /// Status does.
    #[must_use]
    pub fn start_parents(&self, scheme: &Scheme, actor: &Actor) -> (Vec<Draft>, Vec<ParentStart>) {
        start(
            scheme,
            self.issues.get(1..).unwrap_or_default(),
            self,
            actor,
        )
    }
}

/// `issue` and its ancestors, nearest first, with the Claims holding them at
/// `at_ms`, as [`Store::append_with_lineage`] reads them.
pub(crate) fn lineage(
    conn: &Connection,
    scheme: &tm_core::Scheme,
    issue: Uuid,
    at_ms: i64,
) -> Result<Lineage, StoreError> {
    let mut ids = vec![issue];
    ids.extend(chain(conn, issue)?);
    let mut held = HashMap::new();
    for id in &ids {
        if let Some(claim) = claim::current(conn, *id)?.filter(|claim| !claim.lapsed(at_ms)) {
            held.insert(*id, claim);
        }
    }
    Ok(Lineage {
        issues: read::issues(conn, scheme, &ids)?,
        held,
    })
}

/// The ids above `issue`, nearest first.
fn chain(conn: &Connection, issue: Uuid) -> Result<Vec<Uuid>, StoreError> {
    let mut seen = HashSet::from([issue]);
    let mut chain = Vec::new();
    let mut at = issue;
    while let Some(parent) = parent(conn, at)? {
        if !seen.insert(parent) {
            break;
        }
        chain.push(parent);
        at = parent;
    }
    Ok(chain)
}

fn parent(conn: &Connection, issue: Uuid) -> Result<Option<Uuid>, StoreError> {
    conn.prepare_cached("SELECT parent_id FROM issues WHERE id = ?1")?
        .query_row([issue.as_bytes()], |row| row.get::<_, Option<Vec<u8>>>(0))
        .optional()?
        .flatten()
        .map(|id| uuid_from(&id))
        .transpose()
}

fn type_of(conn: &Connection, issue: Uuid) -> Result<Option<String>, StoreError> {
    Ok(conn
        .prepare_cached("SELECT type FROM issues WHERE id = ?1")?
        .query_row([issue.as_bytes()], |row| row.get::<_, Option<String>>(0))
        .optional()?
        .flatten())
}

/// Refuses `parent` for `child` of type `child_type`; `child` is `None` when
/// not yet created.
pub(crate) fn parent_type_allowed(
    conn: &Connection,
    scheme: &Scheme,
    (child, child_type): (Option<Uuid>, &str),
    parent: Uuid,
) -> Result<(), StoreError> {
    let Some(allowed) = scheme.parent_types(child_type) else {
        return Ok(());
    };
    let parent_type = type_of(conn, parent)?;
    if parent_type.as_ref().is_some_and(|t| allowed.contains(t)) {
        return Ok(());
    }
    Err(StoreError::ParentType(Box::new(crate::ParentType {
        child: child.map(Into::into),
        child_type: child_type.to_owned(),
        parent: parent.into(),
        parent_type,
        allowed: allowed.to_vec(),
    })))
}

/// Refuses `issue`'s children whose types do not list `issue_type`, skipping
/// those in `created`, which are checked as themselves.
fn children_allowed(
    conn: &Connection,
    scheme: &Scheme,
    (issue, issue_type): (Uuid, &str),
    created: &BTreeSet<Uuid>,
) -> Result<(), StoreError> {
    let children: Vec<(Vec<u8>, Option<String>)> = conn
        .prepare_cached("SELECT id, type FROM issues WHERE parent_id = ?1")?
        .query_map([issue.as_bytes()], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (child, child_type) in children {
        let child = uuid_from(&child)?;
        let Some(allowed) = child_type
            .as_deref()
            .filter(|_| !created.contains(&child))
            .and_then(|t| scheme.parent_types(t))
        else {
            continue;
        };
        if !allowed.iter().any(|t| t == issue_type) {
            return Err(StoreError::ParentType(Box::new(crate::ParentType {
                child: Some(child.into()),
                child_type: child_type.unwrap_or_default(),
                parent: issue.into(),
                parent_type: Some(issue_type.to_owned()),
                allowed: allowed.to_vec(),
            })));
        }
    }
    Ok(())
}

/// Refuses the parent types `issue` would have: `parent`'s, and its
/// children's when `new_type` retypes it.
pub(crate) fn types_allowed(
    conn: &Connection,
    scheme: &Scheme,
    issue: Uuid,
    new_type: Option<&str>,
    parent: Option<Uuid>,
) -> Result<(), StoreError> {
    let Some(issue_type) = new_type.map(str::to_owned).or(type_of(conn, issue)?) else {
        return Ok(());
    };
    if let Some(parent) = parent {
        parent_type_allowed(conn, scheme, (Some(issue), &issue_type), parent)?;
    }
    if new_type.is_some() {
        children_allowed(conn, scheme, (issue, &issue_type), &BTreeSet::new())?;
    }
    Ok(())
}

/// The Issues a batch gave a parent or a type, held to the Scheme's parent
/// types once the batch is written.
#[derive(Default)]
pub(crate) struct Family {
    /// Each Issue, and whether its type was written.
    written: BTreeMap<Uuid, bool>,
    created: BTreeSet<Uuid>,
}

impl Family {
    pub(crate) fn note(&mut self, issue: Uuid, change: &Change, writing: &Writing<'_>) {
        match change {
            Change::Create => {
                self.created.insert(issue);
            }
            Change::SetField { field, .. }
                if writing.checks_rules() && (field == "type" || field == "parent") =>
            {
                *self.written.entry(issue).or_default() |= field == "type";
            }
            _ => {}
        }
    }

    /// An Issue the batch created is refused as new, since the refusal rolls
    /// it back; its children got their parent in the same batch, so are
    /// checked as themselves.
    pub(crate) fn allowed(&self, conn: &Connection, scheme: &Scheme) -> Result<(), StoreError> {
        for (&issue, &retyped) in &self.written {
            let Some(issue_type) = type_of(conn, issue)? else {
                continue;
            };
            let named = (!self.created.contains(&issue)).then_some(issue);
            if let Some(parent) = parent(conn, issue)? {
                parent_type_allowed(conn, scheme, (named, &issue_type), parent)?;
            }
            if retyped && named.is_some() {
                children_allowed(conn, scheme, (issue, &issue_type), &self.created)?;
            }
        }
        Ok(())
    }
}
