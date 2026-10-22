//! Claims: a take succeeds only on an unclaimed Issue, and replay applies
//! takes and releases by compare-and-set on the Claim they saw (ADR 0009).

use std::borrow::Cow;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use tm_core::actor::{SessionId, is_session_id};
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
    /// The harness session that took it; none when the harness named none.
    pub session: Option<String>,
}

impl Claim {
    /// Whether the Claim has lapsed by `at_ms`; one with no expiry never does.
    #[must_use]
    pub fn lapsed(&self, at_ms: i64) -> bool {
        self.expires_ms
            .is_some_and(|expires_ms| expires_ms <= at_ms)
    }

    /// The Agent that took it, with its session when it names one, as
    /// messages show it.
    #[must_use]
    pub fn taker(&self) -> String {
        match self.session_label() {
            Some(label) => format!("{} ({label})", self.agent),
            None => self.agent.clone(),
        }
    }

    /// Its session as text shows it, such as `session …6417eead`.
    #[must_use]
    pub fn session_label(&self) -> Option<String> {
        let session = self.session.as_deref()?;
        Some(format!("session {}", short_session(session)))
    }

    /// Whether it was taken in `session`; one that names no session reads as
    /// every session's own.
    #[must_use]
    pub fn taken_in(&self, session: Option<&str>) -> bool {
        self.session.is_none() || self.session.as_deref() == session
    }
}

/// `session` as text shows it. Any UUID becomes `…` and its last 8 hex
/// digits: a time-ordered id begins with its start time, which sessions begun
/// the same minute share, and the `…` keeps it from reading as a name. Any
/// other session shows whole.
fn short_session(session: &str) -> Cow<'_, str> {
    match Uuid::try_parse(session) {
        Ok(id) => {
            let hex = id.simple().to_string();
            Cow::Owned(format!("…{}", &hex[hex.len() - 8..]))
        }
        Err(_) => Cow::Borrowed(session),
    }
}

/// An Issue's Claim: holding it, or lapsed and not yet replaced by a take or
/// removed by a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimState {
    Held(Claim),
    Lapsed(Claim),
}

impl ClaimState {
    fn at(claim: Claim, at_ms: i64) -> Self {
        if claim.lapsed(at_ms) {
            Self::Lapsed(claim)
        } else {
            Self::Held(claim)
        }
    }

    #[must_use]
    pub fn claim(&self) -> &Claim {
        match self {
            Self::Held(claim) | Self::Lapsed(claim) => claim,
        }
    }

    #[must_use]
    pub fn held(&self) -> Option<&Claim> {
        match self {
            Self::Held(claim) => Some(claim),
            Self::Lapsed(_) => None,
        }
    }

    #[must_use]
    pub fn lapsed(&self) -> Option<&Claim> {
        match self {
            Self::Lapsed(claim) => Some(claim),
            Self::Held(_) => None,
        }
    }
}

/// An Agent's Claims for one human, split by whether each has lapsed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentClaims {
    pub held: Vec<Claim>,
    pub lapsed: Vec<Claim>,
}

/// What holds an Issue when a release names a Claim that does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Holder {
    Unclaimed,
    Current(Box<ClaimState>),
    /// Took the Issue over from the released Claim once that lapsed.
    TookOver(Box<Claim>),
}

impl std::fmt::Display for Holder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unclaimed => write!(f, "it is unclaimed"),
            Self::Current(state) => match &**state {
                ClaimState::Held(held) => {
                    write!(f, "{} holds it for {}", held.taker(), held.on_behalf_of)
                }
                ClaimState::Lapsed(held) => write!(
                    f,
                    "the Claim by {} for {} on it has lapsed",
                    held.taker(),
                    held.on_behalf_of
                ),
            },
            Self::TookOver(held) => write!(
                f,
                "it lapsed and {} took the Issue over for {}",
                held.taker(),
                held.on_behalf_of
            ),
        }
    }
}

impl Store {
    /// Claims `issue` for the Agent writing, taking over a lapsed Claim.
    /// Refused while any other Claim holds it, even one under the same Agent
    /// name; [`Claim::id`] is the token to release it.
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
        let take = |prev| ClaimAction::Take {
            agent: actor.name().to_owned(),
            ttl_ms,
            prev,
            session: actor.session().map(str::to_owned),
        };
        let draft = self.claim_draft(actor.clone(), issue, take(None))?;
        // `prev` is read inside the write, so no other take slips in between.
        let ops = self.append_with(
            Vec::new(),
            |tx| {
                let prev = current(tx, issue)?.map(|held| held.id);
                Ok(vec![Draft {
                    change: Change::Claim { action: take(prev) },
                    ..draft
                }])
            },
            true,
            None,
        )?;
        // From the operation, not the table: another writer may already have
        // released and retaken the Issue.
        taken(&ops[0], actor.name(), ttl_ms, actor.session())
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

