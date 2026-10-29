//! Filters run end to end: text in, matching Issues out, in order.

use serde_json::{Value, json};
use tm_core::filter::{Context, FilterError, Span};
use tm_core::{Change, Hlc, ProjectId};
use tm_store::{Place, Store, StoreError};
use uuid::Uuid;

use crate::support::PROJECT;
use crate::support::{ManualClock, TempDir, draft, insert_acked, jace, open, remote_op, set_field};

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
    let (issue, _) = store.create_issue(&jace(), PROJECT, "task", "t").unwrap();
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
fn parent_names_an_issue_by_key_or_id() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let epic = issue(&mut store, &clock, &[], &[]);
    let other = issue(&mut store, &clock, &[], &[]);
    let under = |above: Uuid| [("parent", json!(above.to_string()))];
    let child = issue(&mut store, &clock, &under(epic), &[]);
    let archived = issue(
        &mut store,
        &clock,
        &[under(epic)[0].clone(), ("archived", json!(true))],
        &[],
    );
    let cousin = issue(&mut store, &clock, &under(other), &[]);
    let key = store.issue(epic).unwrap().unwrap().key.unwrap();

    assert_eq!(find(&store, &format!("parent:{key}")), [child]);
    assert_eq!(
        find(&store, &format!("Parent:{}", key.to_lowercase())),
        [child]
    );
    assert_eq!(find(&store, &format!("parent:{epic}")), [child]);
    assert_eq!(
        sorted(find(&store, &format!("parent:{epic} archived:true,false"))),
        sorted(vec![child, archived])
    );
    assert_eq!(
        sorted(find(&store, &format!("-parent:{key}"))),
        sorted(vec![epic, other, cousin])
    );
    assert_eq!(
        sorted(find(&store, &format!("parent:{key},{other}"))),
        sorted(vec![child, cousin])
    );
    assert_eq!(
        sorted(find(&store, "has:parent")),
        sorted(vec![child, cousin])
    );
    let childless = store.issue(child).unwrap().unwrap().key.unwrap();
    for reference in [child.to_string(), childless] {
        assert_eq!(
            find(&store, &format!("parent:{reference}")),
            Vec::<Uuid>::new()
        );
    }
}

#[test]
fn parent_finds_the_children_of_a_parent_missing_from_the_log() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let orphan = issue(&mut store, &clock, &[], &[]);
    let gone = Uuid::now_v7();
    store
        .connection()
        .execute(
            "UPDATE issues SET parent_id = ?1 WHERE id = ?2",
            [gone.as_bytes(), orphan.as_bytes()],
        )
        .unwrap();
    assert_eq!(find(&store, &format!("parent:{gone}")), [orphan]);
}

#[test]
fn blocked_by_finds_the_issues_a_blocker_missing_from_the_log_blocks() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let held = issue(&mut store, &clock, &[], &[]);
    let gone = Uuid::now_v7();
    store
        .connection()
        .execute(
            "INSERT INTO links (type, src, dst) VALUES ('blocks', ?1, ?2)",
            [gone.as_bytes(), held.as_bytes()],
        )
        .unwrap();
    assert_eq!(find(&store, &format!("blocked_by:{gone}")), [held]);
    let err = store
        .find(&format!("parent:{gone}"), ME)
        .unwrap_err()
        .to_string();
    assert_eq!(err, format!("parent: no Issue is {gone}"));
}

#[test]
fn a_parent_that_does_not_exist_is_named() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (first, _) = store.create_issue(&jace(), PROJECT, "task", "t").unwrap();
    let key = store.issue(first).unwrap().unwrap().key.unwrap();
    let missing = Uuid::now_v7().to_string();
    let named = |filter: &str| match store.find(filter, ME).unwrap_err() {
        StoreError::Filter(FilterError::NoIssue { value, span, .. }) => (value, span),
        err => panic!("{filter}: {err}"),
    };
    assert_eq!(named("parent:CPL-9"), ("CPL-9".into(), Span(7, 12)));
    assert_eq!(named("parent:CPL-8,CPL-9"), ("CPL-8".into(), Span(7, 12)));
    assert_eq!(
        named(&format!("status:todo parent:{missing}")),
        (missing, Span(19, 55))
    );
    let after = 7 + key.len() + 1;
    assert_eq!(
        named(&format!("parent:{key},CPL-99")),
        ("CPL-99".into(), Span(after, after + 6))
    );
    let err = store.find("stauts:todo parent:CPL-9", ME).unwrap_err();
    assert!(
        matches!(err, StoreError::Filter(FilterError::UnknownField { .. })),
        "a mistyped field is named before a missing parent: {err}"
    );
    let err = store.find("parent:nope", ME).unwrap_err();
    assert!(
        matches!(err, StoreError::Filter(FilterError::InvalidValue { .. })),
        "{err}"
    );
    let field = |filter: &str| match store.find(filter, ME).unwrap_err() {
        StoreError::Filter(FilterError::NoIssue { field, .. }) => field,
        err => panic!("{filter}: {err}"),
    };
    assert_eq!(named("blocked-by:CPL-9"), ("CPL-9".into(), Span(11, 16)));
    assert_eq!(field("blocked-by:CPL-9"), "blocked_by");
    assert_eq!(named("BLOCKING:CPL-9"), ("CPL-9".into(), Span(9, 14)));
    assert_eq!(field("BLOCKING:CPL-9"), "blocking");
    assert_eq!(field("Parent:CPL-9"), "parent");
}

