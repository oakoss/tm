//! Taking the next Issue (ADR 0016): the first eligible of an ordered set,
//! chosen inside the write.

use std::path::PathBuf;

use serde_json::json;
use tm_core::Actor;
use tm_core::actor::SessionId;
use tm_store::{Draft, IssueView, Next, Store};
use uuid::Uuid;

use crate::support::{
    ManualClock, TempDir, claude, draft, field, held, jace, new_issue, open, run_children,
    set_field,
};

/// What `tm start` would write: the Issue's Status to in progress.
fn begin(lineage: &[IssueView]) -> (Vec<Draft>, ()) {
    let issue = &lineage[0];
    let started = draft(issue.id, set_field("status", json!("in_progress")));
    let begun = Draft {
        project: issue.project,
        ..started
    };
    (vec![begun], ())
}

fn next(store: &mut Store, actor: &Actor, candidates: &[Uuid]) -> Option<Next<()>> {
    store.take_next(actor, candidates, None, begin).unwrap()
}

#[test]
fn the_first_eligible_issue_is_taken_and_the_rest_passed_over() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let ids: Vec<Uuid> = (0..7).map(|_| new_issue(&mut store)).collect();
    let [done, started, claimed, epic, archived, blocked, free] = ids[..] else {
        unreachable!()
    };
    store
        .append(draft(done, set_field("status", json!("done"))))
        .unwrap();
    store
        .append(draft(started, set_field("status", json!("in_progress"))))
        .unwrap();
    store
        .take(&Actor::agent("codex", "jace"), claimed, None)
        .unwrap();
    store
        .append(draft(epic, set_field("type", json!("epic"))))
        .unwrap();
    store
        .append(draft(archived, set_field("archived", json!(true))))
        .unwrap();
    store.link(&jace(), "blocks", free, blocked).unwrap();

    let taken = next(&mut store, &claude(), &ids).unwrap();
    assert_eq!(taken.issue, free);
    assert_eq!(taken.claim.as_ref().map(|claim| claim.issue), Some(free));
    let agent = held(&store, free).map(|claim| claim.agent);
    assert_eq!(agent.as_deref(), Some("claude"));
    assert_eq!(field(&store, free, "status"), Some(json!("in_progress")));
}

#[test]
fn a_lapsed_claim_is_taken_over() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    let codex = Actor::agent("codex", "jace");
    store.take(&codex, issue, Some(1_000)).unwrap();
    clock.advance(5_000);
    let taken = next(&mut store, &claude(), &[issue]).unwrap();
    assert_eq!(taken.issue, issue);
    let agent = held(&store, issue).map(|claim| claim.agent);
    assert_eq!(agent.as_deref(), Some("claude"));
}

#[test]
fn a_human_begins_without_a_claim_and_nothing_eligible_writes_nothing() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let taken = next(&mut store, &jace(), &[issue]).unwrap();
    assert_eq!((taken.issue, taken.claim), (issue, None));
    assert_eq!(held(&store, issue), None);

    let seq = |store: &Store| -> i64 {
        store
            .connection()
            .query_row("SELECT max(seq) FROM ops", [], |row| row.get(0))
            .unwrap()
    };
    let before = seq(&store);
    assert!(next(&mut store, &claude(), &[issue]).is_none());
    assert!(next(&mut store, &claude(), &[]).is_none());
    assert_eq!(seq(&store), before);
}

/// Set in child processes: the store, the candidates and the child's index.
const CHILD_DB: &str = "TM_STORE_TEST_NEXT_DB";
const CHILD_IDS: &str = "TM_STORE_TEST_NEXT_IDS";
const CHILD_INDEX: &str = "TM_STORE_TEST_NEXT_INDEX";
const TAKERS: usize = 12;

/// ADR 0016: Agents taking from one list at once take different Issues.
#[test]
fn agents_taking_at_once_take_different_issues() {
    if let (Ok(db), Ok(ids), Ok(index)) = (
        std::env::var(CHILD_DB),
        std::env::var(CHILD_IDS),
        std::env::var(CHILD_INDEX),
    ) {
        let parse = |id| Uuid::parse_str(id).unwrap();
        let ids: Vec<Uuid> = ids.split(',').map(parse).collect();
        let mut store = Store::open(&PathBuf::from(db)).unwrap();
        let session = SessionId::parse(&format!("session-{index}")).unwrap();
        let agent = claude().in_session(session);
        assert!(next(&mut store, &agent, &ids).is_some());
        return;
    }
    let dir = TempDir::new();
    let mut store = Store::open(&dir.db()).unwrap();
    crate::support::seed_projects(&mut store);
    let ids: Vec<Uuid> = (0..TAKERS).map(|_| new_issue(&mut store)).collect();
    drop(store);
    let joined: Vec<String> = ids.iter().map(Uuid::to_string).collect();
    let joined = joined.join(",");
    run_children(
        "next::agents_taking_at_once_take_different_issues",
        TAKERS,
        |i, command| {
            command
                .env(CHILD_DB, dir.db())
                .env(CHILD_IDS, &joined)
                .env(CHILD_INDEX, i.to_string());
        },
    );
    let store = Store::open(&dir.db()).unwrap();
    let sessions: Vec<Option<String>> = ids
        .iter()
        .map(|id| held(&store, *id).and_then(|claim| claim.session))
        .collect();
    let mut distinct: Vec<&String> = sessions.iter().flatten().collect();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), TAKERS, "{sessions:?}");
}
