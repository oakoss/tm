//! Each kind of field: what the Scheme check accepts, and how it reads back.

use serde_json::{Value, json};
use tm_core::scheme::{Field, FieldType};
use tm_core::{Change, Hlc};
use tm_store::{Store, StoreError};
use uuid::Uuid;

use crate::support::{
    ManualClock, TempDir, draft, field, insert_acked, jace, new_issue, open, remote_op, scheme,
    set_field,
};
use crate::support::{OTHER, PROJECT};

fn add(field: &str, elem: &str) -> Change {
    Change::Add {
        field: field.into(),
        label: String::new(),
        elem: elem.into(),
        elem_label: None,
    }
}

fn remove(field: &str, elem: &str) -> Change {
    Change::Remove {
        field: field.into(),
        label: String::new(),
        elem: elem.into(),
        elem_label: None,
    }
}

fn set(store: &mut Store, issue: Uuid, name: &str, value: Value) -> Result<(), StoreError> {
    store
        .append(draft(issue, set_field(name, value)))
        .map(|_| ())
}

#[test]
fn dates_and_timestamps_must_parse() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    set(&mut store, issue, "due", json!("2026-10-17")).unwrap();
    set(&mut store, issue, "seen", json!("2026-10-17T12:00:00Z")).unwrap();
    for (name, bad) in [
        ("due", "17/10/2026"),
        ("due", "20261017"),
        ("due", "2026-10-17T00:00"),
        ("seen", "yesterday"),
    ] {
        let err = set(&mut store, issue, name, json!(bad)).unwrap_err();
        assert!(matches!(err, StoreError::Invalid { .. }), "{name}: {err}");
    }
    assert_eq!(field(&store, issue, "due"), Some(json!("2026-10-17")));
}

#[test]
fn booleans_and_whole_numbers_read_back_as_written() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    set(&mut store, issue, "flag", json!(true)).unwrap();
    set(&mut store, issue, "area", json!(3)).unwrap();
    assert_eq!(field(&store, issue, "flag"), Some(json!(true)));
    assert_eq!(field(&store, issue, "area"), Some(json!(3)));
    set(&mut store, issue, "area", json!(2.5)).unwrap();
    assert_eq!(field(&store, issue, "area"), Some(json!(2.5)));
}

#[test]
fn integers_an_f64_cannot_hold_exactly_are_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    set(&mut store, issue, "area", json!(1_u64 << 53)).unwrap();
    let rounds_to_the_limit: Value = serde_json::from_str("9007199254740993e0").unwrap();
    for too_large in [
        json!((1_u64 << 53) + 1),
        json!(1e16),
        json!(-1e16),
        rounds_to_the_limit,
    ] {
        let err = set(&mut store, issue, "area", too_large.clone()).unwrap_err();
        assert!(
            matches!(err, StoreError::Invalid { .. }),
            "{too_large}: {err}"
        );
    }
}

#[test]
fn multi_select_elements_must_be_options_and_carry_their_label() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let op = store.append(draft(issue, add("tags", "x"))).unwrap();
    let Change::Add { elem_label, .. } = op.change().unwrap() else {
        panic!("not an add");
    };
    assert_eq!(elem_label.as_deref(), Some("Ex"));
    let err = store.append(draft(issue, add("tags", "z"))).unwrap_err();
    assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
    let sets = store.issue(issue).unwrap().unwrap().sets;
    assert_eq!(sets["tags"]["x"], "Ex");
}

#[test]
fn labels_show_the_spelling_they_were_added_with() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    store.append(draft(issue, add("labels", "Bug"))).unwrap();
    store.append(draft(issue, add("labels", "ui"))).unwrap();
    let sets = store.issue(issue).unwrap().unwrap().sets;
    assert_eq!(sets["labels"]["bug"], "Bug");
    assert_eq!(sets["labels"]["ui"], "ui");
    store.append(draft(issue, remove("labels", "BUG"))).unwrap();
    let sets = store.issue(issue).unwrap().unwrap().sets;
    assert!(!sets["labels"].contains_key("bug"));
}

