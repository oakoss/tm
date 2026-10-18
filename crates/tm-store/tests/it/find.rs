//! Filters run end to end: text in, matching Issues out, in order.

use serde_json::{Value, json};
use tm_core::filter::{Context, FilterError};
use tm_core::{Change, Hlc};
use tm_store::{Store, StoreError};
use uuid::Uuid;

use crate::support::PROJECT;
use crate::support::{ManualClock, TempDir, draft, insert_acked, open, remote_op, set_field};

const ME: Context<'static> = Context { me: Some("jace") };

/// Writes an Issue with `fields` set and `labels` added, advancing the clock
/// first so later Issues are more recently updated.
fn issue(
    store: &mut Store,
    clock: &ManualClock,
    fields: &[(&str, Value)],
    labels: &[&str],
) -> Uuid {
    clock.advance(1_000);
    let (issue, _) = store.create_issue("jace", PROJECT, "task", "t").unwrap();
    for (name, value) in fields {
        store
            .append(draft(issue, set_field(name, value.clone())))
            .unwrap();
    }
    for label in labels {
        let add = Change::Add {
            field: "labels".into(),
            label: String::new(),
            elem: (*label).into(),
            elem_label: None,
        };
        store.append(draft(issue, add)).unwrap();
    }
    issue
}

fn find(store: &Store, filter: &str) -> Vec<Uuid> {
    store.find(filter, ME).unwrap()
}

fn sorted(mut ids: Vec<Uuid>) -> Vec<Uuid> {
    ids.sort();
    ids
}

#[test]
fn a_comma_list_is_any_of_and_repeated_qualifiers_are_all_of() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let both = issue(&mut store, &clock, &[], &["ui", "bug"]);
    let ui = issue(&mut store, &clock, &[], &["ui"]);
    let none = issue(&mut store, &clock, &[], &[]);
    assert_eq!(
        sorted(find(&store, "labels:ui,bug")),
        sorted(vec![both, ui])
    );
    assert_eq!(find(&store, "labels:ui labels:bug"), [both]);
    assert_eq!(sorted(find(&store, "-labels:bug")), sorted(vec![ui, none]));
}

#[test]
fn negation_matches_issues_with_no_value() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let mine = issue(
        &mut store,
        &clock,
        &[("assignee", json!("jace")), ("tier", json!("gold"))],
        &[],
    );
    let theirs = issue(
        &mut store,
        &clock,
        &[("assignee", json!("codex")), ("tier", json!("free"))],
        &[],
    );
    let unset = issue(&mut store, &clock, &[], &[]);
    assert_eq!(
        sorted(find(&store, "-assignee:jace")),
        sorted(vec![theirs, unset])
    );
    assert_eq!(
        sorted(find(&store, "-tier:gold")),
        sorted(vec![theirs, unset])
    );
    assert_eq!(find(&store, "assignee:@me"), [mine]);
    assert_eq!(find(&store, "no:assignee"), [unset]);
    assert_eq!(sorted(find(&store, "has:tier")), sorted(vec![mine, theirs]));
}

#[test]
fn scheme_values_match_by_type() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let three = issue(
        &mut store,
        &clock,
        &[("area", json!(3)), ("flag", json!(true))],
        &[],
    );
    let half = issue(
        &mut store,
        &clock,
        &[("area", json!(2.5)), ("due", json!("2026-10-17"))],
        &[],
    );
    let add = Change::Add {
        field: "tags".into(),
        label: String::new(),
        elem: "x".into(),
        elem_label: None,
    };
    store.append(draft(half, add)).unwrap();
    assert_eq!(find(&store, "area:3"), [three]);
    assert_eq!(find(&store, "area:2.5"), [half]);
    assert_eq!(find(&store, "flag:true"), [three]);
    assert_eq!(find(&store, "due:2026-10-17"), [half]);
    assert_eq!(find(&store, "tags:X"), [half]);
}

#[test]
fn categories_follow_the_scheme_and_unknown_statuses_read_as_triage() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let active = issue(&mut store, &clock, &[("status", json!("in_review"))], &[]);
    let done = issue(&mut store, &clock, &[("status", json!("done"))], &[]);
    let triage = issue(&mut store, &clock, &[("status", json!("triage"))], &[]);
    let legacy = issue(&mut store, &clock, &[], &[]);
    store
        .connection()
        .execute(
            "UPDATE issues SET status = 'legacy' WHERE id = ?1",
            [legacy.as_bytes()],
        )
        .unwrap();
    assert_eq!(find(&store, "category:active"), [active]);
    assert_eq!(
        sorted(find(&store, "category:triage")),
        sorted(vec![triage, legacy])
    );
    assert_eq!(
        sorted(find(&store, "-category:triage")),
        sorted(vec![active, done])
    );
    assert_eq!(
        sorted(find(&store, "-category:done,cancelled")),
        sorted(vec![active, triage, legacy])
    );
}

#[test]
fn issues_missing_a_type_or_status_from_other_replicas_still_categorize() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let untyped = Uuid::now_v7();
    let unstatused = Uuid::now_v7();
    let ops = [
        (untyped, Change::Create),
        (untyped, set_field("status", json!("todo"))),
        (unstatused, Change::Create),
        (unstatused, set_field("type", json!("task"))),
    ];
    for (seq, (issue, change)) in (1..).zip(&ops) {
        let op = remote_op(7, *issue, change, Hlc::new(1_000, 0), 0);
        insert_acked(&store, &op, seq, 1_000);
    }
    store.rebuild().unwrap();
    assert_eq!(find(&store, "category:triage"), [untyped]);
    assert_eq!(find(&store, "category:todo"), Vec::<Uuid>::new());
    assert_eq!(
        sorted(find(&store, "-category:done")),
        sorted(vec![untyped, unstatused])
    );
}

