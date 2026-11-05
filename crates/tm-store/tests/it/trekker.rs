//! The Trekker adapter: a Trekker database's epics and tasks written as
//! records (ADR 0013).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::{Value, json};
use tm_store::{Store, StoreError, TrekkerImported};
use uuid::Uuid;

use crate::support::{ManualClock, TempDir, jace, links, open};

/// The tables Trekker 1.12 creates, as far as the adapter reads them.
const SCHEMA: &str = "
CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE epics (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, title TEXT NOT NULL,
  description TEXT, status TEXT NOT NULL DEFAULT 'todo', priority INTEGER NOT NULL DEFAULT 2,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, rank TEXT);
CREATE TABLE tasks (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, epic_id TEXT,
  parent_task_id TEXT, title TEXT NOT NULL, description TEXT,
  priority INTEGER NOT NULL DEFAULT 2, status TEXT NOT NULL DEFAULT 'todo', tags TEXT,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, rank TEXT);
CREATE TABLE comments (id TEXT PRIMARY KEY, task_id TEXT NOT NULL, author TEXT NOT NULL,
  content TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE dependencies (id TEXT PRIMARY KEY, task_id TEXT NOT NULL,
  depends_on_id TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE TABLE events (id INTEGER PRIMARY KEY AUTOINCREMENT, action TEXT NOT NULL,
  entity_type TEXT NOT NULL, entity_id TEXT NOT NULL, snapshot TEXT, changes TEXT,
  created_at INTEGER NOT NULL);
";

/// One of each case the adapter maps, at times in Unix seconds as Trekker
/// writes them.
const ROWS: &str = "
INSERT INTO projects VALUES ('p', 'phantom', 1774665800, 1774665800);
INSERT INTO epics VALUES
  ('EPIC-1', 'p', 'Core', 'The core', 'completed', 0, 1774665800, 1774665900, 'a0'),
  ('EPIC-2', 'p', 'Later', NULL, 'todo', 2, 1774665846, 1774665846, NULL);
INSERT INTO tasks VALUES
  ('TREK-1', 'p', 'EPIC-1', NULL, 'Render', '', 1, 'todo', 'feature, core',
   1774665835, 1774665835, 'b1'),
  ('TREK-2', 'p', NULL, NULL, 'Crash', 'It crashes', 5, 'in_progress', 'bug,renderer',
   1774665840, 1774665850, NULL),
  ('TREK-3', 'p', 'EPIC-1', 'TREK-1', 'Measure', NULL, 2, 'completed', 'spike',
   1774665845, 1774665845, 'b2'),
  ('TREK-4', 'p', NULL, NULL, 'Drop it', NULL, 3, 'wont_fix', NULL,
   1774665846, 1774665846, NULL),
  ('TREK-5', 'p', NULL, NULL, 'Odd', NULL, 9, 'blocked',
   'Core,core,Ä,ä,trekker-status:blocked', 1774665847, 1774665847, NULL),
  ('TREK-10', 'p', 'EPIC-1', 'TREK-404', 'Orphan', NULL, 2, 'todo', NULL,
   1774665840, 1774665840, NULL),
  ('TREK-11', 'p', 'EPIC-2', 'TREK-1', 'Elsewhere', NULL, 2, 'todo', NULL,
   1774665848, 1774665848, NULL),
  ('TREK-12', 'p', NULL, 'TREK-1', 'Under its task', NULL, 2, 'todo', NULL,
   1774665849, 1774665849, NULL),
  ('TREK-13', 'p', 'EPIC-2', 'TREK-2', 'Task has none', NULL, 2, 'todo', NULL,
   1774665850, 1774665850, NULL);
INSERT INTO comments VALUES
  ('CMT-1', 'TREK-1', 'claude', 'Started on it', 1774667951, 1774667951),
  ('CMT-2', 'TREK-99', 'claude', 'On a task since deleted', 1774667952, 1774667952),
  ('CMT-3', 'TREK-2', 'claude', 'Edited in Trekker', 1774667953, 1774667999);
INSERT INTO dependencies VALUES
  ('DEP-1', 'TREK-2', 'TREK-1', 1774665900),
  ('DEP-2', 'TREK-4', 'TREK-404', 1774665901),
  ('DEP-3', 'TREK-3', 'TREK-3', 1774665902);
INSERT INTO events (action, entity_type, entity_id, created_at) VALUES
  ('create', 'task', 'TREK-1', 1774665835000),
  ('create', 'task', 'TREK-2', 1774665840000),
  ('update', 'task', 'TREK-2', 1774665850000);
";

fn trekker(dir: &TempDir) -> PathBuf {
    let path = dir.path().join("trekker.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    conn.execute_batch(ROWS).unwrap();
    path
}

fn import(store: &mut Store, path: &Path) -> Result<TrekkerImported, StoreError> {
    store.import_trekker(path, "CPL", &jace())
}

fn issue(imported: &TrekkerImported, id: &str) -> Uuid {
    let source = format!("trekker:{id}");
    imported
        .imported
        .records
        .iter()
        .find(|record| record.source == source)
        .unwrap()
        .issue
}

/// An Issue's type, Status, priority, labels and parent, as `show` holds them.
fn shape(store: &Store, issue: Uuid) -> Value {
    let view = store.issue(issue).unwrap().unwrap();
    let labels: Vec<&String> = view
        .sets
        .get("labels")
        .map(|labels| labels.values().collect())
        .unwrap_or_default();
    json!({
        "type": view.fields["type"],
        "status": view.fields["status"],
        "priority": view.fields.get("priority"),
        "labels": labels,
        "parent": view.fields.get("parent"),
    })
}

#[test]
fn epics_tasks_comments_and_dependencies_map_as_decided() {
    let dir = TempDir::new();
    let path = trekker(&dir);
    let before = std::fs::read(&path).unwrap();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    let imported = import(&mut store, &path).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the database is only read"
    );

    let epic = issue(&imported, "EPIC-1");
    let render = issue(&imported, "TREK-1");
    let id = |id: &str| json!(issue(&imported, id).to_string());
    let expected = [
        (
            "EPIC-1",
            json!({"type": "epic", "status": "done", "priority": "urgent",
            "labels": [], "parent": null}),
        ),
        (
            "TREK-1",
            json!({"type": "task", "status": "todo", "priority": "high",
            "labels": ["core", "feature"], "parent": id("EPIC-1")}),
        ),
        (
            "TREK-2",
            json!({"type": "bug", "status": "in_progress", "priority": "none",
            "labels": ["bug", "renderer"], "parent": null}),
        ),
        (
            "TREK-3",
            json!({"type": "sub_task", "status": "done", "priority": "medium",
            "labels": ["spike"], "parent": id("TREK-1")}),
        ),
        (
            "TREK-4",
            json!({"type": "task", "status": "cancelled", "priority": "low",
            "labels": [], "parent": null}),
        ),
        (
            "TREK-5",
            json!({"type": "task", "status": "todo", "priority": "none",
            "labels": ["Core", "trekker-status:blocked", "Ä"], "parent": null}),
        ),
        (
            "TREK-10",
            json!({"type": "task", "status": "todo", "priority": "medium",
            "labels": [], "parent": id("EPIC-1")}),
        ),
        (
            "TREK-11",
            json!({"type": "sub_task", "status": "todo", "priority": "medium",
            "labels": [], "parent": id("TREK-1")}),
        ),
    ];
    for (source, want) in expected {
        assert_eq!(shape(&store, issue(&imported, source)), want, "{source}");
    }
    let view = store.issue(render).unwrap().unwrap();
    assert!(
        !view.fields.contains_key("description"),
        "an empty one is none"
    );
    let view = store.issue(epic).unwrap().unwrap();
    assert_eq!(view.fields["description"], "The core");
    assert_eq!(
        view.fields["created"],
        json!(1_774_665_800_000_i64),
        "seconds read as seconds"
    );

    assert_eq!(
        links(&store),
        [("blocks".to_owned(), render, issue(&imported, "TREK-2"))],
        "TREK-2 depends on TREK-1, so TREK-1 blocks it"
    );
    let comments = store.comments(render).unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(
        (comments[0].actor.as_str(), comments[0].body.as_str()),
        ("claude", "Started on it")
    );
    assert_eq!(
        imported.dropped,
        BTreeMap::from([
            ("comment edit time".to_owned(), 1),
            ("comment on a task not imported".to_owned(), 1),
            ("dependency on a task not imported".to_owned(), 1),
            ("dependency on itself".to_owned(), 1),
            ("events".to_owned(), 3),
            ("parent task not imported".to_owned(), 1),
            ("priority".to_owned(), 1),
            ("rank".to_owned(), 3),
            ("sub-task's epic other than its task's".to_owned(), 2),
        ])
    );
}

#[test]
fn keys_follow_creation_order() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    let imported = import(&mut store, &trekker(&dir)).unwrap();
    // Oldest first; in one second an epic first, then by the id's number.
    let order = [
        "EPIC-1", "TREK-1", "TREK-2", "TREK-10", "TREK-3", "EPIC-2", "TREK-4", "TREK-5", "TREK-11",
        "TREK-12", "TREK-13",
    ];
    for (n, source) in order.into_iter().enumerate() {
        let view = store.issue(issue(&imported, source)).unwrap().unwrap();
        assert_eq!(view.key, Some(format!("CPL-{}", n + 1)), "{source}");
    }
}

/// A database with the adapter's tables, then `rows`.
fn database(dir: &TempDir, schema: &str, rows: &str) -> PathBuf {
    let path = dir.path().join(format!("{}.db", uuid::Uuid::now_v7()));
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(schema).unwrap();
    conn.execute_batch(rows).unwrap();
    path
}

#[test]
fn what_the_database_cannot_give_is_refused_before_anything_is_written() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    // Without primary keys, so a repeated comment id can be stored.
    let loose = SCHEMA
        .replace("id TEXT PRIMARY KEY, task_id", "id TEXT, task_id")
        .replace("id TEXT PRIMARY KEY, name", "id TEXT, name");
    let task = "INSERT INTO tasks VALUES ('TREK-1', 'p', NULL, NULL, 't', NULL, 2, 'todo',
        NULL, 1774665835, 1774665835, NULL);";
    let cases = [
        (
            format!(
                "{task} INSERT INTO comments VALUES ('CMT-1', 'TREK-1', 'a', 'x', 1, 1),
                ('CMT-1', 'TREK-1', 'a', 'y', 2, 2);"
            ),
            "appears twice",
        ),
        (
            "INSERT INTO tasks VALUES ('TREK-1', 'p', NULL, NULL, 't', NULL, 2, 'todo',
              NULL, 1774665835000000, 1774665835000000, NULL);"
                .to_owned(),
            "milliseconds",
        ),
        (
            "INSERT INTO projects VALUES ('p', 'a', 1, 1), ('q', 'b', 1, 1);".to_owned(),
            "2 Trekker projects",
        ),
    ];
    for (rows, why) in cases {
        let path = database(&dir, &loose, &rows);
        let err = import(&mut store, &path).unwrap_err();
        assert!(err.to_string().contains(why), "{why}: {err}");
    }
    let issues: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM issues", [], |row| row.get(0))
        .unwrap();
    assert_eq!(issues, 0);
}

