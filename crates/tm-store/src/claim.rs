//! Claims: a take succeeds only on an unclaimed Issue, and replay applies
//! takes and releases by compare-and-set on the Claim they saw (ADR 0009).

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use tm_core::op::{CLAIM_FIELD, ClaimAction};
use tm_core::{Actor, Change, Op};
use uuid::Uuid;

use crate::apply::{issue_project, uuid_from};
use crate::{Draft, Store, StoreError};

/// Begins the `unapplied` reason of a take or release that lost to another,
/// telling lost Claims apart from operations that do not fit.
pub(crate) const LOST: &str = "lost:";

/// The Claim on an Issue. `id` is the token a release must name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub id: Uuid,
    pub issue: Uuid,
    pub agent: String,
    /// The human the Claim is held for: the take's `on_behalf_of`, else its
    /// writer.
    pub on_behalf_of: String,
    pub expires_ms: Option<i64>,
}

impl Store {
    /// Claims `issue` for the Agent writing. Refused while any Claim holds it,
    /// even one under the same Agent name; [`Claim::id`] is the token to
    /// release it.
    ///
    /// # Errors
    ///
    /// [`StoreError::NoAgent`] when a human writes,
    /// [`StoreError::AlreadyClaimed`] naming the holder,
    /// [`StoreError::UnknownIssue`], [`StoreError::Invalid`] for an expiry of
    /// zero or out of range or an archived Issue, or as [`Store::append_all`].
    pub fn take(
        &mut self,
        actor: &Actor,
        issue: Uuid,
        ttl_ms: Option<u64>,
    ) -> Result<Claim, StoreError> {
        let op = self.append(self.claim_draft(
            actor.clone(),
            issue,
            ClaimAction::Take {
                agent: actor.name().to_owned(),
                ttl_ms,
                prev: None,
            },
        )?)?;
        // From the operation, not the table: another writer may already have
        // released and retaken the Issue.
        taken(&op, actor.name(), ttl_ms)
    }

    /// Releases the Claim `claim` holds on `issue`.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotClaimHolder`] when `claim` is not the Issue's current
    /// Claim, [`StoreError::UnknownIssue`], or as [`Store::append_all`].
    pub fn release(&mut self, actor: &Actor, issue: Uuid, claim: Uuid) -> Result<Op, StoreError> {
        let draft = self.claim_draft(actor.clone(), issue, ClaimAction::Release { claim })?;
        self.append(draft)
    }

    /// The Claim holding `issue`; one past `expires_ms` still holds until
    /// expiry is enforced.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownIssue`], any SQLite failure, or
    /// [`StoreError::Corrupt`] for a malformed row.
    pub fn claim(&self, issue: Uuid) -> Result<Option<Claim>, StoreError> {
        issue_project(&self.conn, issue)?.ok_or(StoreError::UnknownIssue(issue))?;
        current(&self.conn, issue)
    }

    /// The Claims `agent` holds for `on_behalf_of`, by Issue id; as with
    /// [`Store::claim`], one past `expires_ms` still holds.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn claims_held(&self, agent: &str, on_behalf_of: &str) -> Result<Vec<Claim>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT issue_id, op_id, expires_ms FROM claims
             WHERE agent = ?1 AND on_behalf_of = ?2 ORDER BY issue_id",
        )?;
        let rows = stmt.query_map(params![agent, on_behalf_of], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (issue, id, expires_ms) = row?;
            Ok(Claim {
                id: uuid_from(&id)?,
                issue: uuid_from(&issue)?,
                agent: agent.to_owned(),
                on_behalf_of: on_behalf_of.to_owned(),
                expires_ms,
            })
        })
        .collect()
    }

    fn claim_draft(
        &self,
        actor: Actor,
        issue: Uuid,
        action: ClaimAction,
    ) -> Result<Draft, StoreError> {
        let project = issue_project(&self.conn, issue)?.ok_or(StoreError::UnknownIssue(issue))?;
        Ok(Draft {
            actor,
            project,
            subject: issue,
            change: Change::Claim { action },
        })
    }
}

