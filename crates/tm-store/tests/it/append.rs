use serde_json::json;
use tm_core::scheme::Category;
use tm_core::{Change, Hlc};
use tm_store::{Draft, StoreError};
use uuid::Uuid;

use crate::support::{ManualClock, TempDir, draft, field, new_issue, open, set_field};
use crate::support::{OTHER, PROJECT};

fn count_ops(store: &tm_store::Store) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM ops", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn creating_an_issue_writes_its_required_fields_in_its_starting_status() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (issue, ops) = store.create_issue("jace", PROJECT, "bug", "Crash").unwrap();
    let kinds: Vec<&str> = ops.iter().map(|op| op.kind.as_str()).collect();
    assert_eq!(kinds, ["create", "set_field", "set_field", "set_field"]);
    assert_eq!(count_ops(&store), 4);
    let view = store.issue(issue).unwrap().unwrap();
    assert_eq!(view.fields["type"], json!("bug"));
    assert_eq!(view.fields["status"], json!("todo"));
    assert_eq!(view.fields["reporter"], json!("jace"));
    assert_eq!(view.fields["priority"], json!("none"));
    assert_eq!(view.category, Some(Category::Todo));
}

#[test]
fn enumerated_values_record_their_display_label() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let op = store
        .append(draft(issue, set_field("status", json!("in_review"))))
        .unwrap();
    let Change::SetField {
        label, value_label, ..
    } = op.change().unwrap()
    else {
        panic!("not a set_field");
    };
    assert_eq!(label, "Status");
    assert_eq!(value_label.as_deref(), Some("In Review"));
}

#[test]
fn an_issue_without_its_required_fields_is_refused_and_nothing_is_logged() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let err = store
        .append(draft(Uuid::now_v7(), Change::Create))
        .unwrap_err();
    let StoreError::MissingFields { fields, .. } = err else {
        panic!("{err}");
    };
    assert_eq!(fields, ["type", "title", "status"]);
    assert_eq!(count_ops(&store), 0);
}

#[test]
fn a_change_to_a_missing_issue_is_refused_and_nothing_is_logged() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let err = store
        .append(draft(Uuid::now_v7(), set_field("title", json!("x"))))
        .unwrap_err();
    assert!(matches!(err, StoreError::UnknownIssue(_)), "{err}");
    assert_eq!(count_ops(&store), 0);
}

#[test]
fn creating_an_issue_twice_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let err = store.append(draft(issue, Change::Create)).unwrap_err();
    assert!(matches!(err, StoreError::DuplicateIssue(_)), "{err}");
}

#[test]
fn an_invalid_field_name_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let err = store
        .append(draft(issue, set_field("a.b", json!(1))))
        .unwrap_err();
    assert!(matches!(err, StoreError::Op(_)), "{err}");
}

#[test]
fn an_unknown_field_lists_the_fields_that_can_be_set() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let err = store
        .append(draft(issue, set_field("severity", json!("high"))))
        .unwrap_err();
    let StoreError::UnknownField { valid, .. } = err else {
        panic!("{err}");
    };
    assert!(valid.contains(&"title".to_owned()));
    assert!(valid.contains(&"area".to_owned()));
    assert!(!valid.contains(&"created".to_owned()));
}

#[test]
fn values_the_scheme_does_not_allow_are_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let cases = [
        set_field("status", json!("shipped")),
        set_field("type", json!("saga")),
        set_field("priority", json!("p0")),
        set_field("tier", json!("silver")),
        set_field("area", json!("ten")),
        set_field("title", json!("  ")),
        set_field("parent", json!(issue.to_string())),
        Change::Add {
            field: "title".into(),
            label: String::new(),
            elem: "x".into(),
            elem_label: None,
        },
    ];
    for change in cases {
        let shown = format!("{change:?}");
        let err = store.append(draft(issue, change)).unwrap_err();
        assert!(matches!(err, StoreError::Invalid { .. }), "{shown}: {err}");
    }
    let err = store
        .append(draft(issue, set_field("status", json!("shipped"))))
        .unwrap_err();
    let StoreError::Invalid { valid, .. } = err else {
        panic!("{err}");
    };
    assert!(valid.contains(&"in_progress".to_owned()));
}

#[test]
fn computed_and_multi_value_fields_name_why_they_cannot_be_set() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let cases = [
        ("reporter", json!("codex"), "is computed and cannot be set"),
        (
            "labels",
            json!("bug"),
            "holds several values; add or remove them",
        ),
    ];
    for (name, value, expected) in cases {
        let err = store
            .append(draft(issue, set_field(name, value)))
            .unwrap_err();
        let StoreError::Invalid { reason, .. } = err else {
            panic!("{name}: {err}");
        };
        assert_eq!(reason, expected, "{name}");
    }
}

#[test]
fn required_fields_cannot_be_cleared() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let err = store
        .append(draft(issue, set_field("title", serde_json::Value::Null)))
        .unwrap_err();
    assert!(matches!(err, StoreError::MissingFields { .. }), "{err}");
}

