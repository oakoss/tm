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
