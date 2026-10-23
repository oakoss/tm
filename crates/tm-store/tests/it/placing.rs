//! Putting Issues on a Short-list, moving them and taking them off (ADR 0016).

use serde_json::json;
use tm_core::Change;
use tm_store::{Place, Store, StoreError};
use uuid::Uuid;

use crate::support::{ManualClock, PROJECT, TempDir, draft, jace, new_issue, open, set_field};

fn list(store: &mut Store, name: &str, size: Option<u64>) -> Uuid {
    store
        .create_shortlist(&jace(), PROJECT, name, &["codex".to_owned()], size)
        .unwrap()
        .0
}

fn issues(store: &mut Store, n: usize) -> Vec<Uuid> {
    (0..n).map(|_| new_issue(store)).collect()
}

fn add(
    store: &mut Store,
    list: Uuid,
    ids: &[Uuid],
    at: Place,
) -> Result<Vec<tm_core::Op>, StoreError> {
    store
        .shortlist_add(&jace(), list, ids, at)
        .map(|placed| placed.ops)
}

fn members(store: &Store, list: Uuid) -> Vec<Uuid> {
    store.shortlist_members(list).unwrap()
}

fn refused(err: &StoreError, needle: &str) -> bool {
    matches!(err, StoreError::CannotJoin { reason, .. } if reason.contains(needle))
}

#[test]
fn issues_go_where_they_are_placed() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs", None);
    let ids = issues(&mut store, 5);
    let [one, two, three, four, five] = ids[..] else {
        unreachable!()
    };
    add(&mut store, docs, &[one, two], Place::End).unwrap();
    add(&mut store, docs, &[three], Place::Top).unwrap();
    add(&mut store, docs, &[four], Place::Before(two)).unwrap();
    add(&mut store, docs, &[five], Place::After(three)).unwrap();
    assert_eq!(members(&store, docs), [three, five, one, four, two]);
    store.shortlist_move(&jace(), three, Place::End).unwrap();
    store.shortlist_move(&jace(), two, Place::Top).unwrap();
    assert_eq!(members(&store, docs), [two, five, one, four, three]);
}

#[test]
fn a_command_is_one_batch_and_one_undo() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs", None);
    let ids = issues(&mut store, 3);
    let ops = add(&mut store, docs, &ids, Place::End).unwrap();
    assert!(ops.len() >= 6);
    store.undo(&jace(), ops[0].op_id).unwrap();
    assert_eq!(members(&store, docs), Vec::<Uuid>::new());
    add(&mut store, docs, &ids, Place::End).unwrap();
    let removed = store.shortlist_remove(&jace(), &ids[..2]).unwrap();
    assert_eq!(removed.from, [(ids[0], docs), (ids[1], docs)]);
    assert_eq!(members(&store, docs), [ids[2]]);
    store.undo(&jace(), removed.ops[0].op_id).unwrap();
    assert_eq!(members(&store, docs), ids);
}

#[test]
fn adding_an_issue_on_another_list_moves_it_there() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs", None);
    let ops = list(&mut store, "ops", None);
    let [a, b] = issues(&mut store, 2)[..] else {
        unreachable!()
    };
    add(&mut store, docs, &[a, b], Place::End).unwrap();
    add(&mut store, ops, &[a], Place::End).unwrap();
    assert_eq!(members(&store, docs), [b]);
    assert_eq!(members(&store, ops), [a]);
    // Already on this list, it moves rather than joining twice.
    add(&mut store, ops, &[b], Place::End).unwrap();
    add(&mut store, ops, &[b], Place::Top).unwrap();
    assert_eq!(members(&store, ops), [b, a]);
}

#[test]
fn an_add_refuses_epics_triage_archived_and_finished_issues() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs", None);
    let epic = store.create_issue(&jace(), PROJECT, "epic", "e").unwrap().0;
    let status = |status: &str| Some(set_field("status", json!(status)));
    let cases = [
        (None, "epic"),
        (status("triage"), "triage"),
        (Some(set_field("archived", json!(true))), "archived"),
        (status("done"), "done or cancelled"),
        (status("cancelled"), "done or cancelled"),
    ];
    for (change, reason) in cases {
        let issue = match change {
            None => epic,
            Some(change) => {
                let issue = new_issue(&mut store);
                store.append(draft(issue, change)).unwrap();
                issue
            }
        };
        let err = add(&mut store, docs, &[issue], Place::End).unwrap_err();
        assert!(refused(&err, reason), "{reason}: {err}");
    }
    assert_eq!(members(&store, docs), Vec::<Uuid>::new());
}

#[test]
fn an_add_past_the_size_is_refused_but_a_move_within_a_full_list_is_not() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs", Some(2));
    let [a, b, c] = issues(&mut store, 3)[..] else {
        unreachable!()
    };
    add(&mut store, docs, &[a, b], Place::End).unwrap();
    let err = add(&mut store, docs, &[c], Place::End).unwrap_err();
    assert!(
        matches!(&err, StoreError::ShortlistFull { name, size: 2 } if name == "docs"),
        "{err}"
    );
    add(&mut store, docs, &[b], Place::Top).unwrap();
    store.shortlist_move(&jace(), a, Place::Top).unwrap();
    assert_eq!(members(&store, docs), [a, b]);
    // Size is checked only when an Issue joins: a list left over its size
    // still reorders.
    let shrink = tm_store::ShortlistEdit {
        size: Some(1),
        ..tm_store::ShortlistEdit::default()
    };
    store.edit_shortlist(&jace(), docs, &shrink).unwrap();
    add(&mut store, docs, &[b], Place::Top).unwrap();
    assert_eq!(members(&store, docs), [b, a]);
}

