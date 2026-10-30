//! Claims: a take succeeds only on an unclaimed Issue, and replay applies
//! takes and releases by compare-and-set on the Claim they saw (ADR 0009).

use std::borrow::Cow;
use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use tm_core::actor::{SessionId, is_session_id};
use tm_core::op::{CLAIM_FIELD, ClaimAction};
use tm_core::{Actor, Change, Op};
use uuid::Uuid;

use crate::apply::{issue_project, uuid_from};
use crate::{Draft, Lineage, Provenance, Store, StoreError};

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
    /// The duration a renewal extends it by; none for a Claim that never
    /// lapses.
    pub ttl_ms: Option<u64>,
    /// The harness session that took it; none when the harness named none.
    pub session: Option<String>,
    /// Handed to `agent` by another session and not yet taken by one of its
    /// own, so its first session may take it over while it holds.
    pub handed: bool,
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
            None if self.handed => format!("{} (handed on, not yet taken)", self.agent),
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

    /// Whether `actor` is the Agent, human and session that took it, exactly.
    /// A handed Claim was taken by no one yet, even an Agent in no session.
    #[must_use]
    pub fn taken_by(&self, actor: &Actor) -> bool {
        !self.handed
            && actor.is_agent()
            && self.agent == actor.name()
            && self.on_behalf_of == actor.on_behalf_of().unwrap_or(actor.name())
            && self.session.as_deref() == actor.session()
    }

    /// Whether a renewal at `at_ms` is due: a third of its duration has
    /// passed since it was taken or last renewed, so renewals stay few.
    #[must_use]
    pub fn due(&self, at_ms: i64) -> bool {
        match (
            self.expires_ms,
            self.ttl_ms.and_then(|ttl| i64::try_from(ttl).ok()),
        ) {
            (Some(expires_ms), Some(ttl)) => at_ms >= expires_ms.saturating_sub(ttl - ttl / 3),
            _ => false,
        }
    }
}

/// `session` as text shows it. Any UUID becomes `…` and its last 8 hex
/// digits: a time-ordered id begins with its start time, which sessions begun
/// the same minute share, and the `…` keeps it from reading as a name. Any
/// other session shows whole.
#[must_use]
pub fn short_session(session: &str) -> Cow<'_, str> {
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

/// What [`Store::take_with`] wrote: the Claim, whether it was already the
/// session's and so kept, the batch, and what `extend` made.
#[derive(Debug)]
pub struct TakenWith<T> {
    pub claim: Claim,
    pub kept: bool,
    pub ops: Vec<Op>,
    pub made: T,
}

