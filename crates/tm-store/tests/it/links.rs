//! Links: canonical keys, write-time checks and merge (ADR 0009).

use tm_core::{Change, Hlc};
use tm_store::{Store, StoreError};
use uuid::Uuid;

use crate::support::{
    ManualClock, OTHER, TempDir, draft, insert_acked, insert_acked_raw, links, new_issue, open,
    remote_op,
};

fn link_versions(store: &Store) -> i64 {
    store
        .connection()
        .query_row(
            "SELECT count(*) FROM field_versions WHERE field = 'links'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn link(link_type: &str, target: Uuid, present: bool) -> Change {
    Change::Link {
        link_type: link_type.into(),
        target,
        present,
    }
}

/// Two Issues ordered by id, so a symmetric Link's canonical source is `low`.
fn ordered_pair(store: &mut Store) -> (Uuid, Uuid) {
    let (a, b) = (new_issue(store), new_issue(store));
    (a.min(b), a.max(b))
}

#[test]
fn a_symmetric_link_written_either_way_round_is_one_link() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (low, high) = ordered_pair(&mut store);
    let op = store.link("jace", "relates_to", high, low).unwrap();
    assert_eq!(op.subject, low, "filed under the canonical source");
    store.link("jace", "relates_to", low, high).unwrap();
    assert_eq!(links(&store), [("relates_to".into(), low, high)]);
    assert_eq!(link_versions(&store), 1);
    store.unlink("jace", "relates_to", high, low).unwrap();
    assert_eq!(links(&store), Vec::new());
}

#[test]
fn the_type_is_part_of_a_links_key() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (a, b) = (new_issue(&mut store), new_issue(&mut store));
    store.link("jace", "blocks", a, b).unwrap();
    store.link("jace", "duplicates", a, b).unwrap();
    store.link("jace", "blocks", b, a).unwrap_err();
    assert_eq!(links(&store).len(), 2);
    assert_eq!(link_versions(&store), 2);
}

#[test]
fn the_same_link_from_two_replicas_is_one_element_and_a_later_remove_wins() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    let ops = [
        (7, a, Change::Create),
        (7, b, Change::Create),
        (7, a, link("blocks", b, true)),
        (8, a, link("blocks", b, true)),
    ];
    for (seq, (replica, issue, change)) in (1..).zip(ops) {
        let op = remote_op(replica, issue, &change, Hlc::new(1_000 + seq, 0), 0);
        insert_acked(&store, &op, i64::try_from(seq).unwrap(), 2_000);
    }
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(links(&store), [("blocks".into(), a, b)]);
    assert_eq!(link_versions(&store), 1);
    let remove = remote_op(8, a, &link("blocks", b, false), Hlc::new(1_010, 0), 4);
    insert_acked(&store, &remove, 5, 2_000);
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(links(&store), Vec::new());
}

#[test]
fn adding_a_held_link_or_removing_a_missing_one_is_logged() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (a, b) = (new_issue(&mut store), new_issue(&mut store));
    store.link("jace", "blocks", a, b).unwrap();
    store.link("jace", "blocks", a, b).unwrap();
    store.unlink("jace", "blocks", a, b).unwrap();
    store.unlink("jace", "blocks", a, b).unwrap();
    let logged: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM ops WHERE kind = 'link'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(logged, 4);
    assert_eq!(links(&store), Vec::new());
}