#[test]
fn blockers_come_first_on_a_list() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs", None);
    let [first, waiting, other] = issues(&mut store, 3)[..] else {
        unreachable!()
    };
    store.link(&jace(), "blocks", first, waiting).unwrap();

    let err = add(&mut store, docs, &[waiting], Place::End).unwrap_err();
    assert!(refused(&err, "is not on the list"), "{err}");
    add(&mut store, docs, &[first, other], Place::End).unwrap();
    let err = add(&mut store, docs, &[waiting], Place::Top).unwrap_err();
    assert!(refused(&err, "before its blocker"), "{err}");
    add(&mut store, docs, &[waiting], Place::End).unwrap();
    assert_eq!(members(&store, docs), [first, other, waiting]);

    let err = store
        .shortlist_move(&jace(), first, Place::End)
        .unwrap_err();
    assert!(refused(&err, "which it blocks"), "{err}");
    store.shortlist_move(&jace(), other, Place::End).unwrap();
    assert_eq!(members(&store, docs), [first, waiting, other]);
}

/// A member that becomes blocked by an Issue off the list stays, and still
/// moves; only an add refuses an off-list blocker.
#[test]
fn a_member_blocked_later_stays_and_moves() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs", None);
    let [a, b, outside] = issues(&mut store, 3)[..] else {
        unreachable!()
    };
    add(&mut store, docs, &[a, b], Place::End).unwrap();
    store.link(&jace(), "blocks", outside, a).unwrap();
    store.shortlist_move(&jace(), a, Place::End).unwrap();
    assert_eq!(members(&store, docs), [b, a]);
    // Added again, a member moves as it would by tm shortlist move.
    add(&mut store, docs, &[a], Place::Top).unwrap();
    assert_eq!(members(&store, docs), [a, b]);
}

/// Order follows the blocker while it is open, whatever the member it
/// blocks has become.
#[test]
fn an_open_blocker_stays_ahead_of_an_archived_member_it_blocks() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs", None);
    let [first, waiting] = issues(&mut store, 2)[..] else {
        unreachable!()
    };
    store.link(&jace(), "blocks", first, waiting).unwrap();
    add(&mut store, docs, &[first, waiting], Place::End).unwrap();
    store
        .append(draft(waiting, set_field("archived", json!(true))))
        .unwrap();
    let err = store
        .shortlist_move(&jace(), first, Place::End)
        .unwrap_err();
    assert!(refused(&err, "which it blocks"), "{err}");
}

#[test]
fn a_place_between_tied_ranks_respaces_the_list() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs", None);
    let [a, b, c] = issues(&mut store, 3)[..] else {
        unreachable!()
    };
    // Two Replicas inserting at once can leave equal ranks.
    let join = |issue| {
        vec![
            draft(issue, set_field("shortlist", json!(docs.to_string()))),
            draft(issue, set_field("shortlist_rank", json!("V"))),
        ]
    };
    store.append_all(join(a)).unwrap();
    store.append_all(join(b)).unwrap();
    let tied = members(&store, docs);
    let ops = add(&mut store, docs, &[c], Place::Before(tied[1])).unwrap();
    assert_eq!(members(&store, docs), [tied[0], c, tied[1]]);
    let ranks: Vec<String> = ops
        .iter()
        .filter_map(|op| match op.change().unwrap() {
            Change::SetField { field, value, .. } if field == "shortlist_rank" => {
                value.as_str().map(str::to_owned)
            }
            _ => None,
        })
        .collect();
    assert_eq!(ranks.len(), 3, "every member gets a fresh rank");
}

#[test]
fn a_place_beside_an_issue_off_the_list_or_itself_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs", None);
    let [a, b, outside] = issues(&mut store, 3)[..] else {
        unreachable!()
    };
    add(&mut store, docs, &[a], Place::End).unwrap();
    let elsewhere = list(&mut store, "ops", None);
    add(&mut store, elsewhere, &[outside], Place::End).unwrap();
    // Named as not on this list, not as on none, which it is not.
    let err = add(&mut store, docs, &[b], Place::After(outside)).unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid { field, reason, .. }
            if field == "place" && reason.contains("not on docs")),
        "{err}"
    );
    assert_eq!(members(&store, elsewhere), [outside]);
    let unknown = Uuid::now_v7();
    let err = add(&mut store, docs, &[b], Place::Before(unknown)).unwrap_err();
    assert!(
        matches!(err, StoreError::UnknownIssue(id) if id == unknown),
        "{err}"
    );
    let err = add(&mut store, docs, &[b], Place::After(b)).unwrap_err();
    assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
    let err = store.shortlist_move(&jace(), b, Place::Top).unwrap_err();
    assert!(
        matches!(&err, StoreError::NotOnShortlist(named) if named.key.is_some()),
        "named by Key: {err}"
    );
    let err = store.shortlist_remove(&jace(), &[b]).unwrap_err();
    assert!(matches!(err, StoreError::NotOnShortlist(_)), "{err}");
}

#[test]
fn a_list_is_found_by_name_without_case() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "Docs", None);
    assert_eq!(store.shortlist_named(PROJECT, "DOCS").unwrap().id, docs);
    let err = store.shortlist_named(PROJECT, "ops").unwrap_err();
    assert!(matches!(err, StoreError::ShortlistNotFound(_)), "{err}");
}
