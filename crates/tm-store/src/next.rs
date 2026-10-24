//! Taking the next Issue for `tm next` (ADR 0016): the first of an ordered
//! set that it may take, chosen inside the write.

use tm_core::op::ClaimAction;
use tm_core::scheme::Category;
use tm_core::{Actor, Change, Op};
use uuid::Uuid;

use crate::claim::{Claim, current, taken};
use crate::membership::{open_blocks, passed_over};
use crate::{Draft, IssueView, Store, StoreError};

/// The Issue [`Store::take_next`] took, with what its `extend` made.
#[derive(Debug)]
pub struct Next<T> {
    pub issue: Uuid,
    /// The Claim an Agent took; a human begins work without one.
    pub claim: Option<Claim>,
    pub ops: Vec<Op>,
    pub made: T,
}

impl Store {
    /// Takes the first of `candidates`, in order, that `tm next` may take:
    /// in the todo category and passed over for nothing. Chosen inside the
    /// write, so two sessions never take the same Issue and none takes one
    /// a human started meanwhile. An Agent claims it, for `ttl_ms`; a human
    /// begins it without a Claim. `extend` makes the drafts that begin it
    /// from it and its ancestors, as [`Store::take_with`] does. `Ok(None)`,
    /// writing nothing, when none may be taken.
    ///
    /// # Errors
    ///
    /// [`StoreError::Clock`], [`StoreError::Invalid`] for an expiry of zero
    /// or out of range, or as [`Store::append_all`].
    pub fn take_next<T: Default>(
        &mut self,
        actor: &Actor,
        candidates: &[Uuid],
        ttl_ms: Option<u64>,
        extend: impl FnOnce(&[IssueView]) -> (Vec<Draft>, T),
    ) -> Result<Option<Next<T>>, StoreError> {
        let scheme = self.scheme.clone();
        let writer = actor.clone();
        let mut extend = Some(extend);
        let mut chosen = None;
        let mut made = T::default();
        let ops = self.append_with(
            Vec::new(),
            |tx, at_ms| {
                let blocks = open_blocks(tx, &scheme)?;
                for view in crate::read::issues(tx, &scheme, candidates)? {
                    let todo = view.category == Some(Category::Todo);
                    if !todo || !passed_over(tx, &view, &blocks, at_ms)?.is_empty() {
                        continue;
                    }
                    let twice = || StoreError::Corrupt("tm next began two Issues".into());
                    let extend = extend.take().ok_or_else(twice)?;
                    let (begin, out) = extend(&crate::family::lineage(tx, &scheme, view.id)?);
                    made = out;
                    chosen = Some(view.id);
                    let mut drafts = Vec::with_capacity(begin.len() + 1);
                    if writer.is_agent() {
                        let prev = current(tx, view.id)?.map(|held| held.id);
                        drafts.push(Draft {
                            actor: writer.clone(),
                            project: view.project,
                            subject: view.id,
                            change: Change::Claim {
                                action: take(&writer, ttl_ms, prev),
                            },
                        });
                    }
                    drafts.extend(begin);
                    return Ok(drafts);
                }
                Ok(Vec::new())
            },
            true,
            None,
        )?;
        let Some(issue) = chosen else {
            return Ok(None);
        };
        let claim = ops
            .iter()
            .find(|op| {
                op.subject == issue
                    && matches!(
                        op.change(),
                        Ok(Change::Claim {
                            action: ClaimAction::Take { .. }
                        })
                    )
            })
            .map(|op| taken(op, actor.name(), ttl_ms, actor.session()))
            .transpose()?;
        Ok(Some(Next {
            issue,
            claim,
            ops,
            made,
        }))
    }
}

fn take(actor: &Actor, ttl_ms: Option<u64>, prev: Option<Uuid>) -> ClaimAction {
    ClaimAction::Take {
        agent: actor.name().to_owned(),
        ttl_ms,
        prev,
        session: actor.session().map(str::to_owned),
    }
}
