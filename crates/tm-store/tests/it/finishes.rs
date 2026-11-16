//! Finishing an Issue: its move into done or cancelled releases its Claim and
//! clears its question in the same batch, read inside the write (ADR 0009).

use serde_json::json;
use tm_core::Actor;
use tm_store::{StoreError, WriteOptions};

use crate::support::{
    ManualClock, PROJECT, TempDir, claude, draft, field, held, jace, meanwhile, new_issue, open,
    set_field, unpassed,
};

fn done(issue: uuid::Uuid) -> Vec<tm_store::Draft> {
    vec![draft(issue, set_field("status", json!("done")))]
}

#[test]
fn a_finish_releases_the_claim_held_at_its_write_not_one_read_before() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    let read = store.take(&claude(), issue, None).unwrap();
    meanwhile((&dir.db(), &clock), 0, move |other| {
        other.release(&claude(), issue, read.id).unwrap();
        other
            .take(&Actor::agent("codex", "jace"), issue, None)
            .unwrap();
    });
    let unguarded = (false, &WriteOptions::default());
    unpassed(
        store
            .finish(&jace(), (PROJECT, issue), done(issue), unguarded)
            .unwrap(),
    );
    assert_eq!(store.claim_state(issue).unwrap(), None);
    assert_eq!(field(&store, issue, "status"), Some(json!("done")));
}

#[test]
fn an_agents_finish_is_refused_for_a_claim_taken_before_its_write() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    meanwhile((&dir.db(), &clock), 0, move |other| {
        other
            .take(&Actor::agent("codex", "jace"), issue, None)
            .unwrap();
    });
    let guarded = (true, &WriteOptions::default());
    let err = store
        .finish(&claude(), (PROJECT, issue), done(issue), guarded)
        .unwrap_err();
    let StoreError::AlreadyClaimed {
        issue: named,
        holder,
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(holder.agent, "codex");
    assert_eq!(named.key.as_deref(), Some("CPL-1"), "named by its Key");
    assert_eq!(field(&store, issue, "status"), Some(json!("todo")));
    let forced = (false, &WriteOptions::default());
    unpassed(
        store
            .finish(&claude(), (PROJECT, issue), done(issue), forced)
            .unwrap(),
    );
    assert_eq!(held(&store, issue), None);
}

#[test]
fn a_finish_clears_the_question_the_issue_waits_on() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    unpassed(
        store
            .append(draft(issue, set_field("waiting_on", json!("jace"))))
            .unwrap(),
    );
    let unguarded = (false, &WriteOptions::default());
    unpassed(
        store
            .finish(&jace(), (PROJECT, issue), done(issue), unguarded)
            .unwrap(),
    );
    assert_eq!(field(&store, issue, "waiting_on"), None);
}

#[test]
fn a_finish_with_nothing_left_to_write_writes_nothing_but_judges_its_version() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let read = store.issue_version(issue).unwrap();
    let options = WriteOptions {
        based_on: Some(read),
        ..WriteOptions::default()
    };
    let ops = store.finish(&claude(), (PROJECT, issue), Vec::new(), (true, &options));
    assert_eq!(unpassed(ops.unwrap()), []);
    let ahead = WriteOptions {
        based_on: Some(read + 100),
        ..WriteOptions::default()
    };
    let err = store.finish(&claude(), (PROJECT, issue), Vec::new(), (true, &ahead));
    assert!(matches!(err, Err(StoreError::Invalid { .. })), "{err:?}");
}