/// Claims split by whether each had lapsed at `at_ms`, so time left is
/// read against the same instant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentClaims {
    pub held: Vec<Claim>,
    pub lapsed: Vec<Claim>,
    pub at_ms: i64,
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
            |tx, _| {
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

    /// [`Store::take`], with the drafts `extend` makes from `issue` and its
    /// ancestors, read inside the same write, in the take's batch after it;
    /// if the take is refused, none is written. A Claim `actor`'s session
    /// holds on `issue` at the write is kept, not taken again.
    ///
    /// # Errors
    ///
    /// [`StoreError::Clock`], or as [`Store::take`].
    pub fn take_with<T: Default>(
        &mut self,
        actor: &Actor,
        issue: Uuid,
        ttl_ms: Option<u64>,
        extend: impl FnOnce(&Lineage) -> (Vec<Draft>, T),
    ) -> Result<TakenWith<T>, StoreError> {
        let take = |prev| ClaimAction::Take {
            agent: actor.name().to_owned(),
            ttl_ms,
            prev,
            session: actor.session().map(str::to_owned),
        };
        let draft = self.claim_draft(actor.clone(), issue, take(None))?;
        let scheme = self.scheme.clone();
        let mut made = T::default();
        let mut kept = None;
        let ops = self.append_with(
            Vec::new(),
            |tx, at_ms| {
                let held = current(tx, issue)?;
                let (more, out) = extend(&crate::family::lineage(tx, &scheme, issue, at_ms)?);
                made = out;
                if let Some(own) = held
                    .as_ref()
                    .filter(|held| held.taken_by(actor) && !held.lapsed(at_ms))
                {
                    kept = Some(own.clone());
                    return Ok(more);
                }
                let prev = held.map(|held| held.id);
                let mut drafts = vec![Draft {
                    change: Change::Claim { action: take(prev) },
                    ..draft
                }];
                drafts.extend(more);
                Ok(drafts)
            },
            true,
            None,
        )?;
        if let Some(claim) = kept {
            return Ok(TakenWith {
                claim,
                kept: true,
                ops,
                made,
            });
        }
        let take = ops
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
            .ok_or_else(|| StoreError::Corrupt("a take's batch holds no take".into()))?;
        let claim = taken(take, actor.name(), ttl_ms, actor.session())?;
        Ok(TakenWith {
            claim,
            kept: false,
            ops,
            made,
        })
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

    /// Refuses as the write would when `actor` sets one of `fields` on
    /// `issue` past another session's live Claim. The write checks again;
    /// this only fails early, such as before piped text is read.
    ///
    /// # Errors
    ///
    /// [`StoreError::Protected`], or as [`Store::claim_state`].
    pub fn check_protected(
        &self,
        actor: &Actor,
        issue: Uuid,
        fields: &[&str],
    ) -> Result<(), StoreError> {
        let Some(field) = fields.iter().find(|field| PROTECTED_FIELDS.contains(field)) else {
            return Ok(());
        };
        match self.claim_state(issue)? {
            Some(ClaimState::Held(holder))
                if actor.is_agent() && !self.forcing && !holder.taken_by(actor) =>
            {
                Err(StoreError::Protected {
                    issue: issue.into(),
                    field: (*field).to_owned(),
                    holder: Box::new(holder),
                }
                .named(&self.conn))
            }
            _ => Ok(()),
        }
    }

    /// Hands `issue`'s Claim `claim` to `agent` for `ttl_ms`, with `note` as a
    /// comment in the same batch.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotClaimHolder`] when `claim` no longer holds `issue`,
    /// or as [`Store::append_all`].
    pub fn hand(
        &mut self,
        actor: &Actor,
        held: &Claim,
        agent: &str,
        ttl_ms: Option<u64>,
        note: &str,
    ) -> Result<(Vec<Op>, Claim), StoreError> {
        let action = ClaimAction::Hand {
            claim: held.id,
            agent: agent.to_owned(),
            ttl_ms,
            note: note.to_owned(),
        };
        let hand = self.claim_draft(actor.clone(), held.issue, action)?;
        let comment = Draft {
            change: Change::Comment {
                body: note.to_owned(),
            },
            ..hand.clone()
        };
        // The Claim must be as judged, expiry included: one renewed since was
        // judged lapsed but holds again.
        let ops = self.append_if_claim(vec![hand, comment], held.issue, Some(held))?;
        // From the operation, not the table: the receiver may take it at once.
        let handed = Claim {
            on_behalf_of: held.on_behalf_of.clone(),
            handed: true,
            ..taken(&ops[0], agent, ttl_ms, None)?
        };
        Ok((ops, handed))
    }

    /// Who handed `claim` on and their note, when a handoff made it.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed note.
    pub fn handed_by(&self, claim: Uuid) -> Result<Option<(String, String)>, StoreError> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT actor, payload FROM ops WHERE op_id = ?1",
                [claim.as_bytes()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(actor, payload)| match serde_json::from_str(&payload) {
            Ok(Change::Claim {
                action: ClaimAction::Hand { note, .. },
            }) => Ok(Some((actor, note))),
            Ok(_) => Ok(None),
            Err(err) => Err(StoreError::Corrupt(format!(
                "Claim {claim}'s operation: {err}"
            ))),
        })
        .transpose()
        .map(Option::flatten)
    }

    /// Whether `agent` has written anything in this Workspace, so a handoff
    /// to a name never seen can warn of a typo.
    ///
    /// # Errors
    ///
    /// Any SQLite failure.
    pub fn has_written(&self, agent: &str) -> Result<bool, StoreError> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM ops WHERE actor = ?1)",
            [agent],
            |row| row.get(0),
        )?)
    }

    /// The Claim on `issue`, held or lapsed, read against one clock reading.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownIssue`], [`StoreError::Clock`], any SQLite
    /// failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn claim_state(&self, issue: Uuid) -> Result<Option<ClaimState>, StoreError> {
        self.claim_state_at(issue, self.utc_ms()?)
    }

    /// As [`Store::claim_state`], judged at `at_ms`.
    ///
    /// # Errors
    ///
    /// As [`Store::claim_state`].
    pub fn claim_state_at(
        &self,
        issue: Uuid,
        at_ms: i64,
    ) -> Result<Option<ClaimState>, StoreError> {
        issue_project(&self.conn, issue)?.ok_or(StoreError::UnknownIssue(issue))?;
        Ok(current(&self.conn, issue)?.map(|claim| ClaimState::at(claim, at_ms)))
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
        let (lapsed, held) = claims_of(&self.conn, agent, on_behalf_of)?
            .into_iter()
            .partition(|claim| claim.lapsed(now_ms));
        Ok(AgentClaims {
            held,
            lapsed,
            at_ms: now_ms,
        })
    }

    /// Every Claim in the Workspace, held and lapsed, by holder and then
    /// Issue id.
    ///
    /// # Errors
    ///
    /// [`StoreError::Clock`], any SQLite failure, or [`StoreError::Corrupt`]
    /// for a malformed row.
    pub fn claims(&self) -> Result<AgentClaims, StoreError> {
        let now_ms = self.utc_ms()?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT issue_id, op_id, agent, on_behalf_of, expires_ms, ttl_ms, session, handed
             FROM claims ORDER BY agent, on_behalf_of, session, issue_id",
        )?;
        let (lapsed, held) = stmt
            .query_map([], ClaimRow::read)?
            .map(|row| row?.claim())
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .partition(|claim| claim.lapsed(now_ms));
        Ok(AgentClaims {
            held,
            lapsed,
            at_ms: now_ms,
        })
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
            "SELECT issue_id, op_id, agent, on_behalf_of, expires_ms, ttl_ms, session, handed
             FROM claims WHERE issue_id = ?1",
            [issue.as_bytes()],
            ClaimRow::read,
        )
        .optional()?;
    row.map(ClaimRow::claim).transpose()
}

