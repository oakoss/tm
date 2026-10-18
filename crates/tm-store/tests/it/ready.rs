//! The ready queue: which Issues it holds, in what order, and what each would
//! unblock.

use serde_json::json;
use tm_store::{Ready, Store};
use uuid::Uuid;

use crate::support::{ManualClock, TempDir, claude, draft, jace, new_issue, open, set_field};

fn set(store: &mut Store, issue: Uuid, field: &str, value: serde_json::Value) {
    store.append(draft(issue, set_field(field, value))).unwrap();
}

fn blocks(store: &mut Store, from: Uuid, to: Uuid) {
    store.link(&jace(), "blocks", from, to).unwrap();
}

fn queue(store: &Store) -> Vec<Uuid> {
    store
        .ready()
        .unwrap()
        .into_iter()
        .map(|ready| ready.issue)
        .collect()
}

#[test]
fn only_unclaimed_unarchived_todo_work_is_ready() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let todo = new_issue(&mut store);
    let mut others = Vec::new();
    for (field, value) in [
        ("status", json!("triage")),
        ("status", json!("in_progress")),
        ("status", json!("done")),
        ("status", json!("cancelled")),
        ("archived", json!(true)),
    ] {
        let issue = new_issue(&mut store);
        set(&mut store, issue, field, value);
        others.push(issue);
    }
    let claimed = new_issue(&mut store);
    store.take(&claude(), claimed, None).unwrap();
    assert_eq!(queue(&store), [todo]);
}

#[test]
fn an_open_blocker_holds_an_issue_back_and_a_closed_one_does_not() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let held = new_issue(&mut store);
    let blocker = new_issue(&mut store);
    blocks(&mut store, blocker, held);
    assert_eq!(queue(&store), [blocker]);

    set(&mut store, blocker, "status", json!("in_progress"));
    assert_eq!(
        queue(&store),
        Vec::<Uuid>::new(),
        "an active blocker still blocks"
    );
    for status in ["done", "cancelled"] {
        set(&mut store, blocker, "status", json!(status));
        assert_eq!(queue(&store), [held], "{status}");
    }
    set(&mut store, blocker, "status", json!("todo"));
    set(&mut store, blocker, "archived", json!(true));
    assert_eq!(queue(&store), [held], "an archived blocker is gone");
}

#[test]
fn the_queue_is_most_urgent_first_then_oldest() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let mut made = Vec::new();
    for (ms, priority) in [
        (1, "none"),
        (2, "low"),
        (3, "urgent"),
        (4, "low"),
        (5, "high"),
    ] {
        clock.set(1_000 + ms);
        let issue = new_issue(&mut store);
        set(&mut store, issue, "priority", json!(priority));
        made.push(issue);
    }
    assert_eq!(queue(&store), [made[2], made[4], made[1], made[3], made[0]]);
}

#[test]
fn each_issue_names_what_it_alone_holds_back() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let a = new_issue(&mut store);
    let b = new_issue(&mut store);
    let only_a = new_issue(&mut store);
    let both = new_issue(&mut store);
    let finished = new_issue(&mut store);
    blocks(&mut store, a, only_a);
    blocks(&mut store, a, both);
    blocks(&mut store, b, both);
    blocks(&mut store, a, finished);
    set(&mut store, finished, "status", json!("done"));

    let ready = store.ready().unwrap();
    assert_eq!(
        ready,
        [
            Ready {
                issue: a,
                unblocks: vec![only_a]
            },
            Ready {
                issue: b,
                unblocks: vec![]
            },
        ]
    );
}

#[test]
fn a_blocking_cycle_left_by_a_merge_drops_both_without_looping() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let a = new_issue(&mut store);
    let b = new_issue(&mut store);
    let free = new_issue(&mut store);
    blocks(&mut store, a, b);
    // `link` refuses the closing Link, so write it as a merge would leave it.
    store
        .connection()
        .execute(
            "INSERT INTO links (type, src, dst) VALUES ('blocks', ?1, ?2)",
            [b.as_bytes(), a.as_bytes()],
        )
        .unwrap();
    assert_eq!(queue(&store), [free]);
}
