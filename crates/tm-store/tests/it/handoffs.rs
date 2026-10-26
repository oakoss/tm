//! Handing a Claim to another Agent (ADR 0009): the Claim is replaced by one
//! held for that Agent in no session, which its first session to take the
//! Issue binds.

use std::path::PathBuf;

use tm_core::Actor;
use tm_core::actor::SessionId;
use tm_store::{Claim, Store, StoreError};
use uuid::Uuid;

use crate::support::{ManualClock, TempDir, held, jace, new_issue, open, run_children};

const HOUR_MS: u64 = 3_600_000;

fn agent(name: &str, session: &str) -> Actor {
    Actor::agent(name, "jace").in_session(SessionId::parse(session).unwrap())
}

/// A store whose Issue claude's session s1 has handed to codex.
fn handed(dir: &TempDir, clock: &ManualClock) -> (Store, Uuid) {
    let mut store = open(&dir.db(), clock);
    let issue = new_issue(&mut store);
    let taken = store
        .take(&agent("claude", "s1"), issue, Some(HOUR_MS))
        .unwrap();
    let note = "left off at the parser";
    let (_, handed) = store
        .hand(&agent("claude", "s1"), &taken, "codex", note)
        .unwrap();
    assert_eq!(held(&store, issue), Some(handed), "as the table holds it");
    (store, issue)
}

#[test]
fn a_handoff_holds_the_issue_for_the_agent_with_its_note() {
    let dir = TempDir::new();
    let (store, issue) = handed(&dir, &ManualClock::at(1_000));
    let claim = held(&store, issue).unwrap();
    assert!(claim.handed);
    assert_eq!(
        (claim.agent.as_str(), claim.on_behalf_of.as_str()),
        ("codex", "jace")
    );
    assert_eq!(claim.session, None);
    assert_eq!(claim.expires_ms, Some(1_000 + 3_600_000));
    // Batches are this Replica's alone; the note travels in the operation.
    let batches = "DELETE FROM op_batches";
    store.connection().execute(batches, []).unwrap();
    assert_eq!(
        store.handed_by(claim.id).unwrap(),
        Some(("claude".into(), "left off at the parser".into()))
    );
    let comments = store.comments(issue).unwrap();
    assert_eq!(comments.len(), 1);
}

#[test]
fn the_receiving_agents_first_session_takes_it_and_no_one_else() {
    let dir = TempDir::new();
    let (mut store, issue) = handed(&dir, &ManualClock::at(1_000));
    for other in [agent("claude", "s1"), agent("claude", "s2")] {
        let err = store.take(&other, issue, Some(HOUR_MS)).unwrap_err();
        assert!(matches!(err, StoreError::AlreadyClaimed { .. }), "{err}");
    }
    let taken = store
        .take(&agent("codex", "x1"), issue, Some(HOUR_MS))
        .unwrap();
    assert_eq!(taken.session.as_deref(), Some("x1"));
    let claim = held(&store, issue).unwrap();
    assert!(!claim.handed, "bound to the session that took it");
    let err = store
        .take(&agent("codex", "x2"), issue, Some(HOUR_MS))
        .unwrap_err();
    assert!(matches!(err, StoreError::AlreadyClaimed { .. }), "{err}");
}

#[test]
fn a_handoff_judged_on_a_claim_since_renewed_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let giver = agent("claude", "s1");
    let taken = store.take(&giver, issue, Some(HOUR_MS)).unwrap();
    // As read before a renewal moved its expiry.
    let judged = Claim {
        expires_ms: taken.expires_ms.map(|at| at - 1),
        ..taken
    };
    let err = store.hand(&giver, &judged, "codex", "x").unwrap_err();
    assert!(matches!(err, StoreError::ClaimChanged(_)), "{err}");
    assert!(!held(&store, issue).unwrap().handed);
}

#[test]
fn the_agent_working_for_another_human_cannot_take_it() {
    let dir = TempDir::new();
    let (mut store, issue) = handed(&dir, &ManualClock::at(1_000));
    let for_kim = Actor::agent("codex", "kim").in_session(SessionId::parse("k1").unwrap());
    let err = store.take(&for_kim, issue, Some(HOUR_MS)).unwrap_err();
    assert!(matches!(err, StoreError::AlreadyClaimed { .. }), "{err}");
}