pub(crate) fn current(conn: &Connection, issue: Uuid) -> Result<Option<Claim>, StoreError> {
    let row = conn
        .query_row(
            "SELECT op_id, agent, on_behalf_of, expires_ms FROM claims WHERE issue_id = ?1",
            [issue.as_bytes()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?;
    row.map(|(id, agent, on_behalf_of, expires_ms)| {
        Ok(Claim {
            id: uuid_from(&id)?,
            issue,
            agent,
            on_behalf_of,
            expires_ms,
        })
    })
    .transpose()
}

fn invalid(reason: &str) -> StoreError {
    StoreError::Invalid {
        field: CLAIM_FIELD.into(),
        reason: reason.to_owned(),
        valid: Vec::new(),
    }
}

/// Whether `action` may apply while `held` holds the Issue: a take only to an
/// Issue it saw unclaimed and that still is, a release only to the Claim it
/// names. Taking over a held Claim needs expiry or force, neither of which tm
/// applies.
fn stands(held: Option<&Claim>, action: &ClaimAction) -> bool {
    match action {
        ClaimAction::Take { prev, .. } => held.is_none() && prev.is_none(),
        ClaimAction::Release { claim } => held.is_some_and(|held| held.id == *claim),
    }
}

/// The Claim a take makes, held to the rules that need no state, so replay
/// holds a pulled take to them too.
fn taken(op: &Op, agent: &str, ttl_ms: Option<u64>) -> Result<Claim, StoreError> {
    if agent.trim().is_empty() {
        return Err(invalid("needs an Agent"));
    }
    if agent.chars().any(char::is_control) {
        return Err(invalid("cannot name an Agent with control characters"));
    }
    let expires_ms = match ttl_ms {
        None => None,
        Some(0) => return Err(invalid("cannot expire as it is taken")),
        Some(ttl) => Some(
            i64::try_from(ttl)
                .ok()
                .and_then(|ttl| op.utc_ms.checked_add(ttl))
                .ok_or_else(|| invalid(&format!("expiry {ttl} ms is out of range")))?,
        ),
    };
    Ok(Claim {
        id: op.op_id,
        issue: op.subject,
        agent: agent.to_owned(),
        on_behalf_of: op.on_behalf_of.clone().unwrap_or_else(|| op.actor.clone()),
        expires_ms,
    })
}

/// Checked on write only, so takes already in the log still replay.
pub(crate) fn check_taker(actor: &Actor, action: &ClaimAction) -> Result<(), StoreError> {
    let ClaimAction::Take { agent, .. } = action else {
        return Ok(());
    };
    if !actor.is_agent() {
        return Err(StoreError::NoAgent {
            actor: actor.name().to_owned(),
        });
    }
    if agent != actor.name() {
        return Err(invalid("must name the Agent writing it"));
    }
    Ok(())
}

/// Checked inside the write transaction, so of concurrent takers exactly one
/// finds the Issue unclaimed. The rule is strict: a Claim under the taker's
/// own Agent name still refuses it. An archived Issue is refused only here,
/// as Workflow rules are: a merge may archive an Issue someone took.
pub(crate) fn check(
    tx: &Transaction<'_>,
    issue: Uuid,
    action: &ClaimAction,
) -> Result<(), StoreError> {
    let held = current(tx, issue)?;
    if !stands(held.as_ref(), action) {
        return Err(match (action, held) {
            (ClaimAction::Take { .. }, Some(held)) => StoreError::AlreadyClaimed {
                issue,
                agent: held.agent,
                on_behalf_of: held.on_behalf_of,
                claim: held.id,
                expires_ms: held.expires_ms,
            },
            (ClaimAction::Take { .. }, None) => {
                invalid("names a previous Claim, but the Issue is unclaimed")
            }
            (ClaimAction::Release { claim }, held) => StoreError::NotClaimHolder {
                issue,
                claim: *claim,
                holder: held.map(Box::new),
            },
        });
    }
    if matches!(action, ClaimAction::Take { .. }) {
        let archived: bool = tx.query_row(
            "SELECT archived FROM issues WHERE id = ?1",
            [issue.as_bytes()],
            |row| row.get(0),
        )?;
        if archived {
            return Err(invalid("cannot be taken on an archived Issue"));
        }
    }
    Ok(())
}

/// `Ok(Some(reason))` when the action lost: the Issue's Claim is not the one
/// it saw.
pub(crate) fn apply(
    tx: &Transaction<'_>,
    op: &Op,
    action: &ClaimAction,
) -> Result<Option<String>, StoreError> {
    let held = current(tx, op.subject)?;
    // First, so a take that does not fit is set aside as such, not as lost.
    let claim = match action {
        ClaimAction::Take { agent, ttl_ms, .. } => Some(taken(op, agent, *ttl_ms)?),
        ClaimAction::Release { .. } => None,
    };
    if !stands(held.as_ref(), action) {
        let holder = held.map_or_else(
            || "the Issue is unclaimed".to_owned(),
            |held| format!("{} holds it for {}", held.agent, held.on_behalf_of),
        );
        return Ok(Some(match action {
            ClaimAction::Take { prev, .. } => format!(
                "{LOST} a take that saw {}, but {holder}",
                if prev.is_some() {
                    "an earlier Claim"
                } else {
                    "it unclaimed"
                }
            ),
            ClaimAction::Release { claim } => {
                format!("{LOST} a release of Claim {claim}, but {holder}")
            }
        }));
    }
    match claim {
        Some(claim) => tx.execute(
            "INSERT INTO claims (issue_id, agent, on_behalf_of, op_id, expires_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                claim.issue.as_bytes(),
                claim.agent,
                claim.on_behalf_of,
                claim.id.as_bytes(),
                claim.expires_ms
            ],
        )?,
        None => tx.execute(
            "DELETE FROM claims WHERE issue_id = ?1",
            [op.subject.as_bytes()],
        )?,
    };
    Ok(None)
}