#[test]
fn a_link_must_have_a_known_type_and_join_two_existing_issues() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (a, b) = (new_issue(&mut store), new_issue(&mut store));
    let err = store.link("jace", "waits_for", a, b).unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid { valid, .. } if valid.contains(&"blocks".to_owned())),
        "{err}"
    );
    let err = store.link("jace", "blocks", a, a).unwrap_err();
    assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
    let missing = Uuid::now_v7();
    let err = store.link("jace", "blocks", a, missing).unwrap_err();
    assert!(
        matches!(err, StoreError::UnknownIssue(id) if id == missing),
        "{err}"
    );
    let err = store.link("jace", "blocks", missing, a).unwrap_err();
    assert!(
        matches!(err, StoreError::UnknownIssue(id) if id == missing),
        "{err}"
    );
    let err = store.unlink("jace", "blocks", a, missing).unwrap_err();
    assert!(
        matches!(err, StoreError::UnknownIssue(id) if id == missing),
        "{err}"
    );
    let err = store.link("jace", "relate", missing, a).unwrap_err();
    assert!(
        matches!(err, StoreError::Invalid { .. }),
        "a bad type is reported before a missing Issue: {err}"
    );
    assert_eq!(links(&store), Vec::new());
}

#[test]
fn a_symmetric_link_filed_under_the_wrong_issue_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (low, high) = ordered_pair(&mut store);
    let err = store
        .append(draft(high, link("relates_to", low, true)))
        .unwrap_err();
    assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
}

#[test]
fn a_blocking_link_that_closes_a_cycle_is_refused_with_its_path() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let [a, b, c] = [(); 3].map(|()| new_issue(&mut store));
    store.link("jace", "blocks", a, b).unwrap();
    store.link("jace", "blocks", b, c).unwrap();
    store.link("jace", "causes", a, c).unwrap();
    let err = store.link("jace", "blocks", c, a).unwrap_err();
    let StoreError::Cycle { relation, path } = err else {
        panic!("{err}");
    };
    assert_eq!(relation, "blocks");
    assert_eq!(path, [c, a, b, c]);
    store.link("jace", "causes", c, a).unwrap();
    store.link("jace", "relates_to", c, a).unwrap();
    store.link("jace", "blocks", a, c).unwrap();
    assert_eq!(links(&store).len(), 6);
}

#[test]
fn a_blocking_cycle_from_a_merge_is_kept_and_writes_still_finish() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    let ops = [
        (a, Change::Create),
        (b, Change::Create),
        (a, link("blocks", b, true)),
        (b, link("blocks", a, true)),
    ];
    for (seq, (issue, change)) in (1..).zip(ops) {
        let op = remote_op(7, issue, &change, Hlc::new(1_000 + seq, 0), 0);
        insert_acked(&store, &op, i64::try_from(seq).unwrap(), 2_000);
    }
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(links(&store).len(), 2);
    let c = new_issue(&mut store);
    store.link("jace", "blocks", c, a).unwrap();
    let err = store.link("jace", "blocks", a, c).unwrap_err();
    assert!(matches!(err, StoreError::Cycle { .. }), "{err}");
    store.unlink("jace", "blocks", b, a).unwrap();
}

#[test]
fn a_held_link_of_a_type_this_tm_lacks_can_be_removed() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    let ops = [
        (a, Change::Create),
        (b, Change::Create),
        (a, link("waits_for", b, true)),
    ];
    for (seq, (issue, change)) in (1..).zip(ops) {
        let op = remote_op(7, issue, &change, Hlc::new(5_000 + seq, 0), 0);
        insert_acked(&store, &op, i64::try_from(seq).unwrap(), 6_000);
    }
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(links(&store), [("waits_for".into(), a, b)]);
    let c = new_issue(&mut store);
    store.unlink("jace", "waits_for", a, c).unwrap_err();
    store.unlink("jace", "other_type", a, b).unwrap_err();
    store.unlink("jace", "waits_for", a, b).unwrap();
    assert_eq!(links(&store), Vec::new());
}

