//! An Issue's history: every operation that wrote it, in replay order.

use serde_json::json;
use tm_core::op::ClaimAction;
use tm_core::{Change, Hlc};
use uuid::Uuid;

use crate::support::{
    ManualClock, TempDir, draft, insert_acked, jace, new_issue, open, remote_op, set_field,
};

#[test]
fn history_runs_in_replay_order_with_links_unapplied_ops_and_undo() {
    let dir = TempDir::new();
    let clock = ManualClock::at(10_000);
    let mut store = open(&dir.db(), &clock);
    let (issue, other) = (Uuid::now_v7(), Uuid::now_v7());
    let link = Change::Link {
        link_type: "blocks".into(),
        target: issue,
        present: true,
    };
    // Priority holds one of five levels, so replay sets this aside.
    let bogus = set_field("priority", json!("bogus"));
    let remote = [
        remote_op(7, issue, &Change::Create, Hlc::new(1_000, 0), 0),
        remote_op(7, other, &Change::Create, Hlc::new(1_100, 0), 0),
        remote_op(7, issue, &bogus, Hlc::new(1_200, 0), 0),
        remote_op(7, other, &link, Hlc::new(1_300, 0), 0),
    ];
    for (seq, op) in (1..).zip(&remote) {
        insert_acked(&store, op, seq, 2_000);
    }
    store.rebuild().unwrap();

    let local = store
        .append(draft(issue, set_field("assignee", json!("jace"))))
        .unwrap();
    // A batch is named by its first operation.
    let batch = local.op_id;
    // Acknowledged after the local write arrived, but replay puts every
    // acknowledged operation first.
    let comment = Change::Comment {
        body: "late".into(),
    };
    let late = remote_op(7, issue, &comment, Hlc::new(1_400, 0), 0);
    insert_acked(&store, &late, 5, 2_000);
    store.rebuild().unwrap();
    let (_, undo) = store.undo(&jace(), batch).unwrap();

    let history = store.history(issue).unwrap();
    let kinds: Vec<&str> = history.iter().map(|entry| entry.change.kind()).collect();
    let set = "set_field";
    assert_eq!(kinds, ["create", set, "link", "comment", set, set]);
    assert_eq!(history[0].op_id, remote[0].op_id);
    assert!(history[1].unapplied.is_some(), "{:?}", history[1]);
    assert_eq!(history[2].subject, other, "filed under its other end");
    assert_eq!(history[3].op_id, late.op_id);
    assert_eq!(history[4].batch, Some(batch));
    assert_eq!(history[4].undone_by, Some(undo[0].op_id));
    assert_eq!(history[5].undid, Some(batch));
    assert!(history[..4].iter().all(|entry| entry.batch.is_none()));
    assert_eq!(store.history(Uuid::now_v7()).unwrap(), []);
}

#[test]
fn an_edit_that_loses_its_merge_says_so() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(10_000));
    let issue = Uuid::now_v7();
    let minute = 60_000;
    let create = remote_op(7, issue, &Change::Create, Hlc::new(minute, 0), 0);
    // Both saw only the create, so they are concurrent; the later clock wins
    // though it arrived first.
    let title = |text: &str| set_field("title", json!(text));
    let later = remote_op(7, issue, &title("later"), Hlc::new(6 * minute, 0), 1);
    let earlier = remote_op(8, issue, &title("earlier"), Hlc::new(4 * minute, 0), 1);
    let next = remote_op(7, issue, &title("next"), Hlc::new(7 * minute, 0), 3);
    let received = 6 * minute;
    for (seq, op) in (1..).zip([&create, &later, &earlier, &next]) {
        insert_acked(&store, op, seq, i64::try_from(received).unwrap());
    }
    store.rebuild().unwrap();
    let lost: Vec<bool> = store
        .history(issue)
        .unwrap()
        .iter()
        .map(|entry| entry.lost)
        .collect();
    assert_eq!(lost, [false, false, true, false]);
}

/// `changes` from Replicas 7, 8, 9…, each at its minute and all seeing only
/// the create, acknowledged in that order; which of them lost.
fn concurrent(changes: &[(Change, u64)]) -> Vec<bool> {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(10_000));
    let issue = Uuid::now_v7();
    let minute = 60_000;
    let create = remote_op(7, issue, &Change::Create, Hlc::new(minute, 0), 0);
    insert_acked(&store, &create, 1, 0);
    for ((change, at), (replica, seq)) in changes.iter().zip((7..).zip(2..)) {
        let op = remote_op(replica, issue, change, Hlc::new(at * minute, 0), 1);
        insert_acked(&store, &op, seq, i64::try_from(9 * minute).unwrap());
    }
    store.rebuild().unwrap();
    let history = store.history(issue).unwrap();
    history[1..].iter().map(|entry| entry.lost).collect()
}

