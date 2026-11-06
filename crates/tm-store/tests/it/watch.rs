//! A Watch tells a reader when another process has written (ADR 0018).

use std::fs;
use std::time::{Duration, SystemTime};

use tm_core::Actor;
use tm_store::workspace::Workspace;
use tm_store::{Store, StoreError, Watch};
use uuid::Uuid;

use crate::support::TempDir;

#[test]
fn another_connections_write_is_seen_once() {
    let home = TempDir::new();
    let (workspace, mut store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    let jace = Actor::human("jace");
    let (project, _) = store.create_project(&jace, "TM", "Core").unwrap();
    let mut watch = Watch::store(&workspace.store_path()).unwrap();
    assert!(!watch.changed().unwrap());
    store.create_issue(&jace, project, "task", "t").unwrap();
    assert!(watch.changed().unwrap());
    assert!(!watch.changed().unwrap());
}

#[test]
fn a_store_replaced_by_another_file_is_a_change_and_one_removed_an_error() {
    let home = TempDir::new();
    let (workspace, mut store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    let jace = Actor::human("jace");
    let (project, _) = store.create_project(&jace, "TM", "Core").unwrap();
    let path = workspace.store_path();
    // Closed first, so its last close checkpoints the log into the file.
    drop(store);
    let mut watch = Watch::store(&path).unwrap();
    let copy = home.path().join("copy.db");
    fs::copy(&path, &copy).unwrap();
    fs::rename(&copy, &path).unwrap();
    // The old connection keeps the unlinked -shm, so only a fresh one sees
    // the writes below.
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", path.display()));
    }
    assert!(watch.changed().unwrap());
    let mut store = Store::open(&path).unwrap();
    for title in ["a", "b"] {
        store.create_issue(&jace, project, "task", title).unwrap();
        assert!(watch.changed().unwrap(), "watches the file now at the path");
    }
    drop(store);
    fs::remove_file(&path).unwrap();
    assert!(matches!(watch.changed(), Err(StoreError::StoreGone { .. })));
}

#[test]
fn a_snapshot_renamed_over_with_the_same_size_and_time_is_a_change() {
    let dir = TempDir::new();
    let snapshot = dir.path().join("snapshot.db");
    let next = dir.path().join("next.db");
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for path in [&snapshot, &next] {
        fs::write(path, "same").unwrap();
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }
    let mut watch = Watch::snapshot(&snapshot).unwrap();
    fs::rename(&next, &snapshot).unwrap();
    assert!(watch.changed().unwrap());
}

#[test]
fn a_publish_replacing_the_snapshot_is_seen_once() {
    let home = TempDir::new();
    let (workspace, mut store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    let jace = Actor::human("jace");
    let (project, _) = store.create_project(&jace, "TM", "Core").unwrap();
    let snapshot = workspace.snapshot_path();
    store.publish_snapshot(&snapshot).unwrap();
    let mut watch = Watch::snapshot(&snapshot).unwrap();
    assert!(!watch.changed().unwrap());
    store.create_issue(&jace, project, "task", "t").unwrap();
    store.publish_snapshot(&snapshot).unwrap();
    assert!(watch.changed().unwrap());
    assert!(!watch.changed().unwrap());
    drop(Store::open_snapshot(&snapshot, workspace.id).unwrap());
    assert!(
        !watch.changed().unwrap(),
        "a reader opening it is no change"
    );
}
