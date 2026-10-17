use serde_json::json;
use tm_core::{Change, Hlc};
use tm_store::{StoreError, issue_fields};
use uuid::Uuid;

use crate::support::{ManualClock, TempDir, draft, open, set_field};

#[test]
fn a_write_lands_in_the_log_and_the_projection_together() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = Uuid::now_v7();
    store.append(draft(issue, Change::Create)).unwrap();
    store
        .append(draft(issue, set_field("status", json!("active"))))
        .unwrap();

    let ops: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM ops", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ops, 2);
    let fields = issue_fields(store.connection(), issue).unwrap().unwrap();
    assert_eq!(fields, r#"{"status":"active"}"#);
}

#[test]
fn a_change_to_a_missing_issue_is_refused_and_nothing_is_logged() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let err = store
        .append(draft(Uuid::now_v7(), set_field("status", json!("x"))))
        .unwrap_err();
    assert!(matches!(err, StoreError::UnknownIssue(_)));
    let ops: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM ops", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ops, 0);
}

#[test]
fn creating_an_issue_twice_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    store.append(draft(issue, Change::Create)).unwrap();
    let err = store.append(draft(issue, Change::Create)).unwrap_err();
    assert!(matches!(err, StoreError::DuplicateIssue(_)));
}

#[test]
fn an_invalid_field_name_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    store.append(draft(issue, Change::Create)).unwrap();
    let err = store
        .append(draft(issue, set_field("a.b", json!(1))))
        .unwrap_err();
    assert!(matches!(err, StoreError::Op(_)));
}

#[test]
fn null_clears_a_field() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    store.append(draft(issue, Change::Create)).unwrap();
    store
        .append(draft(issue, set_field("area", json!("cli"))))
        .unwrap();
    store
        .append(draft(issue, set_field("area", serde_json::Value::Null)))
        .unwrap();
    assert_eq!(
        issue_fields(store.connection(), issue).unwrap().unwrap(),
        "{}"
    );
}

#[test]
fn the_clock_never_moves_backwards_across_reopens() {
    let dir = TempDir::new();
    let clock = ManualClock::at(50_000);
    let issue = Uuid::now_v7();
    let first = {
        let mut store = open(&dir.db(), &clock);
        store.append(draft(issue, Change::Create)).unwrap()
    };
    clock.set(10_000);
    let mut store = open(&dir.db(), &clock);
    let second = store
        .append(draft(issue, set_field("status", json!("a"))))
        .unwrap();
    assert!(second.hlc > first.hlc);
    assert_eq!(second.hlc, Hlc::new(50_000, 1));
}

#[test]
fn the_replica_id_is_stable_across_reopens() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let first = open(&dir.db(), &clock).replica_id().unwrap();
    let second = open(&dir.db(), &clock).replica_id().unwrap();
    assert_eq!(first, second);
}

#[test]
fn the_stored_hash_matches_the_operation() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let op = store.append(draft(Uuid::now_v7(), Change::Create)).unwrap();
    let stored: Vec<u8> = store
        .connection()
        .query_row(
            "SELECT op_hash FROM ops WHERE op_id = ?1",
            [op.op_id.as_bytes()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, op.op_hash());
}

#[test]
fn a_stale_projection_version_is_rebuilt_on_open() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let issue = Uuid::now_v7();
    {
        let mut store = open(&dir.db(), &clock);
        store.append(draft(issue, Change::Create)).unwrap();
        store
            .append(draft(issue, set_field("status", json!("done"))))
            .unwrap();
        store
            .connection()
            .execute_batch("PRAGMA user_version = 0; DELETE FROM field_versions; UPDATE issues SET fields = '{}';")
            .unwrap();
    }
    let store = open(&dir.db(), &clock);
    assert_eq!(
        issue_fields(store.connection(), issue).unwrap().unwrap(),
        r#"{"status":"done"}"#
    );
}
