//! The Agent sessions that wrote an Issue, read from the log: each
//! operation records the session that wrote it.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::OptionalExtension;
use serde::Serialize;
use tm_core::op::ClaimAction;
use tm_core::{Change, Op};
use uuid::Uuid;

use crate::{Store, StoreError, Workplace};

/// One session's part in an Issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionWork {
    pub session: String,
    pub agent: String,
    pub on_behalf_of: Option<String>,
    pub first_ms: i64,
    /// Renewals included, so a session that only kept its Claim still reads
    /// as active.
    pub last_ms: i64,
    /// The branch its latest take recorded.
    pub branch: Option<String>,
    /// Where it last wrote from on this machine; none when it ran elsewhere.
    pub place: Option<SessionPlace>,
    pub claimed: bool,
    pub handed: bool,
    pub comments: usize,
    pub asked: usize,
    pub answered: usize,
    pub withdrew: usize,
    /// The Statuses it moved the Issue to, in order.
    pub moved_to: Vec<String>,
    /// Its other writes, renewals and releases aside.
    pub other: usize,
    /// Whether it wrote the Issue's latest Status.
    pub moved_last: bool,
}

impl Store {
    /// The sessions that wrote `issue`, the most recently active first.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn sessions(&self, issue: Uuid) -> Result<Vec<SessionWork>, StoreError> {
        self.read(|store| {
            let mut stmt = store.conn.prepare(
                "SELECT o.session, o.actor, o.on_behalf_of, o.utc_ms, o.payload, o.op_id
                 FROM ops o
                 WHERE o.issue_id = ?1 AND o.session IS NOT NULL
                 ORDER BY o.utc_ms, o.hlc, o.op_id",
            )?;
            let mut rows = stmt.query([issue.as_bytes()])?;
            let mut work: BTreeMap<String, SessionWork> = BTreeMap::new();
            while let Some(row) = rows.next()? {
                let session: String = row.get(0)?;
                let at: i64 = row.get(3)?;
                let payload: String = row.get(4)?;
                let change: Change = serde_json::from_str(&payload)
                    .map_err(|err| StoreError::Corrupt(format!("a logged payload: {err}")))?;
                let entry = work.entry(session.clone()).or_insert_with(|| SessionWork {
                    session,
                    agent: String::new(),
                    on_behalf_of: None,
                    first_ms: at,
                    last_ms: at,
                    branch: None,
                    place: None,
                    claimed: false,
                    handed: false,
                    comments: 0,
                    asked: 0,
                    answered: 0,
                    withdrew: 0,
                    moved_to: Vec::new(),
                    other: 0,
                    moved_last: false,
                });
                entry.agent = row.get(1)?;
                entry.on_behalf_of = row.get(2)?;
                entry.last_ms = entry.last_ms.max(at);
                tally(entry, change);
            }
            let moved_last = latest_status_session(&store.conn, issue)?;
            let mut sessions: Vec<SessionWork> = work.into_values().collect();
            for session in &mut sessions {
                session.moved_last = moved_last.as_deref() == Some(session.session.as_str());
                session.place = place_of(&store.conn, &session.session)?;
            }
            sessions.sort_by(|a, b| b.last_ms.cmp(&a.last_ms).then(a.session.cmp(&b.session)));
            Ok(sessions)
        })
    }

    /// Where `session` last wrote from on this machine.
    ///
    /// # Errors
    ///
    /// Any SQLite failure.
    pub fn session_place(&self, session: &str) -> Result<Option<SessionPlace>, StoreError> {
        place_of(&self.conn, session)
    }

    /// The session that wrote the operation `op`, and where it last wrote
    /// from here, when a session wrote it.
    ///
    /// # Errors
    ///
    /// Any SQLite failure.
    pub fn written_by_session(
        &self,
        op: Uuid,
    ) -> Result<Option<(String, Option<SessionPlace>)>, StoreError> {
        let session: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT session FROM ops WHERE op_id = ?1",
                [op.as_bytes()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(session) = session.flatten() else {
            return Ok(None);
        };
        let place = place_of(&self.conn, &session)?;
        Ok(Some((session, place)))
    }
}

/// Where a session last wrote from on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionPlace {
    pub dir: String,
    pub harness: Option<String>,
}

/// Records, for each session `ops` were written in, the directory and harness
/// `place` names, keeping a harness known before when this write names none.
pub(crate) fn record_places(
    tx: &rusqlite::Transaction<'_>,
    place: &Workplace,
    ops: &[Op],
    now_ms: i64,
) -> Result<(), StoreError> {
    let Some(dir) = &place.dir else {
        return Ok(());
    };
    let sessions: BTreeSet<&str> = ops.iter().filter_map(|op| op.session.as_deref()).collect();
    for session in sessions {
        let harness = place
            .harnesses
            .iter()
            .find(|(_, named)| named == session)
            .map(|(harness, _)| harness.as_str());
        tx.execute(
            "INSERT INTO session_dirs (session, dir, written_ms, harness)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (session) DO UPDATE
             SET dir = ?2, written_ms = ?3, harness = coalesce(?4, harness)",
            rusqlite::params![session, dir, now_ms, harness],
        )?;
    }
    Ok(())
}

fn tally(entry: &mut SessionWork, change: Change) {
    match change {
        Change::Claim { action } => match action {
            ClaimAction::Take { branch, .. } => {
                entry.claimed = true;
                if branch.is_some() {
                    entry.branch = branch;
                }
            }
            // A question keeps the Claim for the asker's own Agent: no handoff.
            ClaimAction::Hand { agent, .. } if agent == entry.agent => {}
            ClaimAction::Hand { .. } => entry.handed = true,
            ClaimAction::Renew { .. } | ClaimAction::Release { .. } => {}
        },
        Change::Comment { .. } => entry.comments += 1,
        Change::Ask { .. } => entry.asked += 1,
        Change::Answer { .. } => entry.answered += 1,
        Change::Withdraw { .. } => entry.withdrew += 1,
        Change::SetField { field, value, .. } if field == "status" => {
            if let Some(status) = value.as_str() {
                entry.moved_to.push(status.to_owned());
            }
        }
        _ => entry.other += 1,
    }
}

/// The session that wrote `issue`'s winning Status, if a session did.
fn latest_status_session(
    conn: &rusqlite::Connection,
    issue: Uuid,
) -> Result<Option<String>, StoreError> {
    let session: Option<Option<String>> = conn
        .query_row(
            "SELECT o.session FROM field_versions v JOIN ops o ON o.op_id = v.op_id
             WHERE v.issue_id = ?1 AND v.field = 'status' AND v.elem = ''",
            [issue.as_bytes()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(session.flatten())
}

fn place_of(
    conn: &rusqlite::Connection,
    session: &str,
) -> Result<Option<SessionPlace>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT dir, harness FROM session_dirs WHERE session = ?1",
            [session],
            |row| {
                Ok(SessionPlace {
                    dir: row.get(0)?,
                    harness: row.get(1)?,
                })
            },
        )
        .optional()?)
}
