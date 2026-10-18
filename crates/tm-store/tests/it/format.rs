//! A store written by an older format opens, upgrades and keeps every
//! operation's hash (ADR 0010).

use rusqlite::Connection;
use serde_json::json;
use tm_core::{Change, Hlc, ProjectId};
use tm_store::Store;

use crate::support::{ManualClock, TempDir, insert_acked, links, open, remote_op};

/// The log of a format 1 store, as tm wrote it before format v2: a Project,
/// two Issues, a Link, a Claim, a comment and a label.
const FORMAT_1: &str = include_str!("fixtures/format-1.sql");

fn format_1_store(dir: &TempDir) {
    Connection::open(dir.db())
        .unwrap()
        .execute_batch(FORMAT_1)
        .unwrap();
}

fn meta_format(store: &Store) -> i64 {
    store
        .connection()
        .query_row("SELECT v FROM meta WHERE k = 'format'", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn a_format_1_store_upgrades_and_replays_every_operation() {
    let dir = TempDir::new();
    format_1_store(&dir);
    let mut store = Store::open_with_clock(&dir.db(), Box::new(ManualClock::at(1))).unwrap();
    assert_eq!(meta_format(&store), 2);
    // Every stored hash still matches its content, or replay stops.
    assert_eq!(store.rebuild().unwrap(), 0);
    let (ops, empty): (i64, i64) = store
        .connection()
        .query_row(
            "SELECT count(*), count(*) FILTER (WHERE on_behalf_of IS NULL) FROM ops",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((ops, empty), (13, 13));

    let first = store.issue_by_key("CPL-1").unwrap().unwrap();
    let second = store.issue_by_key("CPL-2").unwrap().unwrap();
    for (issue, title, issue_type) in [(first, "first", "task"), (second, "second", "bug")] {
        let view = store.issue(issue).unwrap().unwrap();
        assert_eq!(view.fields["title"], json!(title));
        assert_eq!(view.fields["type"], json!(issue_type));
        assert_eq!(view.fields["status"], json!("todo"));
    }
    let labels = &store.issue(first).unwrap().unwrap().sets["labels"];
    assert_eq!(labels.keys().collect::<Vec<_>>(), ["core"]);
    assert_eq!(links(&store), [("blocks".to_owned(), first, second)]);
    assert_eq!(store.claim(first).unwrap().unwrap().agent, "claude");
    let body: String = store
        .connection()
        .query_row(
            "SELECT body FROM comments WHERE issue_id = ?1",
            [first.as_bytes()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(body, "a comment");
}

#[test]
fn a_failed_upgrade_leaves_the_store_at_format_1() {
    let dir = TempDir::new();
    format_1_store(&dir);
    let conn = Connection::open(dir.db()).unwrap();
    // Fails the upgrade's last step, after the column is added and the
    // trigger dropped.
    conn.execute_batch(
        "CREATE TRIGGER fail_stamp BEFORE UPDATE ON meta
         BEGIN SELECT RAISE(ABORT, 'stamp failed'); END;",
    )
    .unwrap();
    let trigger = |conn: &Connection| -> String {
        conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE name = 'ops_immutable'",
            [],
            |row| row.get(0),
        )
        .unwrap()
    };
    let before = trigger(&conn);
    let err = Store::open_with_clock(&dir.db(), Box::new(ManualClock::at(1)))
        .err()
        .unwrap();
    assert!(err.to_string().contains("stamp failed"), "{err}");
    let (format, columns): (i64, i64) = conn
        .query_row(
            "SELECT (SELECT v FROM meta WHERE k = 'format'),
                    (SELECT count(*) FROM pragma_table_info('ops') WHERE name = 'on_behalf_of')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((format, columns), (1, 0));
    assert_eq!(trigger(&conn), before);
}

#[test]
fn the_upgrade_guards_the_new_column() {
    let dir = TempDir::new();
    format_1_store(&dir);
    let store = Store::open_with_clock(&dir.db(), Box::new(ManualClock::at(1))).unwrap();
    let err = store
        .connection()
        .execute("UPDATE ops SET on_behalf_of = 'jace'", [])
        .unwrap_err();
    assert!(err.to_string().contains("immutable"), "{err}");
    drop(store);
    let store = Store::open_with_clock(&dir.db(), Box::new(ManualClock::at(1))).unwrap();
    assert_eq!(meta_format(&store), 2, "a second open changes nothing");
}

#[test]
fn on_behalf_of_survives_a_rebuild() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let project = ProjectId::new();
    let mut op = remote_op(
        7,
        project.uuid(),
        &Change::CreateProject {
            key: "AGT".into(),
            name: "Agents".into(),
        },
        Hlc::new(2_000, 0),
        0,
    );
    op.project = project;
    op.actor = "claude".into();
    op.on_behalf_of = Some("jace".into());
    insert_acked(&store, &op, 1, 2_000);
    assert_eq!(store.rebuild().unwrap(), 0);
    let stored: Option<String> = store
        .connection()
        .query_row(
            "SELECT on_behalf_of FROM ops WHERE op_id = ?1",
            [op.op_id.as_bytes()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored.as_deref(), Some("jace"));
    assert_eq!(store.project_by_key("AGT").unwrap(), Some(project));
}