#[test]
fn is_counts_blocks_links_between_open_issues_as_the_ready_queue_does() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let status = |status: &str| [("status", json!(status))];
    let holder = issue(&mut store, &clock, &[], &[]);
    let held = issue(&mut store, &clock, &[], &[]);
    let finished = issue(&mut store, &clock, &status("done"), &[]);
    let done = issue(&mut store, &clock, &status("done"), &[]);
    let after_done = issue(&mut store, &clock, &[], &[]);
    let archived = issue(&mut store, &clock, &[("archived", json!(true))], &[]);
    let after_archived = issue(&mut store, &clock, &[], &[]);
    let free = issue(&mut store, &clock, &[], &[]);
    for (from, to) in [
        (holder, held),
        (holder, finished),
        (done, after_done),
        (archived, after_archived),
    ] {
        store.link(&jace(), "blocks", from, to).unwrap();
    }
    let unarchived = [
        holder,
        held,
        finished,
        done,
        after_done,
        after_archived,
        free,
    ];
    let except = |left: &[Uuid]| {
        sorted(
            unarchived
                .iter()
                .filter(|id| !left.contains(id))
                .copied()
                .collect(),
        )
    };
    assert_eq!(find(&store, "is:blocked"), [held]);
    assert_eq!(find(&store, "IS:Blocking"), [holder]);
    assert_eq!(
        sorted(find(&store, "is:blocked,blocking")),
        sorted(vec![holder, held])
    );
    assert_eq!(sorted(find(&store, "-is:blocked")), except(&[held]));
    assert_eq!(
        sorted(find(&store, "-is:blocked -is:blocking")),
        except(&[held, holder])
    );
    assert_eq!(
        store.open_blockers(&unarchived).unwrap(),
        [(held, vec![holder])].into_iter().collect()
    );
}

#[test]
fn blocked_by_and_blocking_follow_links_whatever_the_statuses() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let done = issue(&mut store, &clock, &[("status", json!("done"))], &[]);
    let open_one = issue(&mut store, &clock, &[], &[]);
    let held = issue(&mut store, &clock, &[], &[]);
    let free = issue(&mut store, &clock, &[], &[]);
    store.link(&jace(), "blocks", done, held).unwrap();
    store.link(&jace(), "blocks", open_one, held).unwrap();
    let key = |id: Uuid| store.issue(id).unwrap().unwrap().key.unwrap();
    assert_eq!(find(&store, &format!("blocked-by:{}", key(done))), [held]);
    assert_eq!(find(&store, &format!("blocked_by:{done}")), [held]);
    assert_eq!(
        sorted(find(&store, &format!("blocking:{}", key(held)))),
        sorted(vec![done, open_one])
    );
    assert_eq!(
        sorted(find(&store, &format!("-blocked-by:{}", key(open_one)))),
        sorted(vec![done, open_one, free])
    );
    assert_eq!(find(&store, "has:blocked_by"), [held]);
    assert_eq!(
        sorted(find(&store, "no:blocking")),
        sorted(vec![held, free])
    );
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

fn on_list(store: &mut Store, project: ProjectId, name: &str, issues: &[Uuid]) -> Uuid {
    let actors = ["codex".to_owned()];
    let (list, _) = store
        .create_shortlist(&jace(), project, name, &actors, None)
        .unwrap();
    store
        .shortlist_add(&jace(), list, issues, Place::End)
        .unwrap();
    list
}

/// ADR 0016: `shortlist:NAME` selects a list's Issues by the read rule, so
/// a membership in a deleted list reads as none.
#[test]
fn a_shortlist_term_selects_its_members_by_name_or_id() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let [a, b, c, d] = [(); 4].map(|()| issue(&mut store, &clock, &[], &[]));
    let docs = on_list(&mut store, PROJECT, "docs", &[a, b]);
    let ops = on_list(&mut store, PROJECT, "ops", &[c]);

    assert_eq!(sorted(find(&store, "shortlist:DOCS")), sorted(vec![a, b]));
    assert_eq!(find(&store, &format!("shortlist:{docs}")).len(), 2);
    let either = find(&store, "shortlist:docs,ops");
    assert_eq!(sorted(either), sorted(vec![a, b, c]));
    assert_eq!(sorted(find(&store, "-shortlist:docs")), sorted(vec![c, d]));
    assert_eq!(sorted(find(&store, "has:shortlist")), sorted(vec![a, b, c]));
    assert_eq!(find(&store, "no:shortlist"), [d]);

    store.delete_shortlist(&jace(), ops).unwrap();
    assert_eq!(sorted(find(&store, "has:shortlist")), sorted(vec![a, b]));
    assert_eq!(sorted(find(&store, "no:shortlist")), sorted(vec![c, d]));
    assert_eq!(sorted(find(&store, "-shortlist:docs")), sorted(vec![c, d]));
}