#[test]
fn scheme_fields_are_stored_and_cleared() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    store
        .append(draft(issue, set_field("area", json!(3))))
        .unwrap();
    store
        .append(draft(issue, set_field("tier", json!("gold"))))
        .unwrap();
    assert_eq!(field(&store, issue, "area"), Some(json!(3)));
    assert_eq!(field(&store, issue, "tier"), Some(json!("gold")));
    store
        .append(draft(issue, set_field("area", serde_json::Value::Null)))
        .unwrap();
    assert_eq!(field(&store, issue, "area"), None);
}

#[test]
fn labels_ignore_case_and_keep_their_written_form() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let add = |elem: &str| Change::Add {
        field: "labels".into(),
        label: String::new(),
        elem: elem.into(),
        elem_label: None,
    };
    store.append(draft(issue, add("Bug"))).unwrap();
    let op = store.append(draft(issue, add("bug"))).unwrap();
    assert_eq!(
        op.change().unwrap().elem(),
        Some("bug"),
        "stored lowercased"
    );
    let display: Option<String> = store
        .connection()
        .query_row("SELECT display FROM set_members", [], |r| r.get(0))
        .unwrap();
    assert_eq!(display, None, "the later add wrote the lowercase form");
    let sets = store.issue(issue).unwrap().unwrap().sets;
    assert_eq!(sets["labels"].len(), 1);
}

#[test]
fn a_batch_is_written_whole_or_not_at_all() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let before = count_ops(&store);
    let batch = vec![
        draft(issue, set_field("title", json!("renamed"))),
        draft(issue, set_field("status", json!("shipped"))),
    ];
    assert!(store.append_all(batch).is_err());
    assert_eq!(count_ops(&store), before);
    assert_eq!(field(&store, issue, "title"), Some(json!("t")));
}

#[test]
fn a_type_change_that_would_invalidate_the_status_is_refused() {
    let dir = TempDir::new();
    let mut two_workflows = crate::support::scheme();
    let mut bugs = two_workflows.workflows[0].clone();
    bugs.name = "bugs".into();
    bugs.statuses.retain(|status| status.id != "in_review");
    two_workflows.workflows.push(bugs);
    for issue_type in &mut two_workflows.types {
        if issue_type.id == "bug" {
            issue_type.workflow = "bugs".into();
        }
    }
    let mut store = open(&dir.db(), &ManualClock::at(1_000)).with_scheme(two_workflows);
    let issue = new_issue(&mut store);
    store
        .append(draft(issue, set_field("status", json!("in_review"))))
        .unwrap();
    let err = store
        .append(draft(issue, set_field("type", json!("bug"))))
        .unwrap_err();
    assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
    store
        .append(draft(issue, set_field("status", json!("todo"))))
        .unwrap();
    store
        .append(draft(issue, set_field("type", json!("bug"))))
        .unwrap();
}

#[test]
fn a_status_the_scheme_no_longer_defines_reads_as_triage() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    store
        .append(draft(issue, set_field("status", json!("done"))))
        .unwrap();
    assert_eq!(
        store.issue(issue).unwrap().unwrap().category,
        Some(Category::Done)
    );
    store
        .connection()
        .execute("UPDATE issues SET status = 'legacy'", [])
        .unwrap();
    assert_eq!(
        store.issue(issue).unwrap().unwrap().category,
        Some(Category::Triage)
    );
}

#[test]
fn every_change_advances_updated() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    clock.set(5_000);
    store
        .append(draft(issue, Change::Comment { body: "hi".into() }))
        .unwrap();
    assert_eq!(field(&store, issue, "created"), Some(json!(1_000)));
    assert_eq!(field(&store, issue, "updated"), Some(json!(5_000)));
}

#[test]
fn the_clock_never_moves_backwards_across_reopens() {
    let dir = TempDir::new();
    let clock = ManualClock::at(50_000);
    let (issue, first) = {
        let mut store = open(&dir.db(), &clock);
        let (issue, ops) = store.create_issue("jace", PROJECT, "task", "t").unwrap();
        (issue, ops.last().unwrap().hlc)
    };
    clock.set(10_000);
    let mut store = open(&dir.db(), &clock);
    let second = store
        .append(draft(issue, set_field("title", json!("a"))))
        .unwrap();
    assert!(second.hlc > first);
    assert_eq!(second.hlc, Hlc::new(50_000, 4));
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
    let (_, ops) = store.create_issue("jace", PROJECT, "task", "t").unwrap();
    for op in ops {
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
}

#[test]
fn a_stale_projection_version_is_rebuilt_on_open() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let issue = {
        let mut store = open(&dir.db(), &clock);
        let issue = new_issue(&mut store);
        store
            .append(draft(issue, set_field("status", json!("done"))))
            .unwrap();
        store
            .connection()
            .execute_batch(
                "PRAGMA user_version = 0; DELETE FROM field_versions; UPDATE issues SET status = NULL;",
            )
            .unwrap();
        issue
    };
    let store = open(&dir.db(), &clock);
    assert_eq!(field(&store, issue, "status"), Some(json!("done")));
}

#[test]
fn a_draft_may_name_any_project_for_a_new_issue() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (issue, _) = store.create_issue("jace", OTHER, "task", "t").unwrap();
    let draft = Draft {
        actor: "jace".into(),
        project: OTHER,
        issue_id: issue,
        change: set_field("title", json!("u")),
    };
    store.append(draft).unwrap();
    assert_eq!(store.issue(issue).unwrap().unwrap().project, OTHER);
}