/// A `claims` row as selected in column order, before its ids are checked.
struct ClaimRow {
    issue: Vec<u8>,
    id: Vec<u8>,
    agent: String,
    on_behalf_of: String,
    expires_ms: Option<i64>,
    ttl_ms: Option<i64>,
    session: Option<String>,
    handed: bool,
}

impl ClaimRow {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            issue: row.get(0)?,
            id: row.get(1)?,
            agent: row.get(2)?,
            on_behalf_of: row.get(3)?,
            expires_ms: row.get(4)?,
            ttl_ms: row.get(5)?,
            session: row.get(6)?,
            handed: row.get(7)?,
        })
    }

    fn claim(self) -> Result<Claim, StoreError> {
        Ok(Claim {
            id: uuid_from(&self.id)?,
            issue: uuid_from(&self.issue)?,
            agent: self.agent,
            on_behalf_of: self.on_behalf_of,
            expires_ms: self.expires_ms,
            ttl_ms: self
                .ttl_ms
                .map(u64::try_from)
                .transpose()
                .map_err(|_| StoreError::Corrupt("a Claim's duration is negative".into()))?,
            session: self.session,
            handed: self.handed,
        })
    }
}

impl Store {
    /// Renews each Claim `writer`'s session took that is due, lapsed ones no
    /// take has replaced among them; writes nothing when none is. `issue`'s
    /// Claim is renewed even before it is due.
    ///
    /// # Errors
    ///
    /// [`StoreError::Clock`], or as [`Store::append_all`].
    pub fn heartbeat(
        &mut self,
        writer: &Actor,
        issue: Option<Uuid>,
    ) -> Result<Vec<Op>, StoreError> {
        let writer = writer.clone();
        self.append_with(
            Vec::new(),
            |tx, at_ms| {
                renewals(tx, &writer, |claim| {
                    Some(claim.issue) == issue || claim.due(at_ms)
                })
            },
            true,
            None,
        )
    }