#[test]
fn a_shortlist_name_matches_its_lists_in_every_project() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (other, _) = store.create_project(&jace(), "WEB", "Web").unwrap();
    let (here, _) = store.create_issue(&jace(), PROJECT, "task", "t").unwrap();
    let (there, _) = store.create_issue(&jace(), other, "task", "t").unwrap();
    on_list(&mut store, PROJECT, "docs", &[here]);
    on_list(&mut store, other, "Docs", &[there]);
    let both = find(&store, "shortlist:docs");
    assert_eq!(sorted(both), sorted(vec![here, there]));
}

#[test]
fn a_shortlist_that_does_not_exist_is_named() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (issue, _) = store.create_issue(&jace(), PROJECT, "task", "t").unwrap();
    let gone = on_list(&mut store, PROJECT, "gone", &[issue]);
    store.delete_shortlist(&jace(), gone).unwrap();
    let named = |filter: &str| match store.find(filter, ME).unwrap_err() {
        StoreError::Filter(FilterError::NoShortlist { value, span }) => (value, span),
        err => panic!("{filter}: {err}"),
    };
    let nope = named("status:todo shortlist:nope");
    assert_eq!(nope, ("nope".into(), Span(22, 26)));
    assert_eq!(named("shortlist:gone"), ("gone".into(), Span(10, 14)));
    // The missing name earlier in the text is the one named.
    let first = named("shortlist:nope parent:CPL-9");
    assert_eq!(first, ("nope".into(), Span(10, 14)));
    for filter in ["shortlist:@me", "shortlist:\" \""] {
        let err = store.find(filter, ME).unwrap_err();
        assert!(
            matches!(err, StoreError::Filter(FilterError::InvalidValue { .. })),
            "{filter}: {err}"
        );
    }
}

/// An id names its list even when another list took that id as its name.
#[test]
fn a_shortlist_id_wins_over_a_name_that_spells_it() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (a, _) = store.create_issue(&jace(), PROJECT, "task", "t").unwrap();
    let (b, _) = store.create_issue(&jace(), PROJECT, "task", "t").unwrap();
    let docs = on_list(&mut store, PROJECT, "docs", &[a]);
    on_list(&mut store, PROJECT, &docs.to_string(), &[b]);
    assert_eq!(find(&store, &format!("shortlist:{docs}")), [a]);
}

/// ADR 0016: a name a merge left on two lists in one Project is refused
/// until one is renamed.
#[test]
fn a_shortlist_name_two_lists_share_is_refused() {
    let dir = TempDir::new();
    let store = open(&dir.db(), &ManualClock::at(1_000));
    let name = Change::SetShortlist {
        field: tm_core::shortlist::Field::Name,
        value: json!("docs"),
    };
    for (n, list) in (0..).zip([Uuid::now_v7(), Uuid::now_v7()]) {
        let created = remote_op(8, list, &Change::CreateShortlist, Hlc::new(1_000, 2 * n), 0);
        let named = remote_op(8, list, &name, Hlc::new(1_000, 2 * n + 1), 0);
        insert_acked(&store, &created, i64::from(2 * n) + 3, 1_000);
        insert_acked(&store, &named, i64::from(2 * n) + 4, 1_000);
    }
    let mut store = store;
    store.rebuild().unwrap();
    let shared = |filter: &str| match store.find(filter, ME).unwrap_err() {
        StoreError::Filter(FilterError::ShortlistNameShared { span, count, .. }) => (span, count),
        err => panic!("{filter}: {err}"),
    };
    assert_eq!(shared("shortlist:DOCS"), (Span(10, 14), 2));
    assert_eq!(shared("shortlist:docs parent:CPL-9"), (Span(10, 14), 2));
    // A filter that does not fit the Scheme, or a missing parent earlier in
    // the text, is named first.
    let err = store.find("shortlist:docs nosuch:x", ME).unwrap_err();
    let unknown = matches!(err, StoreError::Filter(FilterError::UnknownField { .. }));
    assert!(unknown, "{err}");
    let err = store.find("parent:CPL-9 shortlist:docs", ME).unwrap_err();
    let parent = matches!(err, StoreError::Filter(FilterError::NoIssue { .. }));
    assert!(parent, "{err}");
}
