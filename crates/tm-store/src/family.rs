//! Parents and children: the Issues above and below one by `parent`.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension};
use tm_core::scheme::Category;
use tm_core::{Actor, Scheme};
use uuid::Uuid;

use crate::apply::uuid_from;
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
    /// Moved from `from` into `to`, the first active Status of its Workflow.
    Started { from: String, to: String },
    /// Archived, which wins over a Claim on it.
    Archived,
    /// Another session's live Claim, left to its holder.
    Held(Claim),
    /// Its Issue type, whose Workflow has no active Status.
    NoActiveStatus(String),
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
            .and_then(|workflow| workflow.of_category(Category::Active).next());
        let held = lineage
            .held(parent.id)
            .filter(|holder| actor.is_agent() && !holder.taken_by(actor));
        let outcome = if parent.fields.get("archived") == Some(&serde_json::json!(true)) {
            ParentOutcome::Archived
        } else if let Some(holder) = held {
            ParentOutcome::Held(holder.clone())
        } else if let Some(to) = to {
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
        } else {
            ParentOutcome::NoActiveStatus(issue_type.to_owned())
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