    /// The Claims that now hold an Issue `batch` wrote, or any Issue when
    /// `batch` is `None`, each taken over from a Claim `writer` had taken
    /// there.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row or
    /// payload.
    pub fn taken_over_from(
        &self,
        writer: &Actor,
        batch: Option<Uuid>,
    ) -> Result<Vec<Claim>, StoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT issue_id FROM claims WHERE ?1 IS NULL OR issue_id IN (
               SELECT o.issue_id FROM ops o JOIN op_batches b ON b.op_id = o.op_id
               WHERE b.batch = ?1)
             ORDER BY issue_id",
        )?;
        let issues = stmt
            .query_map([batch.map(|batch| batch.as_bytes().to_vec())], |row| {
                row.get::<_, Vec<u8>>(0)
            })?
            .map(|issue| uuid_from(&issue?))
            .collect::<Result<Vec<_>, _>>()?;
        let mut over = Vec::new();
        for issue in issues {
            if let Some(held) = current(&self.conn, issue)?
                && !held.taken_by(writer)
                && let Some(prev) = take_of(&self.conn, held.id)?.and_then(|take| take.prev)
                && take_of(&self.conn, prev)?.is_some_and(|take| take.by(writer))
            {
                over.push(held);
            }
        }
        Ok(over)
    }
}

/// A take as the log records it.
struct Take {
    agent: String,
    on_behalf_of: String,
    session: Option<String>,
    prev: Option<Uuid>,
}

impl Take {
    fn by(&self, writer: &Actor) -> bool {
        self.agent == writer.name()
            && self.on_behalf_of == writer.on_behalf_of().unwrap_or(writer.name())
            && self.session.as_deref() == writer.session()
    }
}

/// The take that `claim` is the id of, if the log holds one.
fn take_of(conn: &Connection, claim: Uuid) -> Result<Option<Take>, StoreError> {
    let row = conn
        .query_row(
            "SELECT actor, on_behalf_of, payload FROM ops WHERE op_id = ?1",
            [claim.as_bytes()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((actor, on_behalf_of, payload)) = row else {
        return Ok(None);
    };
    let change: Change = serde_json::from_str(&payload)
        .map_err(|err| StoreError::Corrupt(format!("the take {claim}'s payload: {err}")))?;
    Ok(match change {
        Change::Claim {
            action:
                ClaimAction::Take {
                    agent,
                    session,
                    prev,
                    ..
                },
        } => Some(Take {
            agent,
            on_behalf_of: on_behalf_of.unwrap_or(actor),
            session,
            prev,
        }),
        _ => None,
    })
}

/// The Agent writing every draft of a local batch, whose session's Claims the
/// batch renews; none for a batch that is empty, imported or mixed.
pub(crate) fn renewing(drafts: &[(Draft, Option<Provenance>)]) -> Option<Actor> {
    let (first, _) = drafts.first()?;
    let alone = drafts
        .iter()
        .all(|(draft, provenance)| draft.actor == first.actor && provenance.is_none());
    (alone && first.actor.is_agent()).then(|| first.actor.clone())
}

/// The fields a live Claim keeps another session from writing (ADR 0009).
pub const PROTECTED_FIELDS: [&str; 2] = ["status", "description"];

/// The live Claims a local batch writes past: each on an Issue whose
/// protected field a draft sets while another session holds it. An Agent's
/// write is refused unless `forced`; a human's goes through. A batch that
/// takes or releases the Issue's Claim is judged by that Claim's own rules,
/// as a finishing move is.
pub(crate) fn protected(
    conn: &Connection,
    drafts: &[(Draft, Option<Provenance>)],
    at_ms: i64,
    forced: bool,
) -> Result<Vec<Claim>, StoreError> {
    let taken_or_released: HashSet<Uuid> = drafts
        .iter()
        .filter(|(draft, _)| {
            matches!(
                draft.change,
                Change::Claim {
                    action: ClaimAction::Take { .. } | ClaimAction::Release { .. }
                }
            )
        })
        .map(|(draft, _)| draft.subject)
        .collect();
    let mut passed: Vec<Claim> = Vec::new();
    for (draft, provenance) in drafts {
        let Change::SetField { field, .. } = &draft.change else {
            continue;
        };
        if provenance.is_some()
            || !PROTECTED_FIELDS.contains(&field.as_str())
            || taken_or_released.contains(&draft.subject)
        {
            continue;
        }
        let Some(holder) = current(conn, draft.subject)?
            .filter(|claim| !claim.lapsed(at_ms) && !claim.taken_by(&draft.actor))
        else {
            continue;
        };
        if draft.actor.is_agent() && !forced {
            return Err(StoreError::Protected {
                issue: draft.subject.into(),
                field: field.clone(),
                holder: Box::new(holder),
            });
        }
        if !passed.contains(&holder) {
            passed.push(holder);
        }
    }
    Ok(passed)
}

/// The Issues whose Claim `drafts` take, renew or release, which a renewal
/// beside them must leave alone.
pub(crate) fn claimed_in(drafts: &[(Draft, Option<Provenance>)]) -> HashSet<Uuid> {
    drafts
        .iter()
        .filter(|(draft, _)| matches!(draft.change, Change::Claim { .. }))
        .map(|(draft, _)| draft.subject)
        .collect()
}

/// A renewal for each Claim `writer` took that expires and `renew` picks,
/// lapsed ones no take has replaced among them.
pub(crate) fn renewals(
    conn: &Connection,
    writer: &Actor,
    renew: impl Fn(&Claim) -> bool,
) -> Result<Vec<Draft>, StoreError> {
    let held_for = writer.on_behalf_of().unwrap_or(writer.name());
    let mut drafts = Vec::new();
    for claim in claims_of(conn, writer.name(), held_for)? {
        let (true, Some(ttl_ms)) = (claim.taken_by(writer) && renew(&claim), claim.ttl_ms) else {
            continue;
        };
        let project =
            issue_project(conn, claim.issue)?.ok_or(StoreError::UnknownIssue(claim.issue))?;
        drafts.push(Draft {
            actor: writer.clone(),
            project,
            subject: claim.issue,
            change: Change::Claim {
                action: ClaimAction::Renew {
                    claim: claim.id,
                    ttl_ms,
                },
            },
        });
    }
    Ok(drafts)
}

fn claims_of(conn: &Connection, agent: &str, on_behalf_of: &str) -> Result<Vec<Claim>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT issue_id, op_id, agent, on_behalf_of, expires_ms, ttl_ms, session, handed
         FROM claims WHERE agent = ?1 AND on_behalf_of = ?2 ORDER BY issue_id",
    )?;
    let rows = stmt.query_map(params![agent, on_behalf_of], ClaimRow::read)?;
    rows.map(|row| row?.claim()).collect()
}

