//! ADR 0006 merge rules, with acknowledged operations inserted the way sync
//! will deliver them.

use serde_json::json;
use tm_core::hlc::MAX_DRIFT_MS;
use tm_core::{Change, Hlc};
use tm_store::Store;
use uuid::Uuid;

use crate::support::OTHER;
use crate::support::{
    ManualClock, TempDir, draft, field, insert_acked, new_issue, open, remote_op, set_field,
};

const MINUTE: u64 = 60_000;

/// A store holding one Issue whose creation the server acknowledged as seq 1.
fn store_with_acked_issue(dir: &TempDir, clock: &ManualClock) -> (Store, Uuid) {
    let mut store = open(&dir.db(), clock);
    let issue = Uuid::now_v7();
    insert_acked(
        &store,
        &remote_op(7, issue, &Change::Create, Hlc::new(1_000, 0), 0),
        1,
        1_000,
    );
    store.rebuild().unwrap();
    (store, issue)
}

fn title(store: &Store, issue: Uuid) -> serde_json::Value {
    field(store, issue, "title").unwrap()
}

#[test]
fn an_edit_that_saw_the_winner_replaces_it_despite_a_lower_clock() {
    let dir = TempDir::new();
    let (mut store, issue) = store_with_acked_issue(&dir, &ManualClock::at(1_000));
    let seen = remote_op(
        7,
        issue,
        &set_field("title", json!("old")),
        Hlc::new(9 * MINUTE, 0),
        1,
    );
    let newer = remote_op(
        8,
        issue,
        &set_field("title", json!("new")),
        Hlc::new(5 * MINUTE, 0),
        2,
    );
    insert_acked(&store, &seen, 2, i64::try_from(9 * MINUTE).unwrap());
    insert_acked(&store, &newer, 3, i64::try_from(9 * MINUTE).unwrap());
    store.rebuild().unwrap();
    assert_eq!(title(&store, issue), json!("new"));
}

#[test]
fn concurrent_edits_go_to_the_later_clock() {
    let dir = TempDir::new();
    let (mut store, issue) = store_with_acked_issue(&dir, &ManualClock::at(1_000));
    let later = remote_op(
        7,
        issue,
        &set_field("title", json!("later")),
        Hlc::new(6 * MINUTE, 0),
        1,
    );
    let earlier = remote_op(
        8,
        issue,
        &set_field("title", json!("earlier")),
        Hlc::new(4 * MINUTE, 0),
        1,
    );
    insert_acked(&store, &later, 2, i64::try_from(6 * MINUTE).unwrap());
    insert_acked(&store, &earlier, 3, i64::try_from(6 * MINUTE).unwrap());
    store.rebuild().unwrap();
    assert_eq!(title(&store, issue), json!("later"));
}

#[test]
fn a_fast_clock_is_capped_at_receipt_plus_drift() {
    let dir = TempDir::new();
    let (mut store, issue) = store_with_acked_issue(&dir, &ManualClock::at(1_000));
    let received = 10 * MINUTE;
    let fast = remote_op(
        7,
        issue,
        &set_field("title", json!("fast")),
        Hlc::new(received + 600 * MINUTE, 0),
        1,
    );
    let honest = remote_op(
        8,
        issue,
        &set_field("title", json!("honest")),
        Hlc::new(received + MAX_DRIFT_MS + MINUTE, 0),
        1,
    );
    insert_acked(&store, &fast, 2, i64::try_from(received).unwrap());
    insert_acked(
        &store,
        &honest,
        3,
        i64::try_from(received + MAX_DRIFT_MS + MINUTE).unwrap(),
    );
    store.rebuild().unwrap();
    assert_eq!(title(&store, issue), json!("honest"));
}

#[test]
fn equal_clocks_are_tie_broken_by_replica() {
    let dir = TempDir::new();
    let (mut store, issue) = store_with_acked_issue(&dir, &ManualClock::at(1_000));
    let hlc = Hlc::new(5 * MINUTE, 3);
    let high = remote_op(9, issue, &set_field("title", json!("high")), hlc, 1);
    let low = remote_op(2, issue, &set_field("title", json!("low")), hlc, 1);
    insert_acked(&store, &high, 2, i64::try_from(5 * MINUTE).unwrap());
    insert_acked(&store, &low, 3, i64::try_from(5 * MINUTE).unwrap());
    store.rebuild().unwrap();
    assert_eq!(title(&store, issue), json!("high"));
}

#[test]
fn a_pending_local_edit_shows_over_an_acknowledged_one() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let (mut store, issue) = store_with_acked_issue(&dir, &clock);
    store
        .append(draft(issue, set_field("title", json!("mine"))))
        .unwrap();
    let theirs = remote_op(
        7,
        issue,
        &set_field("title", json!("theirs")),
        Hlc::new(60 * MINUTE, 0),
        1,
    );
    insert_acked(&store, &theirs, 2, i64::try_from(60 * MINUTE).unwrap());
    store.rebuild().unwrap();
    assert_eq!(title(&store, issue), json!("mine"));
}

#[test]
fn set_elements_follow_their_last_add_or_remove() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    for (add, elem) in [
        (true, "a"),
        (true, "b"),
        (false, "a"),
        (true, "c"),
        (false, "c"),
        (true, "c"),
    ] {
        clock.advance(1);
        let change = if add {
            Change::Add {
                field: "labels".into(),
                label: "Labels".into(),
                elem: elem.into(),
                elem_label: None,
            }
        } else {
            Change::Remove {
                field: "labels".into(),
                label: "Labels".into(),
                elem: elem.into(),
                elem_label: None,
            }
        };
        store.append(draft(issue, change)).unwrap();
    }
    let mut stmt = store
        .connection()
        .prepare("SELECT elem FROM set_members ORDER BY elem")
        .unwrap();
    let members: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(members, ["b", "c"]);
}

#[test]
fn base_seq_comes_from_the_drafts_own_project() {
    let dir = TempDir::new();
    let (mut store, issue) = store_with_acked_issue(&dir, &ManualClock::at(1_000));
    let mut other = remote_op(7, Uuid::now_v7(), &Change::Create, Hlc::new(2_000, 0), 0);
    other.project = OTHER;
    insert_acked(&store, &other, 100, 2_000);
    store.rebuild().unwrap();
    let op = store
        .append(draft(issue, set_field("title", json!("x"))))
        .unwrap();
    assert_eq!(op.base_seq, 1);
}
