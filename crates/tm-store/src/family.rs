//! Parents and children: the Issues above and below one by `parent`.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use crate::apply::uuid_from;
use crate::{Claim, Draft, IssueView, Op, Store, StoreError, claim, read};

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

    /// [`Store::append_all`] with the drafts `extend` makes from `issue` and
    /// its ancestors, nearest first, read inside the same write so no other
    /// writer can change them in between. An ancestor that does not exist is
    /// left out, and the walk stops at an Issue it has seen, since a merge can
    /// leave a cycle.
    ///
    /// # Errors
    ///
    /// As [`Store::append_all`].
    pub fn append_with_lineage<T: Default>(
        &mut self,
        drafts: Vec<Draft>,
        issue: Uuid,
        extend: impl FnOnce(&Lineage) -> (Vec<Draft>, T),
    ) -> Result<(Vec<Op>, T), StoreError> {
        let scheme = self.scheme.clone();
        let mut made = T::default();
        let drafts = drafts.into_iter().map(|draft| (draft, None)).collect();
        let ops = self.append_with(
            drafts,
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