fn invalid(reason: &str) -> StoreError {
    StoreError::Invalid {
        field: CLAIM_FIELD.into(),
        reason: reason.to_owned(),
        valid: Vec::new(),
    }
}

/// Whether `action`, written at `at_ms` for `human`, may apply while `held`
/// holds the Issue: a take only over the Claim it saw, unclaimed, lapsed by
/// then or handed to the taking Agent and its human, and a renewal, release or
/// handoff only of the Claim it names, lapsed or not. Judged by the time the
/// operation records, never the clock, so every Replica replays the same.
fn stands(held: Option<&Claim>, action: &ClaimAction, (at_ms, human): (i64, &str)) -> bool {
    match action {
        ClaimAction::Take { prev, agent, .. } => match held {
            None => prev.is_none(),
            Some(held) => {
                let handed_to_taker =
                    held.handed && held.agent == *agent && held.on_behalf_of == human;
                *prev == Some(held.id) && (held.lapsed(at_ms) || handed_to_taker)
            }
        },
        ClaimAction::Renew { claim, .. }
        | ClaimAction::Release { claim }
        | ClaimAction::Hand { claim, .. } => held.is_some_and(|held| held.id == *claim),
    }
}

/// Refuses a renewal of a held Claim that never lapses, which would make it
/// lapse; replay sets a pulled one aside as not fitting.
fn renews_an_expiry(held: Option<&Claim>, action: &ClaimAction) -> Result<(), StoreError> {
    match (action, held) {
        (ClaimAction::Renew { claim, .. }, Some(held))
            if held.id == *claim && held.expires_ms.is_none() =>
        {
            Err(invalid("cannot renew a Claim that does not expire"))
        }
        _ => Ok(()),
    }
}

/// When a Claim written at `op`'s time with `ttl_ms` lapses.
fn expiry(op: &Op, ttl_ms: u64) -> Result<i64, StoreError> {
    if ttl_ms == 0 {
        return Err(invalid("cannot expire as it is taken"));
    }
    i64::try_from(ttl_ms)
        .ok()
        .and_then(|ttl| op.utc_ms.checked_add(ttl))
        .ok_or_else(|| invalid(&format!("expiry {ttl_ms} ms is out of range")))
}