    /// The Claim on `issue`, held or lapsed, read against one clock reading.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownIssue`], [`StoreError::Clock`], any SQLite
    /// failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn claim_state(&self, issue: Uuid) -> Result<Option<ClaimState>, StoreError> {
        issue_project(&self.conn, issue)?.ok_or(StoreError::UnknownIssue(issue))?;
        let now_ms = self.utc_ms()?;
        Ok(current(&self.conn, issue)?.map(|claim| ClaimState::at(claim, now_ms)))
    }

    /// The Claims `agent` took for `on_behalf_of`, held and lapsed, each by
    /// Issue id.
    ///
    /// # Errors
    ///
    /// [`StoreError::Clock`], any SQLite failure, or [`StoreError::Corrupt`]
    /// for a malformed row.
    pub fn claims_by(&self, agent: &str, on_behalf_of: &str) -> Result<AgentClaims, StoreError> {
        let now_ms = self.utc_ms()?;
        let (lapsed, held) = self
            .claims_of(agent, on_behalf_of)?
            .into_iter()
            .partition(|claim| claim.lapsed(now_ms));
        Ok(AgentClaims { held, lapsed })
    }

    fn claims_of(&self, agent: &str, on_behalf_of: &str) -> Result<Vec<Claim>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT issue_id, op_id, expires_ms, session FROM claims
             WHERE agent = ?1 AND on_behalf_of = ?2 ORDER BY issue_id",
        )?;
        let rows = stmt.query_map(params![agent, on_behalf_of], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (issue, id, expires_ms, session) = row?;
            Ok(Claim {
                id: uuid_from(&id)?,
                issue: uuid_from(&issue)?,
                agent: agent.to_owned(),
                on_behalf_of: on_behalf_of.to_owned(),
                expires_ms,
                session,
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
            "SELECT op_id, agent, on_behalf_of, expires_ms, session FROM claims
             WHERE issue_id = ?1",
            [issue.as_bytes()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()?;
    row.map(|(id, agent, on_behalf_of, expires_ms, session)| {
        Ok(Claim {
            id: uuid_from(&id)?,
            issue,
            agent,
            on_behalf_of,
            expires_ms,
            session,
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

/// Whether `action`, written at `at_ms`, may apply while `held` holds the
/// Issue: a take only over the Claim it saw, unclaimed or lapsed by then, and
/// a release only of the Claim it names, lapsed or not. Judged by the time the
/// operation records, never the clock, so every Replica replays the same.
fn stands(held: Option<&Claim>, action: &ClaimAction, at_ms: i64) -> bool {
    match action {
        ClaimAction::Take { prev, .. } => match held {
            None => prev.is_none(),
            Some(held) => *prev == Some(held.id) && held.lapsed(at_ms),
        },
        ClaimAction::Release { claim } => held.is_some_and(|held| held.id == *claim),
    }
}

/// The Claim a take makes, held to the rules that need no state, so replay
/// holds a pulled take to them too.
fn taken(
    op: &Op,
    agent: &str,
    ttl_ms: Option<u64>,
    session: Option<&str>,
) -> Result<Claim, StoreError> {
    if agent.trim().is_empty() {
        return Err(invalid("needs an Agent"));
    }
    if agent.chars().any(char::is_control) {
        return Err(invalid("cannot name an Agent with control characters"));
    }
    if session.is_some_and(|session| !is_session_id(session)) {
        return Err(invalid(&format!(
            "cannot name a session other than by {}",
            SessionId::RULE
        )));
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
        session: session.map(str::to_owned),
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
    if let ClaimAction::Take { session, .. } = action
        && session.as_deref() != actor.session()
    {
        return Err(invalid("must name the session writing it"));
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
    at_ms: i64,
) -> Result<(), StoreError> {
    let held = current(tx, issue)?;
    if !stands(held.as_ref(), action, at_ms) {
        return Err(match (action, held) {
            (ClaimAction::Take { .. }, Some(held)) if held.lapsed(at_ms) => {
                invalid("must name the lapsed Claim it takes over")
            }
            (ClaimAction::Take { .. }, Some(held)) => StoreError::AlreadyClaimed {
                issue,
                holder: Box::new(held),
            },
            (ClaimAction::Take { .. }, None) => {
                invalid("names a previous Claim, but the Issue is unclaimed")
            }
            (ClaimAction::Release { claim }, held) => StoreError::NotClaimHolder {
                issue,
                claim: *claim,
                holder: match held {
                    None => Holder::Unclaimed,
                    Some(held) if took_over(tx, &held, *claim)? => Holder::TookOver(Box::new(held)),
                    Some(held) => Holder::Current(Box::new(ClaimState::at(held, at_ms))),
                },
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

/// Whether the take behind `held` named the lapsed `claim` as its `prev`.
fn took_over(tx: &Transaction<'_>, held: &Claim, claim: Uuid) -> Result<bool, StoreError> {
    let payload: String = tx
        .query_row(
            "SELECT payload FROM ops WHERE op_id = ?1",
            [held.id.as_bytes()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| {
            StoreError::Corrupt(format!(
                "the take {} holding Issue {} is not in the log",
                held.id, held.issue
            ))
        })?;
    let change: Change = serde_json::from_str(&payload)
        .map_err(|err| StoreError::Corrupt(format!("the take {}'s payload: {err}", held.id)))?;
    Ok(matches!(
        change,
        Change::Claim {
            action: ClaimAction::Take { prev: Some(prev), .. },
        } if prev == claim
    ))
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
        ClaimAction::Take {
            agent,
            ttl_ms,
            session,
            ..
        } => Some(taken(op, agent, *ttl_ms, session.as_deref())?),
        ClaimAction::Release { .. } => None,
    };
    if !stands(held.as_ref(), action, op.utc_ms) {
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
        // Replaces a lapsed Claim the take took over.
        Some(claim) => tx.execute(
            "INSERT OR REPLACE INTO claims
               (issue_id, agent, on_behalf_of, op_id, expires_ms, session)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                claim.issue.as_bytes(),
                claim.agent,
                claim.on_behalf_of,
                claim.id.as_bytes(),
                claim.expires_ms,
                claim.session
            ],
        )?,
        None => tx.execute(
            "DELETE FROM claims WHERE issue_id = ?1",
            [op.subject.as_bytes()],
        )?,
    };
    Ok(None)
}