#[test]
fn replay_sets_aside_a_link_that_does_not_fit() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    let (low, high) = (a.min(b), a.max(b));
    let missing = Uuid::now_v7();
    let ops = [
        (a, Change::Create),
        (b, Change::Create),
        (high, link("relates_to", low, true)),
        (low, link("blocks", low, true)),
        (low, link("blocks", missing, true)),
        (low, link("blocks", high, true)),
    ];
    for (seq, (issue, change)) in (1..).zip(ops) {
        let op = remote_op(7, issue, &change, Hlc::new(5_000 + seq, 0), 0);
        insert_acked(&store, &op, i64::try_from(seq).unwrap(), 6_000);
    }
    assert_eq!(store.rebuild().unwrap(), 3);
    let reasons: Vec<String> = store
        .unapplied()
        .unwrap()
        .into_iter()
        .map(|unapplied| unapplied.reason)
        .collect();
    assert!(reasons[0].contains("canonical source"), "{reasons:?}");
    assert!(reasons[1].contains("itself"), "{reasons:?}");
    assert!(
        reasons[2].contains(&format!("{missing} it links to does not exist")),
        "{reasons:?}"
    );
    // Kept, as a target in a later chain would be.
    let mut expected = vec![
        ("blocks".to_owned(), low, missing),
        ("blocks".to_owned(), low, high),
    ];
    expected.sort();
    assert_eq!(links(&store), expected);
}

#[test]
fn a_cross_project_link_survives_its_targets_chain_replaying_later() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (here, there) = (Uuid::now_v7(), Uuid::now_v7());
    let mut there_create = remote_op(7, there, &Change::Create, Hlc::new(1_000, 0), 0);
    there_create.project = OTHER;
    insert_acked(
        &store,
        &remote_op(7, here, &Change::Create, Hlc::new(1_001, 0), 0),
        1,
        2_000,
    );
    insert_acked(
        &store,
        &remote_op(7, here, &link("blocks", there, true), Hlc::new(1_002, 0), 1),
        2,
        2_000,
    );
    insert_acked(&store, &there_create, 5, 2_000);
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(links(&store), [("blocks".into(), here, there)]);
}

#[test]
fn a_link_from_an_earlier_position_with_a_later_clock_beats_a_concurrent_add() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    let ops = [
        (7, a, Change::Create, 1_001, 0),
        (7, b, Change::Create, 1_002, 0),
        (7, a, link("blocks", b, true), 1_003, 2),
        (7, a, link("blocks", b, false), 1_020, 3),
        (8, a, link("blocks", b, true), 1_010, 3),
    ];
    for (seq, (replica, issue, change, ms, base)) in (1..).zip(ops) {
        let op = remote_op(replica, issue, &change, Hlc::new(ms, 0), base);
        insert_acked(&store, &op, seq, 2_000);
    }
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(links(&store), Vec::new());
}

#[test]
fn an_operation_of_another_kind_on_the_links_field_is_refused_like_any_malformed_one() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    let forged = Change::Remove {
        field: "links".into(),
        label: String::new(),
        elem: format!("blocks/{b}"),
        elem_label: None,
    };
    let ops = [
        (a, Change::Create, 1_001),
        (b, Change::Create, 1_002),
        (a, link("blocks", b, true), 1_003),
    ];
    for (seq, (issue, change, ms)) in (1..).zip(ops) {
        let op = remote_op(7, issue, &change, Hlc::new(ms, 0), 0);
        insert_acked(&store, &op, seq, 2_000);
    }
    let op = remote_op(7, a, &forged, Hlc::new(1_010, 0), 3);
    insert_acked_raw(
        &store,
        &op,
        forged.field(),
        forged.elem().as_deref(),
        4,
        2_000,
    );
    let err = store.rebuild().unwrap_err();
    assert!(
        matches!(&err, StoreError::Replay { seq, .. } if *seq == 6),
        "{err}"
    );
}

#[test]
fn a_link_across_projects_is_filed_in_the_source_project() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let here = new_issue(&mut store);
    let there = store.create_issue("jace", OTHER, "task", "t").unwrap().0;
    let op = store.link("jace", "blocks", there, here).unwrap();
    assert_eq!(op.project, OTHER);
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(links(&store), [("blocks".into(), there, here)]);
}