#[test]
fn a_receiver_in_no_session_takes_it_rather_than_keeping_it() {
    let dir = TempDir::new();
    let (mut store, issue) = handed(&dir, &ManualClock::at(1_000));
    let unnamed = Actor::agent("codex", "jace");
    assert!(!held(&store, issue).unwrap().taken_by(&unnamed));
    let started = store
        .take_with(&unnamed, issue, Some(HOUR_MS), |_| (Vec::new(), ()))
        .unwrap();
    assert!(!started.kept, "a take is written");
    assert!(!held(&store, issue).unwrap().handed);
    let err = store
        .take(&agent("codex", "x2"), issue, Some(HOUR_MS))
        .unwrap_err();
    assert!(matches!(err, StoreError::AlreadyClaimed { .. }), "{err}");
}

#[test]
fn a_claim_in_no_session_is_still_taken_by_no_one_else() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let unnamed = Actor::agent("codex", "jace");
    store.take(&unnamed, issue, Some(HOUR_MS)).unwrap();
    let err = store
        .take(&agent("codex", "x1"), issue, Some(HOUR_MS))
        .unwrap_err();
    assert!(
        matches!(err, StoreError::AlreadyClaimed { .. }),
        "only a handed Claim is taken while it holds: {err}"
    );
}

#[test]
fn an_untaken_handoff_lapses_and_any_agent_may_take_it() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let (mut store, issue) = handed(&dir, &clock);
    clock.advance(i64::try_from(HOUR_MS).unwrap());
    assert_eq!(held(&store, issue), None);
    store
        .take(&agent("claude", "s2"), issue, Some(HOUR_MS))
        .unwrap();
}

#[test]
fn undoing_a_handoff_releases_it_and_a_rebuild_keeps_it() {
    let dir = TempDir::new();
    let (mut store, issue) = handed(&dir, &ManualClock::at(1_000));
    let before = held(&store, issue).unwrap();
    store.rebuild().unwrap();
    assert_eq!(held(&store, issue), Some(before.clone()));

    store.undo(&agent("claude", "s1"), before.id).unwrap();
    assert_eq!(held(&store, issue), None);
    assert!(store.comments(issue).unwrap()[0].retracted);
}

const CHILD_DB: &str = "TM_STORE_TEST_HANDOFF_DB";
const CHILD_ISSUE: &str = "TM_STORE_TEST_HANDOFF_ISSUE";
const CHILD_SESSION: &str = "TM_STORE_TEST_HANDOFF_SESSION";

#[test]
fn of_many_sessions_taking_one_handoff_one_wins() {
    if let (Ok(db), Ok(issue), Ok(session)) = (
        std::env::var(CHILD_DB),
        std::env::var(CHILD_ISSUE),
        std::env::var(CHILD_SESSION),
    ) {
        let mut store = Store::open(&PathBuf::from(db)).unwrap();
        let issue = Uuid::parse_str(&issue).unwrap();
        match store.take(&agent("codex", &session), issue, Some(HOUR_MS)) {
            Ok(_) | Err(StoreError::AlreadyClaimed { .. }) => {}
            Err(err) => panic!("{err}"),
        }
        return;
    }

    let dir = TempDir::new();
    let mut store = Store::open(&dir.db()).unwrap();
    let (project, _) = store.create_project(&jace(), "CPL", "Core").unwrap();
    let (issue, _) = store.create_issue(&jace(), project, "task", "t").unwrap();
    let giver = agent("claude", "s1");
    let taken = store.take(&giver, issue, Some(HOUR_MS)).unwrap();
    store.hand(&giver, &taken, "codex", "yours").unwrap();
    drop(store);

    run_children(
        "handoffs::of_many_sessions_taking_one_handoff_one_wins",
        50,
        |i, command| {
            command
                .env(CHILD_DB, dir.db())
                .env(CHILD_ISSUE, issue.to_string())
                .env(CHILD_SESSION, format!("x{i}"));
        },
    );

    let store = Store::open(&dir.db()).unwrap();
    let codex_writes: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM ops o WHERE o.kind = 'claim' AND o.actor = 'codex'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(codex_writes, 1, "one take written, the rest refused");
    let claim = held(&store, issue).unwrap();
    assert!(!claim.handed);
    assert!(claim.session.is_some());
}