#[test]
fn empty_labels_and_control_characters_are_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    for bad in ["", "   ", "a\nb", "tab\there"] {
        let err = store.append(draft(issue, add("labels", bad))).unwrap_err();
        assert!(matches!(err, StoreError::Invalid { .. }), "{bad:?}: {err}");
    }
}

#[test]
fn a_parent_must_be_another_existing_issue() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let parent = new_issue(&mut store);
    let child = new_issue(&mut store);
    set(&mut store, child, "parent", json!(parent.to_string())).unwrap();
    assert_eq!(
        field(&store, child, "parent"),
        Some(json!(parent.to_string()))
    );
    let err = set(
        &mut store,
        child,
        "parent",
        json!(Uuid::now_v7().to_string()),
    )
    .unwrap_err();
    assert!(matches!(err, StoreError::UnknownIssue(_)), "{err}");
}

#[test]
fn a_parent_that_closes_a_cycle_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let [a, b, c, d] = [(); 4].map(|()| new_issue(&mut store));
    set(&mut store, b, "parent", json!(a.to_string())).unwrap();
    set(&mut store, c, "parent", json!(b.to_string())).unwrap();
    let err = set(&mut store, a, "parent", json!(c.to_string())).unwrap_err();
    let StoreError::Cycle { relation, path } = err else {
        panic!("{err}");
    };
    assert_eq!(relation, "parent");
    assert_eq!(path, [a, c, b, a]);
    assert_eq!(field(&store, a, "parent"), None);
    set(&mut store, a, "parent", json!(d.to_string())).unwrap();
    set(&mut store, c, "parent", json!(d.to_string())).unwrap();
}

#[test]
fn a_parent_cycle_built_within_one_batch_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let [a, b] = [(); 2].map(|()| new_issue(&mut store));
    let err = store
        .append_all(vec![
            draft(b, set_field("parent", json!(a.to_string()))),
            draft(a, set_field("parent", json!(b.to_string()))),
        ])
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Cycle { path, .. } if *path == [a, b, a]),
        "{err}"
    );
    assert_eq!(field(&store, b, "parent"), None, "the batch rolled back");
}

#[test]
fn a_parent_cycle_from_a_merge_is_kept_and_writes_still_finish() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let [a, b] = [Uuid::now_v7(), Uuid::now_v7()];
    let ops = [
        (a, Change::Create),
        (b, Change::Create),
        (a, set_field("parent", json!(b.to_string()))),
        (b, set_field("parent", json!(a.to_string()))),
    ];
    for (seq, (issue, change)) in (1..).zip(ops) {
        let op = remote_op(7, issue, &change, Hlc::new(1_000 + seq, 0), 0);
        insert_acked(&store, &op, i64::try_from(seq).unwrap(), 2_000);
    }
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(field(&store, a, "parent"), Some(json!(b.to_string())));
    assert_eq!(field(&store, b, "parent"), Some(json!(a.to_string())));
    let child = new_issue(&mut store);
    set(&mut store, child, "parent", json!(a.to_string())).unwrap();
    set(&mut store, a, "title", json!("still editable")).unwrap();
}

#[test]
fn adding_a_held_label_or_removing_a_missing_one_is_logged() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let remove = |elem: &str| Change::Remove {
        field: "labels".into(),
        label: String::new(),
        elem: elem.into(),
        elem_label: None,
    };
    store.append(draft(issue, add("labels", "x"))).unwrap();
    store.append(draft(issue, add("labels", "x"))).unwrap();
    store.append(draft(issue, remove("x"))).unwrap();
    store.append(draft(issue, remove("x"))).unwrap();
    let logged: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM ops WHERE field = 'labels'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(logged, 4);
    assert_eq!(field(&store, issue, "labels"), None);
}