#[test]
fn a_merge_replays_against_its_winner_per_element_and_label_case() {
    let title = |text: &str| set_field("title", json!(text));
    let three = [(title("six"), 6), (title("four"), 4), (title("five"), 5)];
    let against_the_winner = [false, true, true];
    assert_eq!(concurrent(&three), against_the_winner);
    let add = |elem: &str| Change::Add {
        field: "labels".into(),
        label: "Labels".into(),
        elem: elem.into(),
        elem_label: None,
    };
    let remove = |elem: &str| Change::Remove {
        field: "labels".into(),
        label: "Labels".into(),
        elem: elem.into(),
        elem_label: None,
    };
    // Each element merges alone, and labels merge without case.
    let two = [(add("a"), 6), (add("b"), 4)];
    assert_eq!(concurrent(&two), [false, false]);
    let case = [(add("UI"), 6), (remove("ui"), 4)];
    assert_eq!(concurrent(&case), [false, true]);
    // Priority holds one of five levels, so replay sets "bogus" aside, and an
    // operation set aside never wins.
    let priority = |text: &str| set_field("priority", json!(text));
    let unapplied = [(priority("bogus"), 6), (priority("high"), 4)];
    assert_eq!(concurrent(&unapplied), [false, false]);
}

#[test]
fn claims_apply_by_compare_and_set_not_by_merge() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(10_000));
    let issue = Uuid::now_v7();
    let minute = 60_000;
    let create = remote_op(7, issue, &Change::Create, Hlc::new(minute, 0), 0);
    let take = |agent: &str, prev| Change::Claim {
        action: ClaimAction::Take {
            agent: agent.into(),
            ttl_ms: None,
            prev,
            session: None,
            branch: None,
        },
    };
    let first = remote_op(7, issue, &take("claude", None), Hlc::new(5 * minute, 0), 1);
    let release = Change::Claim {
        action: ClaimAction::Release { claim: first.op_id },
    };
    let released = remote_op(7, issue, &release, Hlc::new(6 * minute, 0), 2);
    // Saw only the create, at an earlier clock: a merge would drop it, but
    // the Claim it names as current is still none, so it takes the Issue.
    let mut later = remote_op(8, issue, &take("codex", None), Hlc::new(4 * minute, 0), 1);
    later.offset_min = 330;
    for (seq, op) in (1..).zip([&create, &first, &released, &later]) {
        insert_acked(&store, op, seq, i64::try_from(9 * minute).unwrap());
    }
    store.rebuild().unwrap();
    let history = store.history(issue).unwrap();
    assert!(history.iter().all(|entry| !entry.lost), "{history:?}");
    assert_eq!(history[3].offset_min, 330, "its recorded offset");
}

#[test]
fn an_undone_batch_names_the_undo_that_stands() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(10_000));
    let issue = new_issue(&mut store);
    let batch = store
        .append(draft(issue, set_field("assignee", json!("jace"))))
        .unwrap()
        .op_id;
    let (_, first) = store.undo(&jace(), batch).unwrap();
    store.undo(&jace(), first[0].op_id).unwrap();
    let (_, again) = store.undo(&jace(), batch).unwrap();
    let history = store.history(issue).unwrap();
    let entry = history.iter().find(|entry| entry.op_id == batch).unwrap();
    assert_eq!(entry.undone_by, Some(again[0].op_id));

    // Undoing the undo of an undo leaves the first undo standing, as tm undo
    // decides it.
    let other = store
        .append(draft(issue, set_field("assignee", json!("ana"))))
        .unwrap()
        .op_id;
    let (_, u1) = store.undo(&jace(), other).unwrap();
    let (_, u2) = store.undo(&jace(), u1[0].op_id).unwrap();
    store.undo(&jace(), u2[0].op_id).unwrap();
    let history = store.history(issue).unwrap();
    let entry = history.iter().find(|entry| entry.op_id == other).unwrap();
    assert_eq!(entry.undone_by, Some(u1[0].op_id));
}

#[test]
fn a_link_to_a_missing_issue_still_wins_its_merge() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(10_000));
    let (issue, missing) = (Uuid::now_v7(), Uuid::now_v7());
    let minute = 60_000;
    let link = |present| Change::Link {
        link_type: "blocks".into(),
        target: missing,
        present,
    };
    let create = remote_op(7, issue, &Change::Create, Hlc::new(minute, 0), 0);
    let add = remote_op(7, issue, &link(true), Hlc::new(6 * minute, 0), 1);
    let remove = remote_op(8, issue, &link(false), Hlc::new(4 * minute, 0), 1);
    let received = i64::try_from(6 * minute).unwrap();
    for (seq, op) in (1..).zip([&create, &add, &remove]) {
        insert_acked(&store, op, seq, received);
    }
    store.rebuild().unwrap();
    let history = store.history(issue).unwrap();
    let (added, removed) = (&history[1], &history[2]);
    assert_eq!((added.unapplied.as_deref(), added.lost), (None, false));
    assert!(removed.lost, "{removed:?}");
}
