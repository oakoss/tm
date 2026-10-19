//! Many short-lived processes writing at once, as parallel agents do.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;
use tm_core::Change;
use tm_store::{Draft, Store};
use uuid::Uuid;

use crate::support::{TempDir, draft, jace, run_children, set_field};

/// Set in child processes: the store path to write to, and the shared Issue.
const CHILD_DB: &str = "TM_STORE_TEST_CHILD_DB";
const CHILD_ISSUE: &str = "TM_STORE_TEST_CHILD_ISSUE";

const PROCESSES: usize = 50;
const WRITES_EACH: usize = 10;

#[test]
fn reads_in_one_read_see_one_commit() {
    let dir = TempDir::new();
    let mut writer = Store::open(&dir.db()).unwrap();
    let (project, _) = writer.create_project(&jace(), "CPL", "Core").unwrap();
    let (issue, _) = writer
        .create_issue(&jace(), project, "task", "before")
        .unwrap();
    let reader = Store::open(&dir.db()).unwrap();
    let title = |store: &Store| store.issue(issue).unwrap().unwrap().fields["title"].clone();
    let (first, second) = reader
        .read(|store| {
            let first = title(store);
            writer
                .append(Draft {
                    project,
                    ..draft(issue, set_field("title", json!("after")))
                })
                .unwrap();
            Ok::<_, tm_store::StoreError>((first, title(store)))
        })
        .unwrap();
    assert_eq!((first, second), (json!("before"), json!("before")));
    assert_eq!(title(&reader), json!("after"));
}

const CHILD_PAIR: &str = "TM_STORE_TEST_CHILD_PAIR";

#[test]
fn of_many_processes_unlinking_one_link_only_one_writes() {
    if let (Ok(db), Ok(pair)) = (std::env::var(CHILD_DB), std::env::var(CHILD_PAIR)) {
        let (from, to) = pair.split_once(',').unwrap();
        let mut store = Store::open(&PathBuf::from(db)).unwrap();
        let from = Uuid::parse_str(from).unwrap();
        let to = Uuid::parse_str(to).unwrap();
        match store.unlink_held(&jace(), "blocks", from, to) {
            Ok(_) | Err(tm_store::StoreError::NotLinked { .. }) => {}
            Err(err) => panic!("{err}"),
        }
        return;
    }

    let dir = TempDir::new();
    let mut store = Store::open(&dir.db()).unwrap();
    let (project, _) = store.create_project(&jace(), "CPL", "Core").unwrap();
    let (from, _) = store.create_issue(&jace(), project, "task", "a").unwrap();
    let (to, _) = store.create_issue(&jace(), project, "task", "b").unwrap();
    store.link(&jace(), "blocks", from, to).unwrap();
    drop(store);

    run_children(
        "concurrency::of_many_processes_unlinking_one_link_only_one_writes",
        20,
        |_, command| {
            command
                .env(CHILD_DB, dir.db())
                .env(CHILD_PAIR, format!("{from},{to}"));
        },
    );

    let store = Store::open(&dir.db()).unwrap();
    let removals: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM ops WHERE kind = 'link' AND payload LIKE '%\"present\":false%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(removals, 1);
    assert_eq!(store.links(from).unwrap(), Vec::new());
}

const CHILD_BATCH: &str = "TM_STORE_TEST_CHILD_BATCH";

#[test]
fn of_many_processes_undoing_one_batch_only_one_writes() {
    if let (Ok(db), Ok(batch)) = (std::env::var(CHILD_DB), std::env::var(CHILD_BATCH)) {
        let mut store = Store::open(&PathBuf::from(db)).unwrap();
        match store.undo(&jace(), Uuid::parse_str(&batch).unwrap()) {
            Ok(_) | Err(tm_store::StoreError::UndoConflict { .. }) => {}
            Err(err) => panic!("{err}"),
        }
        return;
    }

    let dir = TempDir::new();
    let mut store = Store::open(&dir.db()).unwrap();
    let (project, _) = store.create_project(&jace(), "CPL", "Core").unwrap();
    let (issue, _) = store.create_issue(&jace(), project, "task", "t").unwrap();
    let batch = store
        .append(Draft {
            project,
            ..draft(issue, set_field("priority", json!("high")))
        })
        .unwrap()
        .op_id;
    drop(store);

    run_children(
        "concurrency::of_many_processes_undoing_one_batch_only_one_writes",
        20,
        |_, command| {
            command
                .env(CHILD_DB, dir.db())
                .env(CHILD_BATCH, batch.to_string());
        },
    );

    let store = Store::open(&dir.db()).unwrap();
    let priority_writes: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM ops WHERE field = 'priority'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(priority_writes, 2, "the write and one undo");
}

#[test]
fn fifty_processes_write_without_losing_operations() {
    if let (Ok(db), Ok(issue)) = (std::env::var(CHILD_DB), std::env::var(CHILD_ISSUE)) {
        write_as_child(&PathBuf::from(db), Uuid::parse_str(&issue).unwrap());
        return;
    }

    let dir = TempDir::new();
    let mut store = Store::open(&dir.db()).unwrap();
    let (project, project_op) = store.create_project(&jace(), "CPL", "Core").unwrap();
    let (issue, mut created) = store.create_issue(&jace(), project, "task", "t").unwrap();
    created.push(project_op);
    drop(store);

    run_children(
        "concurrency::fifty_processes_write_without_losing_operations",
        PROCESSES,
        |_, command| {
            command
                .env(CHILD_DB, dir.db())
                .env(CHILD_ISSUE, issue.to_string());
        },
    );

    let store = Store::open(&dir.db()).unwrap();
    let conn = store.connection();
    let expected = i64::try_from(created.len() + PROCESSES * WRITES_EACH).unwrap();
    let (ops, distinct_hlc): (i64, i64) = conn
        .query_row("SELECT count(*), count(DISTINCT hlc) FROM ops", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(ops, expected);
    assert_eq!(distinct_hlc, expected, "two writes shared a clock value");
    let comments: i64 = conn
        .query_row("SELECT count(*) FROM comments", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        comments,
        i64::try_from(PROCESSES * WRITES_EACH / 2).unwrap()
    );
}

#[test]
fn opening_and_reading_never_wait_for_a_writer() {
    let dir = TempDir::new();
    let issue = {
        let mut store = Store::open(&dir.db()).unwrap();
        let (project, _) = store.create_project(&jace(), "CPL", "Core").unwrap();
        store.create_issue(&jace(), project, "task", "t").unwrap().0
    };
    let writer = rusqlite::Connection::open(dir.db()).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = Instant::now();
    {
        let store = Store::open(&dir.db()).unwrap();
        assert!(store.issue(issue).unwrap().is_some());
    }
    let waited = started.elapsed();
    writer.execute_batch("ROLLBACK").unwrap();
    assert!(waited < Duration::from_secs(1), "waited {waited:?}");
}

fn write_as_child(db: &std::path::Path, issue: Uuid) {
    let mut store = Store::open(db).unwrap();
    let project = store.issue(issue).unwrap().unwrap().project;
    for i in 0..WRITES_EACH {
        let change = if i % 2 == 0 {
            set_field("title", json!(format!("write {i}")))
        } else {
            Change::Comment {
                body: format!("write {i}"),
            }
        };
        let mut draft = draft(issue, change);
        draft.project = project;
        store.append(draft).unwrap();
    }
}