/// The Claim a take makes, held to the rules that need no state, so replay
/// holds a pulled take to them too.
pub(crate) fn taken(
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
    let expires_ms = ttl_ms.map(|ttl| expiry(op, ttl)).transpose()?;
    Ok(Claim {
        id: op.op_id,
        issue: op.subject,
        agent: agent.to_owned(),
        on_behalf_of: op.on_behalf_of.clone().unwrap_or_else(|| op.actor.clone()),
        expires_ms,
        ttl_ms,
        session: session.map(str::to_owned),
        handed: false,
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
    (writer, at_ms): (&Actor, i64),
) -> Result<(), StoreError> {
    let held = current(tx, issue)?;
    if let (ClaimAction::Renew { claim, .. }, Some(held)) = (action, &held)
        && held.id == *claim
        && !held.taken_by(writer)
    {
        return Err(invalid("can be renewed only by the session that took it"));
    }
    let human = writer.on_behalf_of().unwrap_or(writer.name());
    if !stands(held.as_ref(), action, (at_ms, human)) {
        return Err(match (action, held) {
            (ClaimAction::Take { .. }, Some(held)) if held.lapsed(at_ms) => {
                invalid("must name the lapsed Claim it takes over")
            }
            (ClaimAction::Take { .. }, Some(held)) => StoreError::AlreadyClaimed {
                issue: issue.into(),
                holder: Box::new(held),
            },
            (ClaimAction::Take { .. }, None) => {
                invalid("names a previous Claim, but the Issue is unclaimed")
            }
            (
                ClaimAction::Release { claim }
                | ClaimAction::Renew { claim, .. }
                | ClaimAction::Hand { claim, .. },
                held,
            ) => StoreError::NotClaimHolder {
                issue: issue.into(),
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
        ClaimAction::Hand { agent, ttl_ms, .. } => Some(Claim {
            handed: true,
            ..taken(op, agent, *ttl_ms, None)?
        }),
        ClaimAction::Renew { .. } | ClaimAction::Release { .. } => None,
    };
    let renewed = match action {
        ClaimAction::Renew { ttl_ms, .. } => Some((expiry(op, *ttl_ms)?, *ttl_ms)),
        _ => None,
    };
    renews_an_expiry(held.as_ref(), action)?;
    let human = op.on_behalf_of.as_deref().unwrap_or(&op.actor);
    if !stands(held.as_ref(), action, (op.utc_ms, human)) {
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
            ClaimAction::Renew { claim, .. } => {
                format!("{LOST} a renewal of Claim {claim}, but {holder}")
            }
            ClaimAction::Release { claim } => {
                format!("{LOST} a release of Claim {claim}, but {holder}")
            }
            ClaimAction::Hand { claim, .. } => {
                format!("{LOST} a handoff of Claim {claim}, but {holder}")
            }
        }));
    }
    // A handed Claim stays held for the human the Claim it replaces was.
    let claim = claim.map(|claim| match (&held, claim.handed) {
        (Some(held), true) => Claim {
            on_behalf_of: held.on_behalf_of.clone(),
            ..claim
        },
        _ => claim,
    });
    match (claim, renewed) {
        // Replaces a lapsed or handed Claim the take took over, or the one
        // handed on.
        (Some(claim), _) => tx.execute(
            "INSERT OR REPLACE INTO claims
               (issue_id, agent, on_behalf_of, op_id, expires_ms, ttl_ms, session, handed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                claim.issue.as_bytes(),
                claim.agent,
                claim.on_behalf_of,
                claim.id.as_bytes(),
                claim.expires_ms,
                claim.ttl_ms.and_then(|ttl| i64::try_from(ttl).ok()),
                claim.session,
                claim.handed
            ],
        )?,
        (None, Some((expires_ms, ttl_ms))) => tx.execute(
            "UPDATE claims SET expires_ms = ?1, ttl_ms = ?2 WHERE issue_id = ?3",
            params![
                expires_ms,
                i64::try_from(ttl_ms).ok(),
                op.subject.as_bytes()
            ],
        )?,
        (None, None) => tx.execute(
            "DELETE FROM claims WHERE issue_id = ?1",
            [op.subject.as_bytes()],
        )?,
    };
    Ok(None)
}
