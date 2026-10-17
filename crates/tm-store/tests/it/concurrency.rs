//! Many short-lived processes writing at once, as parallel agents do.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::json;
use tm_core::Change;
use tm_store::Store;
use uuid::Uuid;

use crate::support::PROJECT;
use crate::support::{TempDir, draft, set_field};

/// Set in child processes: the store path to write to, and the shared Issue.
const CHILD_DB: &str = "TM_STORE_TEST_CHILD_DB";
const CHILD_ISSUE: &str = "TM_STORE_TEST_CHILD_ISSUE";

const PROCESSES: usize = 50;
const WRITES_EACH: usize = 10;

#[test]
fn fifty_processes_write_without_losing_operations() {
    if let (Ok(db), Ok(issue)) = (std::env::var(CHILD_DB), std::env::var(CHILD_ISSUE)) {
        write_as_child(&PathBuf::from(db), Uuid::parse_str(&issue).unwrap());
        return;
    }

    let dir = TempDir::new();
    let (issue, created) = Store::open(&dir.db())
        .unwrap()
        .create_issue("jace", PROJECT, "task", "t")
        .unwrap();

    let exe = std::env::current_exe().unwrap();
    let children: Vec<_> = (0..PROCESSES)
        .map(|_| {
            Command::new(&exe)
                .args([
                    "--exact",
                    "concurrency::fifty_processes_write_without_losing_operations",
                ])
                .args(["--test-threads", "1", "--quiet"])
                .env(CHILD_DB, dir.db())
                .env(CHILD_ISSUE, issue.to_string())
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success(), "a writer process failed");
    }

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

fn write_as_child(db: &std::path::Path, issue: Uuid) {
    let mut store = Store::open(db).unwrap();
    for i in 0..WRITES_EACH {
        let change = if i % 2 == 0 {
            set_field("title", json!(format!("write {i}")))
        } else {
            Change::Comment {
                body: format!("write {i}"),
            }
        };
        store.append(draft(issue, change)).unwrap();
    }
}