#[test]
fn archiving_can_be_set_and_cleared() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    set(&mut store, issue, "archived", json!(true)).unwrap();
    assert_eq!(field(&store, issue, "archived"), Some(json!(true)));
    set(&mut store, issue, "archived", Value::Null).unwrap();
    assert_eq!(field(&store, issue, "archived"), Some(json!(false)));
}

#[test]
fn every_required_field_is_enforced() {
    let dir = TempDir::new();
    let mut required = scheme();
    let mut team = Field::custom("team", "Team", FieldType::Text, Vec::new());
    team.required = true;
    required.add_field(team).unwrap();
    let mut store = open(&dir.db(), &ManualClock::at(1_000)).with_scheme(required);
    let err = store
        .create_issue(&jace(), PROJECT, "task", "t")
        .unwrap_err();
    let StoreError::MissingFields { fields, .. } = err else {
        panic!("{err}");
    };
    assert_eq!(fields, ["team"]);
}

#[test]
fn a_required_set_field_needs_an_element() {
    let dir = TempDir::new();
    let mut required = scheme();
    for field in &mut required.fields {
        if field.name == "labels" {
            field.required = true;
        }
    }
    let mut store = open(&dir.db(), &ManualClock::at(1_000)).with_scheme(required);
    let err = store
        .create_issue(&jace(), PROJECT, "task", "t")
        .unwrap_err();
    let StoreError::MissingFields { fields, .. } = err else {
        panic!("{err}");
    };
    assert_eq!(fields, ["labels"]);
}

#[test]
fn clearing_the_type_in_a_batch_cannot_slip_a_status_past_the_new_workflow() {
    let dir = TempDir::new();
    let mut two_workflows = scheme();
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
    set(&mut store, issue, "status", json!("in_review")).unwrap();
    let batch = vec![
        draft(issue, set_field("type", Value::Null)),
        draft(issue, set_field("type", json!("bug"))),
    ];
    let err = store.append_all(batch).unwrap_err();
    assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
    assert_eq!(field(&store, issue, "type"), Some(json!("task")));
}

#[test]
fn labels_logged_with_capitals_replay_lowercased() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let ops = [
        Change::Create,
        set_field("type", json!("task")),
        set_field("title", json!("t")),
        set_field("status", json!("todo")),
        add("labels", "Bug"),
    ];
    for (seq, change) in (1..).zip(&ops) {
        let op = remote_op(7, issue, change, Hlc::new(1_000, 0), 0);
        insert_acked(&store, &op, seq, 1_000);
    }
    assert_eq!(store.rebuild().unwrap(), 0);
    let sets = store.issue(issue).unwrap().unwrap().sets;
    assert_eq!(sets["labels"]["bug"], "Bug");
    store.append(draft(issue, remove("labels", "bug"))).unwrap();
    assert!(
        !store
            .issue(issue)
            .unwrap()
            .unwrap()
            .sets
            .contains_key("labels")
    );
}

#[test]
fn values_the_scheme_stopped_defining_can_still_be_cleared() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let issue = {
        let mut store = open(&dir.db(), &clock);
        let issue = new_issue(&mut store);
        set(&mut store, issue, "area", json!(3)).unwrap();
        store.append(draft(issue, add("tags", "x"))).unwrap();
        issue
    };
    let mut narrowed = scheme();
    narrowed.fields.retain(|field| field.name != "area");
    for field in &mut narrowed.fields {
        field.options.retain(|choice| choice.id != "x");
    }
    let mut store = open(&dir.db(), &clock).with_scheme(narrowed);
    set(&mut store, issue, "area", Value::Null).unwrap();
    store.append(draft(issue, remove("tags", "x"))).unwrap();
    let view = store.issue(issue).unwrap().unwrap();
    assert!(!view.fields.contains_key("area"));
    assert!(!view.sets.contains_key("tags"));
    let err = set(&mut store, issue, "area", json!(4)).unwrap_err();
    assert!(matches!(err, StoreError::UnknownField { .. }), "{err}");
}

