//! Claims: a take succeeds only on an unclaimed Issue, and replay applies
//! takes and releases by compare-and-set on the Claim they saw (ADR 0009).

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use tm_core::Op;
use tm_core::op::{CLAIM_FIELD, ClaimAction};
use uuid::Uuid;

use crate::apply::uuid_from;
use crate::{Claim, StoreError};

/// Begins the `unapplied` reason of a take or release that lost to another,
/// telling lost Claims apart from operations that do not fit.
pub(crate) const LOST: &str = "lost:";

pub(crate) fn current(conn: &Connection, issue: Uuid) -> Result<Option<Claim>, StoreError> {
    let row = conn
        .query_row(
            "SELECT op_id, agent, actor, expires_ms FROM claims WHERE issue_id = ?1",
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
    row.map(|(id, agent, actor, expires_ms)| {
        Ok(Claim {
            id: uuid_from(&id)?,
            issue,
            agent,
            actor,
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

/// The rules a take obeys whatever the Issue's state, so replay holds a pulled
/// take to them too; returns when the Claim expires.
pub(crate) fn check_take(
    agent: &str,
    ttl_ms: Option<u64>,
    utc_ms: i64,
) -> Result<Option<i64>, StoreError> {
    if agent.trim().is_empty() {
        return Err(invalid("needs an Agent"));
    }
    if agent.chars().any(char::is_control) {
        return Err(invalid("cannot name an Agent with control characters"));
    }
    match ttl_ms {
        None => Ok(None),
        Some(0) => Err(invalid("cannot expire as it is taken")),
        Some(ttl) => i64::try_from(ttl)
            .ok()
            .and_then(|ttl| utc_ms.checked_add(ttl))
            .map(Some)
            .ok_or_else(|| invalid(&format!("expiry {ttl} ms is out of range"))),
    }
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
    match action {
        ClaimAction::Take {
            agent,
            ttl_ms,
            prev,
        } => {
            if let Some(held) = held {
                return Err(StoreError::AlreadyClaimed {
                    issue,
                    agent: held.agent,
                    actor: held.actor,
                    claim: held.id,
                    expires_ms: held.expires_ms,
                });
            }
            if prev.is_some() {
                return Err(invalid(
                    "names a previous Claim, but the Issue is unclaimed",
                ));
            }
            // The expiry's end is checked against the write's own time in apply.
            check_take(agent, *ttl_ms, 0)?;
            let archived: bool = tx.query_row(
                "SELECT archived FROM issues WHERE id = ?1",
                [issue.as_bytes()],
                |row| row.get(0),
            )?;
            if archived {
                return Err(invalid("cannot be taken on an archived Issue"));
            }
        }
        ClaimAction::Release { claim } => {
            if held.as_ref().map(|held| held.id) != Some(*claim) {
                return Err(StoreError::NotClaimHolder {
                    issue,
                    claim: *claim,
                    current: held.map(|held| held.id),
                });
            }
        }
    }
    Ok(())
}

/// `Ok(Some(reason))` when the action lost: the Issue's Claim is not the one
/// it saw. A take applies only to an unclaimed Issue it saw unclaimed; taking
/// over a held Claim needs expiry or force, neither of which tm applies.
pub(crate) fn apply(
    tx: &Transaction<'_>,
    op: &Op,
    action: ClaimAction,
) -> Result<Option<String>, StoreError> {
    let held = current(tx, op.subject)?;
    let held_id = held.as_ref().map(|held| held.id);
    let describe = |held: Option<Claim>| {
        held.map_or_else(
            || "the Issue is unclaimed".to_owned(),
            |held| format!("{} holds it (Claim {})", held.agent, held.id),
        )
    };
    match action {
        ClaimAction::Take {
            agent,
            ttl_ms,
            prev,
        } => {
            // First, so a take that does not fit is set aside as such, not as lost.
            let expires_ms = check_take(&agent, ttl_ms, op.utc_ms)?;
            if held_id.is_some() || prev.is_some() {
                return Ok(Some(format!(
                    "{LOST} a take that saw {}, but {}",
                    prev.map_or_else(|| "it unclaimed".to_owned(), |prev| format!("Claim {prev}")),
                    describe(held)
                )));
            }
            tx.execute(
                "INSERT INTO claims (issue_id, agent, actor, op_id, expires_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    op.subject.as_bytes(),
                    agent,
                    op.actor,
                    op.op_id.as_bytes(),
                    expires_ms
                ],
            )?;
        }
        ClaimAction::Release { claim } => {
            if held_id != Some(claim) {
                return Ok(Some(format!(
                    "{LOST} a release of Claim {claim}, but {}",
                    describe(held)
                )));
            }
            tx.execute(
                "DELETE FROM claims WHERE issue_id = ?1",
                [op.subject.as_bytes()],
            )?;
        }
    }
    Ok(None)
}