#[test]
fn a_wal_database_is_refused_untouched_and_one_without_events_drops_none() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    let schema = SCHEMA.replace("CREATE TABLE events", "CREATE TABLE gone");
    let made = database(
        &dir,
        &schema,
        ROWS.split("INSERT INTO events").next().unwrap(),
    );
    let path = dir.path().join("o'k.db");
    std::fs::rename(made, &path).unwrap();
    let journal = |mode: &str| {
        let conn = Connection::open(&path).unwrap();
        conn.query_row(&format!("PRAGMA journal_mode = {mode}"), [], |_| Ok(()))
            .unwrap();
    };
    let beside = || {
        let mut files: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        files.sort();
        (files, std::fs::read(&path).unwrap())
    };

    journal("WAL");
    let before = beside();
    let err = import(&mut store, &path).unwrap_err();
    assert!(
        err.to_string()
            .contains(r"o'\''k.db' 'PRAGMA journal_mode=DELETE'"),
        "{err}"
    );
    assert_eq!(beside(), before, "nothing beside it, or in it, changed");

    journal("DELETE");
    let imported = import(&mut store, &path).unwrap();
    assert_eq!(imported.imported.records.len(), 11);
    assert!(
        !imported.dropped.contains_key("events"),
        "{:?}",
        imported.dropped
    );
}

