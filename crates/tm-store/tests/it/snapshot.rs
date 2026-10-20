use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tm_core::Actor;
use tm_core::filter::Context;
use tm_store::workspace::Workspace;
use tm_store::{Store, StoreError};
use uuid::Uuid;

use crate::support::TempDir;

fn titles(store: &Store) -> Vec<String> {
    store
        .connection()
        .prepare("SELECT title FROM issues ORDER BY title")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn a_snapshot_holds_the_store_as_of_its_publish() {
    let home = TempDir::new();
    let (workspace, mut store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    let jace = Actor::human("jace");
    let (project, _) = store.create_project(&jace, "TM", "Core").unwrap();
    store.create_issue(&jace, project, "task", "first").unwrap();
    store.publish_snapshot(&workspace.snapshot_path()).unwrap();
    store
        .create_issue(&jace, project, "task", "second")
        .unwrap();

    let snapshot = Store::open_snapshot(&workspace.snapshot_path(), workspace.id).unwrap();
    assert_eq!(titles(&snapshot), ["first"]);
    let found = snapshot.find("", Context { me: None }).unwrap();
    assert_eq!(found.len(), 1, "filters run on the Snapshot");
    drop(snapshot);

    store.publish_snapshot(&workspace.snapshot_path()).unwrap();
    let snapshot = Store::open_snapshot(&workspace.snapshot_path(), workspace.id).unwrap();
    assert_eq!(titles(&snapshot), ["first", "second"]);
}

#[test]
fn a_snapshot_is_published_and_opened_under_a_path_a_uri_would_misread() {
    let dir = TempDir::new();
    let home = PathBuf::from(format!("/{}", dir.path().join("a b?c#d%e").display()));
    let (workspace, store) = Workspace::create(&home, Uuid::now_v7()).unwrap();
    store.publish_snapshot(&workspace.snapshot_path()).unwrap();
    let entries: Vec<_> = fs::read_dir(&workspace.dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        entries.iter().any(|name| name == "snapshot.db"),
        "{entries:?}"
    );
    Store::open_snapshot(&workspace.snapshot_path(), workspace.id).unwrap();
}

/// macOS refuses such names, so only Linux runs this.
#[cfg(target_os = "linux")]
#[test]
fn a_snapshot_is_published_under_a_path_that_is_not_utf8() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    let dir = TempDir::new();
    let home = dir.path().join(OsStr::from_bytes(b"not-\xff-utf8"));
    let (workspace, store) = Workspace::create(&home, Uuid::now_v7()).unwrap();
    store.publish_snapshot(&workspace.snapshot_path()).unwrap();
    Store::open_snapshot(&workspace.snapshot_path(), workspace.id).unwrap();
}

#[test]
fn a_snapshot_opens_where_nothing_can_be_written_and_takes_no_writes() {
    let home = TempDir::new();
    let (workspace, mut store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    let jace = Actor::human("jace");
    let (project, _) = store.create_project(&jace, "TM", "Core").unwrap();
    store.publish_snapshot(&workspace.snapshot_path()).unwrap();
    drop(store);
    let before: Vec<_> = fs::read_dir(&workspace.dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();

    let mode = |path: &std::path::Path, mode| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    };
    mode(&workspace.snapshot_path(), 0o444);
    mode(&workspace.dir, 0o555);
    // Results are checked once the directory is writable again, so a failure
    // leaves nothing the temporary directory cannot remove.
    let read = Store::open_snapshot(&workspace.snapshot_path(), workspace.id).map(|mut store| {
        let key = store.project_key(project).unwrap();
        (key, store.create_issue(&jace, project, "task", "no"))
    });
    mode(&workspace.dir, 0o755);

    let (key, refused) = read.unwrap();
    assert_eq!(key.as_deref(), Some("TM"));
    assert!(
        matches!(&refused, Err(StoreError::Sqlite(err)) if err.sqlite_error_code() == Some(rusqlite::ErrorCode::ReadOnly)),
        "{refused:?}"
    );
    let after: Vec<_> = fs::read_dir(&workspace.dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(before, after, "opening the Snapshot made no file");
}

#[test]
fn a_publish_waits_for_one_in_progress() {
    let home = TempDir::new();
    let (workspace, store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    let held = fs::File::create(workspace.dir.join("snapshot.lock")).unwrap();
    held.lock().unwrap();
    let released = Arc::new(AtomicBool::new(false));
    let release = std::thread::spawn({
        let released = Arc::clone(&released);
        move || {
            std::thread::sleep(Duration::from_millis(100));
            released.store(true, Ordering::SeqCst);
            drop(held);
        }
    });
    store.publish_snapshot(&workspace.snapshot_path()).unwrap();
    assert!(released.load(Ordering::SeqCst), "published while locked");
    release.join().unwrap();
}

#[test]
fn a_failed_publish_leaves_no_copy() {
    let home = TempDir::new();
    let (workspace, store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    fs::create_dir_all(workspace.snapshot_path().join("in-the-way")).unwrap();
    let failed = store.publish_snapshot(&workspace.snapshot_path());
    assert!(
        matches!(&failed, Err(StoreError::Snapshot { path, .. }) if *path == workspace.snapshot_path()),
        "{failed:?}"
    );
    let left: Vec<_> = fs::read_dir(&workspace.dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().starts_with(".snapshot-"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn a_snapshot_reads_past_a_journal_and_sorts_in_memory() {
    let home = TempDir::new();
    let (workspace, store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    store.publish_snapshot(&workspace.snapshot_path()).unwrap();
    // A journal beside a file opened without `immutable=1` reads as a hot one
    // to roll back, which a reader that cannot write fails on.
    let journal = workspace.dir.join("snapshot.db-journal");
    fs::write(&journal, [0xd9; 512]).unwrap();
    fs::set_permissions(&workspace.dir, fs::Permissions::from_mode(0o555)).unwrap();
    let opened = Store::open_snapshot(&workspace.snapshot_path(), workspace.id).map(|store| {
        store
            .connection()
            .query_row("PRAGMA temp_store", [], |row| row.get::<_, i64>(0))
            .unwrap()
    });
    fs::set_permissions(&workspace.dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(opened.unwrap(), 2, "temp_store is MEMORY");
}

#[test]
fn a_publish_removes_copies_an_interrupted_one_left() {
    let home = TempDir::new();
    let (workspace, store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    let left = workspace.dir.join(".snapshot-0.db");
    fs::write(&left, "partial").unwrap();
    store.publish_snapshot(&workspace.snapshot_path()).unwrap();
    assert!(!left.exists());
    assert!(workspace.snapshot_path().exists());
}

#[test]
fn a_snapshot_of_another_workspace_or_build_is_refused() {
    let home = TempDir::new();
    let (workspace, store) = Workspace::create(home.path(), Uuid::now_v7()).unwrap();
    store.publish_snapshot(&workspace.snapshot_path()).unwrap();
    drop(store);

    let other = Uuid::now_v7();
    assert!(matches!(
        Store::open_snapshot(&workspace.snapshot_path(), other),
        Err(StoreError::WorkspaceMismatch { stored, given }) if stored == workspace.id && given == other
    ));

    rusqlite::Connection::open(workspace.snapshot_path())
        .unwrap()
        .execute_batch("PRAGMA user_version = 1")
        .unwrap();
    assert!(matches!(
        Store::open_snapshot(&workspace.snapshot_path(), workspace.id),
        Err(StoreError::SnapshotVersion { found: 1, .. })
    ));
}
