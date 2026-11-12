//! A Watch tells a reader when another process has written (ADR 0018).

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
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
    let home = TempDir::new();
    let (workspace, store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    let snapshot = workspace.snapshot_path();
    store.publish_snapshot(&snapshot).unwrap();
    let next = home.path().join("next.db");
    fs::copy(&snapshot, &next).unwrap();
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for path in [&snapshot, &next] {
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
fn a_held_claim_lapsing_is_a_change_once_with_no_write() {
    let home = TempDir::new();
    let (workspace, mut store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    let jace = Actor::human("jace");
    let (project, _) = store.create_project(&jace, "TM", "Core").unwrap();
    let (issue, _) = store.create_issue(&jace, project, "task", "t").unwrap();
    let claude = Actor::agent("claude", "jace");
    let first = store.take(&claude, issue, Some(1_000)).unwrap();
    let (other, _) = store.create_issue(&jace, project, "task", "u").unwrap();
    store.take(&claude, other, None).unwrap();
    let snapshot = workspace.snapshot_path();
    store.publish_snapshot(&snapshot).unwrap();
    let mut watches = [
        Watch::store(&workspace.store_path()).unwrap(),
        Watch::snapshot(&snapshot).unwrap(),
    ];
    for watch in &mut watches {
        assert!(!watch.changed().unwrap(), "held");
    }
    let lapses = first.expires_ms.unwrap();
    while jiff::Timestamp::now().as_millisecond() < lapses {
        std::thread::sleep(Duration::from_millis(10));
    }
    for watch in &mut watches {
        assert!(watch.changed().unwrap(), "lapsed");
        assert!(
            !watch.changed().unwrap(),
            "seen once; a Claim with no expiry never lapses"
        );
    }
}

#[test]
fn a_claim_taken_after_the_watch_is_made_lapses_too() {
    let home = TempDir::new();
    let (workspace, mut store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    let jace = Actor::human("jace");
    let (project, _) = store.create_project(&jace, "TM", "Core").unwrap();
    let (issue, _) = store.create_issue(&jace, project, "task", "t").unwrap();
    let snapshot = workspace.snapshot_path();
    store.publish_snapshot(&snapshot).unwrap();
    let mut watches = [
        Watch::store(&workspace.store_path()).unwrap(),
        Watch::snapshot(&snapshot).unwrap(),
    ];
    let taken = store
        .take(&Actor::agent("claude", "jace"), issue, Some(1_000))
        .unwrap();
    store.publish_snapshot(&snapshot).unwrap();
    for watch in &mut watches {
        assert!(watch.changed().unwrap(), "the take");
        assert!(!watch.changed().unwrap(), "held");
    }
    while jiff::Timestamp::now().as_millisecond() < taken.expires_ms.unwrap() {
        std::thread::sleep(Duration::from_millis(10));
    }
    for watch in &mut watches {
        assert!(watch.changed().unwrap(), "lapsed");
    }
}

#[test]
fn a_write_whose_lapse_read_fails_is_reported_once_it_succeeds() {
    let home = TempDir::new();
    let (workspace, mut store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    let jace = Actor::human("jace");
    let snapshot = workspace.snapshot_path();
    store.publish_snapshot(&snapshot).unwrap();
    let mut watch = Watch::snapshot(&snapshot).unwrap();
    store.create_project(&jace, "TM", "Core").unwrap();
    store.publish_snapshot(&snapshot).unwrap();
    // Unreadable but the same file, so only the lapse read fails.
    let mode = |mode| fs::set_permissions(&snapshot, fs::Permissions::from_mode(mode)).unwrap();
    mode(0o000);
    let failed = watch.changed();
    mode(0o644);
    assert!(failed.is_err(), "{failed:?}");
    assert!(watch.changed().unwrap(), "the publish is not lost");
    assert!(!watch.changed().unwrap());
}

#[test]
fn a_snapshot_another_tm_published_is_refused_by_its_version() {
    let home = TempDir::new();
    let (workspace, store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    let snapshot = workspace.snapshot_path();
    store.publish_snapshot(&snapshot).unwrap();
    rusqlite::Connection::open(&snapshot)
        .unwrap()
        .execute_batch("PRAGMA user_version = 1")
        .unwrap();
    assert!(matches!(
        Watch::snapshot(&snapshot),
        Err(StoreError::SnapshotVersion { found: 1, .. })
    ));
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