#[test]
fn importing_twice_writes_nothing_the_second_time() {
    let dir = TempDir::new();
    let path = trekker(&dir);
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    import(&mut store, &path).unwrap();
    let ops = |store: &Store| -> i64 {
        store
            .connection()
            .query_row("SELECT count(*) FROM ops", [], |row| row.get(0))
            .unwrap()
    };
    let before = ops(&store);
    let again = import(&mut store, &path).unwrap();
    assert_eq!(ops(&store), before);
    assert!(again.imported.records.iter().all(|record| {
        !record.created && record.written.is_empty() && record.conflicts.is_empty()
    }));
}

#[test]
fn a_file_that_is_not_a_trekker_database_writes_nothing() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    let text = dir.path().join("notes.txt");
    std::fs::write(&text, "not a database").unwrap();
    let other = dir.path().join("other.db");
    Connection::open(&other)
        .unwrap()
        .execute_batch("CREATE TABLE things (id TEXT)")
        .unwrap();
    for path in [text, other, dir.path().join("missing.db")] {
        let err = import(&mut store, &path).unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidExport { .. }),
            "{path:?}: {err}"
        );
    }
    let issues: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM issues", [], |row| row.get(0))
        .unwrap();
    assert_eq!(issues, 0);
}

#[test]
fn html_entities_an_agent_wrote_are_decoded_and_other_ampersands_kept() {
    let dir = TempDir::new();
    let path = trekker(&dir);
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "INSERT INTO tasks VALUES ('TREK-20', 'p', NULL, NULL, 'Add a Build &amp; Run section',
               'Holds Mutex&lt;Pane&gt; while it takes &mut self && reads &amp;lt;', 2, 'todo',
               NULL, 1774665851, 1774665851, NULL);",
        )
        .unwrap();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    let imported = import(&mut store, &path).unwrap();
    let view = store.issue(issue(&imported, "TREK-20")).unwrap().unwrap();
    assert_eq!(view.fields["title"], "Add a Build & Run section");
    assert_eq!(
        view.fields["description"],
        "Holds Mutex<Pane> while it takes &mut self && reads &lt;"
    );
}
