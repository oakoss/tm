//! People's names (ADR 0019): one record per email, written by the person
//! it names, the latest write winning.

use tm_core::{Actor, Change, Hlc, Op, ProjectId, person};
use tm_store::{Draft, Provenance, StoreError, WallTime};

use crate::support::{ManualClock, OTHER, PROJECT, TempDir, claude, insert_acked, open, remote_op};

#[test]
fn a_name_is_written_once_until_it_changes_and_survives_a_rebuild() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let jace = Actor::human("jace@example.com");
    assert_eq!(store.name_of("jace@example.com").unwrap(), None);
    assert!(store.record_name(&jace, PROJECT, "Jace").unwrap().is_some());
    assert!(
        store
            .record_name(&jace, PROJECT, " Jace ")
            .unwrap()
            .is_none(),
        "the same name, trimmed, writes nothing"
    );
    assert!(
        store
            .record_name(&jace, PROJECT, "Jace Babin")
            .unwrap()
            .is_some()
    );
    store.rebuild().unwrap();
    assert_eq!(
        store.name_of("JACE@example.com ").unwrap().as_deref(),
        Some("Jace Babin")
    );
}

#[test]
fn only_a_human_names_themselves() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let refused = store.record_name(&claude(), PROJECT, "Claude");
    assert!(
        matches!(refused, Err(StoreError::Invalid { ref reason, .. }) if reason.contains("person it names")),
        "{refused:?}"
    );
    let refused = store.record_name(&Actor::human("jace@example.com"), PROJECT, "Jace\nBabin");
    assert!(matches!(refused, Err(StoreError::Op(_))), "{refused:?}");
}

#[test]
fn the_latest_name_wins_whatever_order_replicas_bring_them() {
    let dir = TempDir::new();
    let store = open(&dir.db(), &ManualClock::at(1_000));
    let id = person::id("codex");
    let name = |name: &str| Change::SetName { name: name.into() };
    let later = remote_op(7, id, &name("Codex Later"), Hlc::new(9, 0), 0);
    let earlier = remote_op(8, id, &name("Codex Earlier"), Hlc::new(5, 0), 0);
    insert_acked(&store, &later, 10, 1);
    insert_acked(&store, &earlier, 11, 1);
    let mut store = store;
    store.rebuild().unwrap();
    assert_eq!(
        store.name_of("codex").unwrap().as_deref(),
        Some("Codex Later")
    );
}

#[test]
fn across_projects_the_clock_decides_not_either_projects_position() {
    let dir = TempDir::new();
    let store = open(&dir.db(), &ManualClock::at(1_000));
    let id = person::id("codex");
    let name = |name: &str| Change::SetName { name: name.into() };
    // Far along its Project's log, as if it had seen everything in OTHER too.
    let older = remote_op(7, id, &name("Codex Older"), Hlc::new(5, 0), 90);
    let newer = Op {
        project: OTHER,
        ..remote_op(8, id, &name("Codex Newer"), Hlc::new(9, 0), 1)
    };
    insert_acked(&store, &newer, 12, 1);
    insert_acked(&store, &older, 100, 1);
    let mut store = store;
    store.rebuild().unwrap();
    assert_eq!(
        store.name_of("codex").unwrap().as_deref(),
        Some("Codex Newer")
    );
}

#[test]
fn a_name_in_a_project_that_does_not_exist_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let jace = Actor::human("jace@example.com");
    let refused = store.record_name(&jace, ProjectId::new(), "Jace");
    assert!(
        matches!(refused, Err(StoreError::UnknownProject(_))),
        "{refused:?}"
    );
}

#[test]
fn an_import_brings_in_no_name() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let jace = Actor::human("jace@example.com");
    let named = Draft {
        actor: jace.clone(),
        project: PROJECT,
        subject: person::id(jace.name()),
        change: Change::SetName {
            name: "Jace".into(),
        },
    };
    let provenance = Provenance {
        source: "test".into(),
        at: WallTime {
            utc_ms: 500,
            offset_min: 0,
        },
    };
    let refused = store.append_imported(vec![(named, provenance)]);
    assert!(
        matches!(refused, Err(StoreError::Invalid { ref reason, .. }) if reason.contains("cannot be imported")),
        "{refused:?}"
    );
    assert_eq!(store.name_of("jace@example.com").unwrap(), None);
}