#[test]
fn sorts_follow_natural_directions_with_unset_values_last() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let low = issue(
        &mut store,
        &clock,
        &[("priority", json!("low")), ("tier", json!("free"))],
        &[],
    );
    let none = issue(&mut store, &clock, &[], &[]);
    let urgent = issue(
        &mut store,
        &clock,
        &[("priority", json!("urgent")), ("tier", json!("gold"))],
        &[],
    );
    assert_eq!(
        find(&store, ""),
        [urgent, none, low],
        "default: most recently updated"
    );
    assert_eq!(find(&store, "sort:priority"), [urgent, low, none]);
    assert_eq!(find(&store, "sort:priority-asc"), [low, urgent, none]);
    assert_eq!(find(&store, "sort:tier"), [urgent, low, none]);
    assert_eq!(find(&store, "sort:tier-desc"), [low, urgent, none]);
    assert_eq!(find(&store, "sort:created-asc"), [low, none, urgent]);
}

#[test]
fn archived_issues_are_left_out_unless_named() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let kept = issue(&mut store, &clock, &[], &[]);
    let archived = issue(&mut store, &clock, &[("archived", json!(true))], &[]);
    assert_eq!(find(&store, ""), [kept]);
    assert_eq!(find(&store, "archived:true"), [archived]);
    assert_eq!(find(&store, "archived:false"), [kept]);
    let err = store.find("archived:maybe", ME).unwrap_err();
    assert!(
        matches!(err, StoreError::Filter(FilterError::InvalidValue { .. })),
        "{err}"
    );
}

#[test]
fn people_match_and_sort_without_case_beyond_ascii() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let capital = issue(&mut store, &clock, &[("assignee", json!("Jace"))], &[]);
    let accented = issue(&mut store, &clock, &[("assignee", json!("Élodie"))], &[]);
    let lower = issue(&mut store, &clock, &[("assignee", json!("jace"))], &[]);
    let codex = issue(&mut store, &clock, &[("assignee", json!("codex"))], &[]);
    assert_eq!(
        sorted(find(&store, "assignee:@me")),
        sorted(vec![capital, lower])
    );
    assert_eq!(find(&store, "assignee:élodie"), [accented]);
    let sharp = issue(&mut store, &clock, &[("assignee", json!("Straße"))], &[]);
    assert_eq!(find(&store, "assignee:STRASSE"), [sharp]);
    store
        .append(draft(sharp, set_field("assignee", serde_json::Value::Null)))
        .unwrap();
    assert_eq!(
        sorted(find(&store, "-assignee:ÉLODIE")),
        sorted(vec![capital, lower, codex, sharp])
    );
    let order = find(&store, "sort:assignee");
    let jaces: Vec<usize> = [capital, lower]
        .iter()
        .map(|id| order.iter().position(|o| o == id).unwrap())
        .collect();
    assert_eq!(
        jaces[0].abs_diff(jaces[1]),
        1,
        "Jace and jace sort together: {order:?}"
    );
    assert_eq!(order[0], codex);
}

#[test]
fn sorting_by_a_select_with_no_options_still_works() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut scheme = crate::support::scheme();
    scheme
        .add_field(tm_core::scheme::Field::custom(
            "empty",
            "Empty",
            tm_core::scheme::FieldType::SingleSelect,
            Vec::new(),
        ))
        .unwrap();
    let mut store = open(&dir.db(), &clock).with_scheme(scheme);
    let only = issue(&mut store, &clock, &[], &[]);
    assert_eq!(find(&store, "sort:empty"), [only]);
}

#[test]
fn booleans_types_and_priority_presence_filter_as_written() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let on = issue(
        &mut store,
        &clock,
        &[("flag", json!(true)), ("priority", json!("high"))],
        &[],
    );
    let off = issue(&mut store, &clock, &[("flag", json!(false))], &[]);
    store
        .append(draft(off, set_field("type", json!("bug"))))
        .unwrap();
    assert_eq!(find(&store, "flag:false"), [off]);
    assert_eq!(find(&store, "flag:true"), [on]);
    assert_eq!(find(&store, "type:BUG"), [off]);
    assert_eq!(find(&store, "has:priority"), [on]);
    assert_eq!(find(&store, "no:priority"), [off]);
}

#[test]
fn dates_sort_newest_first() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let early = issue(&mut store, &clock, &[("due", json!("2026-01-01"))], &[]);
    let unset = issue(&mut store, &clock, &[], &[]);
    let late = issue(&mut store, &clock, &[("due", json!("2026-12-31"))], &[]);
    assert_eq!(find(&store, "sort:due"), [late, early, unset]);
    assert_eq!(find(&store, "sort:due-asc"), [early, late, unset]);
}

#[test]
fn every_status_is_triage_when_the_scheme_defines_no_types() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let typed = issue(&mut store, &clock, &[], &[]);
    let mut empty = crate::support::scheme();
    empty.types.clear();
    let store = store.with_scheme(empty);
    assert_eq!(find(&store, "category:triage"), [typed]);
}

#[test]
fn filter_errors_come_back_structured() {
    let dir = TempDir::new();
    let store = open(&dir.db(), &ManualClock::at(1_000));
    let err = store.find("stauts:todo", ME).unwrap_err();
    let StoreError::Filter(FilterError::UnknownField { valid, .. }) = err else {
        panic!("{err}");
    };
    assert!(valid.contains(&"status".to_owned()));
    let err = store.find("assignee:@me", Context::default()).unwrap_err();
    assert!(
        matches!(err, StoreError::Filter(FilterError::NoIdentity { .. })),
        "{err}"
    );
}