#[test]
fn an_issue_with_an_unknown_type_and_status_is_repaired_by_its_type() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let ops = [
        Change::Create,
        set_field("type", json!("feature")),
        set_field("title", json!("t")),
        set_field("status", json!("shipped")),
    ];
    for (seq, change) in (1..).zip(&ops) {
        let op = remote_op(7, issue, change, Hlc::new(1_000, 0), 0);
        insert_acked(&store, &op, seq, 1_000);
    }
    store.rebuild().unwrap();
    set(&mut store, issue, "type", json!("task")).unwrap();
    set(&mut store, issue, "status", json!("todo")).unwrap();
    assert_eq!(field(&store, issue, "status"), Some(json!("todo")));
}

#[test]
fn updated_never_moves_back_for_an_older_operation() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let ops = [
        (Change::Create, 1_000),
        (set_field("title", json!("late")), 9_000),
        (set_field("title", json!("early")), 2_000),
    ];
    for (seq, (change, ms)) in (1..).zip(&ops) {
        let op = remote_op(7, issue, change, Hlc::new(*ms, 0), 0);
        insert_acked(&store, &op, seq, 9_000);
    }
    store.rebuild().unwrap();
    assert_eq!(field(&store, issue, "updated"), Some(json!(9_000)));
}

#[test]
fn replay_records_operations_that_do_not_fit_instead_of_failing() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let created = [
        Change::Create,
        set_field("type", json!("task")),
        set_field("title", json!("t")),
        set_field("status", json!("todo")),
    ];
    let mut seq = 0;
    for change in &created {
        seq += 1;
        insert_acked(
            &store,
            &remote_op(7, issue, change, Hlc::new(1_000, 0), 0),
            seq,
            1_000,
        );
    }
    store.rebuild().unwrap();
    let before = store.issue(issue).unwrap().unwrap();

    let at = |n| Hlc::new(2_000, n);
    let mut other_project = remote_op(7, issue, &set_field("title", json!("x")), at(4), 0);
    other_project.project = OTHER;
    let bad = [
        remote_op(7, issue, &set_field("reporter", json!("codex")), at(0), 0),
        remote_op(7, issue, &set_field("priority", json!("p0")), at(1), 0),
        remote_op(7, issue, &set_field("title", json!(5)), at(2), 0),
        remote_op(7, Uuid::now_v7(), &set_field("title", json!("x")), at(3), 0),
        other_project,
    ];
    for op in &bad {
        seq += 1;
        insert_acked(&store, op, seq, 2_000);
    }
    assert_eq!(store.rebuild().unwrap(), bad.len());

    let unapplied = store.unapplied().unwrap();
    let ids: Vec<Uuid> = unapplied.iter().map(|row| row.op_id).collect();
    assert_eq!(ids, bad.iter().map(|op| op.op_id).collect::<Vec<_>>());
    let reasons: Vec<String> = unapplied.into_iter().map(|row| row.reason).collect();
    let expected = [
        "is computed and cannot be set",
        "is not a priority level",
        "has the wrong value type",
        "does not exist",
        "belongs to Project 00000000-0000-0000-0000-000000000001",
    ];
    assert_eq!(reasons.len(), expected.len(), "{reasons:?}");
    for (reason, fragment) in reasons.iter().zip(expected) {
        assert!(reason.contains(fragment), "{reason:?} lacks {fragment:?}");
    }
    assert_eq!(store.issue(issue).unwrap().unwrap(), before);
}

#[test]
fn a_store_from_the_previous_projection_version_is_rebuilt() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let issue = {
        let mut store = open(&dir.db(), &clock);
        let issue = new_issue(&mut store);
        store
            .connection()
            .execute_batch(
                "PRAGMA user_version = 1; DELETE FROM field_versions; UPDATE issues SET title = NULL;",
            )
            .unwrap();
        issue
    };
    let store = open(&dir.db(), &clock);
    assert_eq!(field(&store, issue, "title"), Some(json!("t")));
}
