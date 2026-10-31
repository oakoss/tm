//! Memories (ADR 0017): what a Project's sessions should know, kept as
//! records of their own.

use serde_json::{Value, json};
use tm_core::memory::Field;
use tm_core::{Change, Hlc};
use tm_store::{Store, StoreError};
use uuid::Uuid;

use crate::support::{
    ManualClock, OTHER, PROJECT, TempDir, claude, insert_acked, jace, open, remote_op,
};

/// A memory another Replica wrote with `fields`, brought in as a merge
/// would, sequenced from `seq`.
fn merged(store: &mut Store, seq: i64, fields: &[(Field, Value)]) -> Uuid {
    let id = Uuid::now_v7();
    let mut changes = vec![Change::CreateMemory];
    changes.extend(fields.iter().map(|(field, value)| Change::SetMemory {
        field: *field,
        value: value.clone(),
    }));
    for (n, change) in (0..).zip(&changes) {
        let op = remote_op(7, id, change, Hlc::new(2 + n, 0), 0);
        insert_acked(store, &op, seq + i64::try_from(n).unwrap(), 1);
    }
    store.rebuild().unwrap();
    id
}

fn exported(store: &Store) -> Vec<u8> {
    let mut out = Vec::new();
    store.export(&mut out, 1).unwrap();
    out
}

fn project_line(out: &[u8]) -> Value {
    out.split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice::<Value>(line).unwrap())
        .find(|line| line["_type"] == "project" && line["key"] == "CPL")
        .unwrap()
}

const WORKSPACE: Uuid = Uuid::from_u128(0x0190_a000_0000_7000_8000_0000_0000_0003);

fn store(dir: &TempDir, clock: &ManualClock) -> Store {
    let mut store = open(&dir.db(), clock);
    store.set_workspace_id(WORKSPACE).unwrap();
    store
}

fn keys(store: &Store) -> Vec<String> {
    store
        .memories(PROJECT)
        .unwrap()
        .into_iter()
        .filter_map(|memory| memory.key)
        .collect()
}

fn body(store: &Store, key: &str) -> Option<String> {
    store
        .memory(PROJECT, key)
        .unwrap()
        .and_then(|memory| memory.body)
}

#[test]
fn a_memory_is_kept_with_who_wrote_it_newest_first() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = store(&dir, &clock);
    let first = store
        .remember(&jace(), PROJECT, "Run the tests with -race", None)
        .unwrap();
    assert_eq!(first.memory.key.as_deref(), Some("run-the-tests-with"));
    assert_eq!(first.memory.written_by.as_deref(), Some("jace"));
    assert_eq!(first.memory.written_ms, Some(1_000));
    assert_eq!((first.replaced.clone(), first.restored), (None, false));
    clock.advance(1_000);
    let second = store
        .remember(
            &claude(),
            PROJECT,
            "Deploys go through staging",
            Some("deploys"),
        )
        .unwrap();
    assert_eq!(
        (
            second.memory.written_by.as_deref(),
            second.memory.written_for.as_deref()
        ),
        (Some("claude"), Some("jace"))
    );
    assert_eq!(keys(&store), ["deploys", "run-the-tests-with"]);
    assert!(store.memories(OTHER).unwrap().is_empty(), "one Project's");
}

#[test]
fn an_unnamed_memory_never_replaces_one_its_key_would_name() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_000));
    store
        .remember(&jace(), PROJECT, "Tests need -race always", None)
        .unwrap();
    let again = store
        .remember(
            &jace(),
            PROJECT,
            "Tests need -race always, even locally",
            None,
        )
        .unwrap();
    assert_eq!(
        again.memory.key.as_deref(),
        Some("tests-need-race-always-2")
    );
    assert_eq!(again.replaced, None);
    assert_eq!(again.alike, ["tests-need-race-always"], "named as alike");
    assert_eq!(
        body(&store, "tests-need-race-always").as_deref(),
        Some("Tests need -race always")
    );
}

#[test]
fn a_named_key_replaces_and_says_what_it_held() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_000));
    store
        .remember(&jace(), PROJECT, "Use Postgres", Some("database"))
        .unwrap();
    let replaced = store
        .remember(&claude(), PROJECT, "Use SQLite", Some("database"))
        .unwrap();
    assert_eq!(
        replaced.replaced,
        Some(("Use Postgres".to_owned(), Some("jace".to_owned())))
    );
    assert_eq!(body(&store, "database").as_deref(), Some("Use SQLite"));
    let same = store
        .remember(&claude(), PROJECT, "Use SQLite", Some("database"))
        .unwrap();
    assert!(same.ops.is_empty(), "nothing to write: {:?}", same.ops);
    assert_eq!(same.replaced, None);
}

#[test]
fn a_forgotten_memory_comes_back_by_its_key_or_an_undo() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_000));
    store
        .remember(&jace(), PROJECT, "Use Postgres", Some("database"))
        .unwrap();
    let (_, ops) = store.forget(&jace(), PROJECT, "database").unwrap();
    assert_eq!(keys(&store), Vec::<String>::new());
    let missing = store.forget(&jace(), PROJECT, "database");
    assert!(
        matches!(missing, Err(StoreError::MemoryNotFound(_))),
        "{missing:?}"
    );

    store.undo(&jace(), ops[0].op_id).unwrap();
    assert_eq!(body(&store, "database").as_deref(), Some("Use Postgres"));

    store.forget(&jace(), PROJECT, "database").unwrap();
    let back = store
        .remember(&jace(), PROJECT, "Use SQLite", Some("database"))
        .unwrap();
    assert!(back.restored);
    assert_eq!(keys(&store), ["database"]);
    assert_eq!(body(&store, "database").as_deref(), Some("Use SQLite"));
}

#[test]
fn undoing_a_remember_forgets_it_or_restores_what_it_replaced() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_000));
    let made = store
        .remember(&jace(), PROJECT, "Use Postgres", Some("database"))
        .unwrap();
    let replaced = store
        .remember(&jace(), PROJECT, "Use SQLite", Some("database"))
        .unwrap();
    store.undo(&jace(), replaced.ops[0].op_id).unwrap();
    assert_eq!(body(&store, "database").as_deref(), Some("Use Postgres"));
    store.undo(&jace(), made.ops[0].op_id).unwrap();
    assert_eq!(
        keys(&store),
        Vec::<String>::new(),
        "undoing the create forgets it"
    );
}

#[test]
fn a_body_is_short_and_a_key_is_one_word() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_000));
    for (body, key) in [
        ("   ", None),
        (&*"x".repeat(501), None),
        ("fine", Some("Has Spaces")),
    ] {
        let refused = store.remember(&jace(), PROJECT, body, key);
        assert!(
            matches!(refused, Err(StoreError::Invalid { .. })),
            "{refused:?}"
        );
    }
    store
        .remember(&jace(), PROJECT, &"x".repeat(500), Some("long"))
        .unwrap();
}

#[test]
fn memories_survive_a_rebuild_and_an_export_into_a_fresh_store() {
    let (dir, other) = (TempDir::new(), TempDir::new());
    let clock = ManualClock::at(1_000);
    let mut store = store(&dir, &clock);
    store
        .remember(&jace(), PROJECT, "Use SQLite", Some("database"))
        .unwrap();
    store
        .remember(&claude(), PROJECT, "Run the tests with -race", Some("race"))
        .unwrap();
    store
        .remember(&jace(), PROJECT, "Forgotten soon", Some("gone"))
        .unwrap();
    store.forget(&jace(), PROJECT, "gone").unwrap();
    let before = store.memories(PROJECT).unwrap();
    store.rebuild().unwrap();
    assert_eq!(store.memories(PROJECT).unwrap(), before);

    let mut out = Vec::new();
    store.export(&mut out, 1).unwrap();
    let project_line = out
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice::<Value>(line).unwrap())
        .find(|line| line["_type"] == "project" && line["key"] == "CPL")
        .unwrap();
    let held: Vec<&Value> = project_line["memories"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    assert_eq!(
        held.len(),
        2,
        "the forgotten one is left out: {project_line}"
    );

    let mut fresh = open(&other.db(), &clock);
    let imported = fresh.import_records(&mut out.as_slice(), &jace()).unwrap();
    assert_eq!(imported.memories.len(), 2);
    assert!(imported.memories.iter().all(|outcome| outcome.created));
    let mut keys: Vec<String> = fresh
        .memories(PROJECT)
        .unwrap()
        .into_iter()
        .filter_map(|memory| memory.key)
        .collect();
    keys.sort();
    assert_eq!(keys, ["database", "race"]);

    let again = fresh.import_records(&mut out.as_slice(), &jace()).unwrap();
    assert!(
        again
            .memories
            .iter()
            .all(|outcome| !outcome.created && outcome.batches.is_empty()),
        "a second import writes nothing: {:?}",
        again.memories
    );
}

#[test]
fn an_import_keeps_tm_s_body_where_both_changed_it() {
    let (dir, other) = (TempDir::new(), TempDir::new());
    let clock = ManualClock::at(1_000);
    let mut source = store(&dir, &clock);
    source
        .remember(&jace(), PROJECT, "Use SQLite", Some("database"))
        .unwrap();
    let mut out = Vec::new();
    source.export(&mut out, 1).unwrap();
    let mut here = open(&other.db(), &clock);
    here.remember(&jace(), PROJECT, "Use Postgres", Some("database"))
        .unwrap();
    let imported = here.import_records(&mut out.as_slice(), &jace()).unwrap();
    let outcome = &imported.memories[0];
    assert!(outcome.by_key && !outcome.created);
    assert_eq!(
        outcome
            .conflicts
            .iter()
            .map(|conflict| (conflict.field.as_str(), conflict.source.clone()))
            .collect::<Vec<_>>(),
        [("body", json!("Use SQLite"))]
    );
    assert_eq!(
        here.memory(PROJECT, "database")
            .unwrap()
            .and_then(|memory| memory.body)
            .as_deref(),
        Some("Use Postgres")
    );
}

#[test]
fn a_key_a_merge_shared_is_named_by_id_and_stays_writable() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_000));
    let ours = store
        .remember(&jace(), PROJECT, "Use SQLite", Some("db"))
        .unwrap()
        .memory
        .id;
    let theirs = merged(
        &mut store,
        10,
        &[
            (Field::Key, json!("db")),
            (Field::Body, json!("Use Postgres")),
        ],
    );
    let shared = |result: Result<_, StoreError>| match result {
        Err(StoreError::MemoryKeyShared { ids, .. }) => ids,
        other => panic!("{other:?}"),
    };
    let mut ids = shared(store.memory(PROJECT, "db").map(|_| ()));
    ids.sort();
    let mut both = vec![ours, theirs];
    both.sort();
    assert_eq!(ids, both);
    shared(store.forget(&jace(), PROJECT, "db").map(|_| ()));
    shared(
        store
            .remember(&jace(), PROJECT, "x", Some("db"))
            .map(|_| ()),
    );

    let by_id = theirs.to_string();
    let updated = store
        .remember(&jace(), PROJECT, "Use Postgres 16", Some(&by_id))
        .unwrap();
    assert_eq!(
        updated.memory.id, theirs,
        "a body-only write on a shared key"
    );
    store.forget(&jace(), PROJECT, &by_id).unwrap();
    let left = store.memory(PROJECT, "db").unwrap().unwrap();
    assert_eq!(left.id, ours);
}

#[test]
fn a_shared_key_imports_into_a_fresh_store() {
    let (dir, other) = (TempDir::new(), TempDir::new());
    let clock = ManualClock::at(1_000);
    let mut store = store(&dir, &clock);
    store
        .remember(&jace(), PROJECT, "Use SQLite", Some("db"))
        .unwrap();
    merged(
        &mut store,
        10,
        &[
            (Field::Key, json!("db")),
            (Field::Body, json!("Use Postgres")),
        ],
    );
    let out = exported(&store);
    let mut fresh = open(&other.db(), &clock);
    let imported = fresh.import_records(&mut out.as_slice(), &jace()).unwrap();
    assert_eq!(imported.memories.len(), 2);
    let mut bodies: Vec<String> = fresh
        .memories(PROJECT)
        .unwrap()
        .into_iter()
        .filter_map(|memory| memory.body)
        .collect();
    bodies.sort();
    assert_eq!(bodies, ["Use Postgres", "Use SQLite"]);
    let again = fresh.import_records(&mut out.as_slice(), &jace()).unwrap();
    assert!(
        again
            .memories
            .iter()
            .all(|outcome| outcome.batches.is_empty()),
        "{:?}",
        again.memories
    );
}

#[test]
fn a_memory_a_merge_left_without_a_key_is_exported_with_what_it_has() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = store(&dir, &clock);
    let id = merged(&mut store, 10, &[(Field::Body, json!("Half arrived"))]);
    let out = exported(&store);
    let line = project_line(&out);
    let held = &line["memories"][0];
    assert_eq!(held["source"], format!("tm:{id}"));
    assert_eq!(held["body"], "Half arrived");
    assert_eq!(held.get("key"), None, "{line}");

    let again = store.import_records(&mut out.as_slice(), &jace()).unwrap();
    assert!(again.memories[0].batches.is_empty(), "found by its id");
    let fresh_dir = TempDir::new();
    let mut fresh = open(&fresh_dir.db(), &clock);
    let refused = fresh.import_records(&mut out.as_slice(), &jace());
    assert!(
        refused.is_err_and(|err| err.to_string().contains("no key or no body")),
        "as an unnamed Short-list is"
    );
}

#[test]
fn bringing_back_a_key_a_newer_memory_holds_is_refused() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_000));
    store
        .remember(&jace(), PROJECT, "Use Postgres", Some("db"))
        .unwrap();
    let (_, ops) = store.forget(&jace(), PROJECT, "db").unwrap();
    let newer = store.remember(&jace(), PROJECT, "db", None).unwrap();
    assert_eq!(newer.memory.key.as_deref(), Some("db"));
    let refused = store.undo(&jace(), ops[0].op_id);
    assert!(
        matches!(&refused, Err(StoreError::Invalid { field, .. }) if field == "key"),
        "{refused:?}"
    );
}

#[test]
fn undoing_a_change_to_a_memory_forgotten_since_waits_for_its_forget() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_000));
    store
        .remember(&jace(), PROJECT, "Use Postgres", Some("db"))
        .unwrap();
    let changed = store
        .remember(&jace(), PROJECT, "Use SQLite", Some("db"))
        .unwrap();
    store.forget(&jace(), PROJECT, "db").unwrap();
    let refused = store.undo(&jace(), changed.ops[0].op_id);
    assert!(
        matches!(&refused, Err(StoreError::CannotUndo { reason, .. }) if reason.contains("undo its forget first")),
        "{refused:?}"
    );
}

#[test]
fn a_memory_unlike_the_others_names_none() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_000));
    store
        .remember(&jace(), PROJECT, "Tests need the race flag", None)
        .unwrap();
    let other = store
        .remember(&jace(), PROJECT, "Deploys go through staging nightly", None)
        .unwrap();
    assert_eq!(other.alike, Vec::<String>::new());
}

#[test]
fn an_import_writes_what_only_the_source_changed_and_keeps_what_only_tm_did() {
    let (dir, other) = (TempDir::new(), TempDir::new());
    let clock = ManualClock::at(1_000);
    let mut source = store(&dir, &clock);
    source
        .remember(&jace(), PROJECT, "Use SQLite", Some("db"))
        .unwrap();
    let mut here = open(&other.db(), &clock);
    here.import_records(&mut exported(&source).as_slice(), &jace())
        .unwrap();

    source
        .remember(&jace(), PROJECT, "Use SQLite 3", Some("db"))
        .unwrap();
    let out = exported(&source);
    let imported = here.import_records(&mut out.as_slice(), &jace()).unwrap();
    let outcome = &imported.memories[0];
    assert_eq!(outcome.written, ["body"]);
    assert_eq!(outcome.conflicts, []);
    assert_eq!(body(&here, "db").as_deref(), Some("Use SQLite 3"));

    here.remember(&jace(), PROJECT, "Use SQLite 3, in WAL mode", Some("db"))
        .unwrap();
    let imported = here.import_records(&mut out.as_slice(), &jace()).unwrap();
    let outcome = &imported.memories[0];
    assert_eq!((outcome.conflicts.len(), outcome.batches.len()), (0, 0));
    assert_eq!(
        body(&here, "db").as_deref(),
        Some("Use SQLite 3, in WAL mode")
    );
}

#[test]
fn an_import_leaves_a_memory_forgotten_here_forgotten() {
    let (dir, other) = (TempDir::new(), TempDir::new());
    let clock = ManualClock::at(1_000);
    let mut source = store(&dir, &clock);
    source
        .remember(&jace(), PROJECT, "Use SQLite", Some("db"))
        .unwrap();
    let out = exported(&source);
    let mut here = open(&other.db(), &clock);
    here.import_records(&mut out.as_slice(), &jace()).unwrap();
    here.forget(&jace(), PROJECT, "db").unwrap();
    let imported = here.import_records(&mut out.as_slice(), &jace()).unwrap();
    let outcome = &imported.memories[0];
    assert!(
        outcome.forgotten && outcome.batches.is_empty(),
        "{outcome:?}"
    );
    assert_eq!(here.memories(PROJECT).unwrap(), []);
}

/// `out` with each memory key `from` given as `to`, as a hand edit would.
fn rekeyed(out: &[u8], from: &str, to: &str) -> Vec<u8> {
    String::from_utf8(out.to_vec())
        .unwrap()
        .replace(&format!("\"key\":\"{from}\""), &format!("\"key\":\"{to}\""))
        .into_bytes()
}

fn live_keys(store: &Store) -> Vec<String> {
    let mut keys = keys(store);
    keys.sort();
    keys
}

#[test]
fn an_import_never_renames_a_memory_onto_a_key_another_holds() {
    let (dir, other) = (TempDir::new(), TempDir::new());
    let clock = ManualClock::at(1_000);
    let mut source = store(&dir, &clock);
    source
        .remember(&jace(), PROJECT, "Use SQLite", Some("db"))
        .unwrap();
    let out = exported(&source);
    let mut here = open(&other.db(), &clock);
    here.import_records(&mut out.as_slice(), &jace()).unwrap();
    here.remember(&jace(), PROJECT, "Cache in Redis", Some("cache"))
        .unwrap();
    let refused = here.import_records(&mut rekeyed(&out, "db", "cache").as_slice(), &jace());
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(live_keys(&here), ["cache", "db"]);
}

#[test]
fn a_memory_matched_by_key_is_found_by_its_source_after() {
    let (dir, other) = (TempDir::new(), TempDir::new());
    let clock = ManualClock::at(1_000);
    let mut source = store(&dir, &clock);
    source
        .remember(&jace(), PROJECT, "Use SQLite", Some("db"))
        .unwrap();
    let out = exported(&source);

    let mut here = open(&other.db(), &clock);
    let ours = here
        .remember(&jace(), PROJECT, "Use SQLite", Some("db"))
        .unwrap()
        .memory
        .id;
    let matched = here.import_records(&mut out.as_slice(), &jace()).unwrap();
    assert!(matched.memories[0].by_key);
    let rekeyed_import = here
        .import_records(&mut rekeyed(&out, "db", "database").as_slice(), &jace())
        .unwrap();
    let outcome = &rekeyed_import.memories[0];
    assert_eq!(
        (outcome.memory, outcome.created),
        (ours, false),
        "the same one"
    );
    assert_eq!(live_keys(&here), ["db"], "none added");

    here.forget(&jace(), PROJECT, "db").unwrap();
    let again = here.import_records(&mut out.as_slice(), &jace()).unwrap();
    let outcome = &again.memories[0];
    assert!(outcome.forgotten && !outcome.created, "{outcome:?}");
    assert_eq!(live_keys(&here), Vec::<String>::new());
}

#[test]
fn an_import_of_a_key_two_memories_here_share_makes_its_own() {
    let (dir, other) = (TempDir::new(), TempDir::new());
    let clock = ManualClock::at(1_000);
    let mut here = store(&dir, &clock);
    here.remember(&jace(), PROJECT, "Use SQLite", Some("db"))
        .unwrap();
    merged(
        &mut here,
        10,
        &[
            (Field::Key, json!("db")),
            (Field::Body, json!("Use Postgres")),
        ],
    );
    let mut source = store(&other, &clock);
    source
        .remember(&jace(), PROJECT, "Use DuckDB", Some("db"))
        .unwrap();
    let imported = here
        .import_records(&mut exported(&source).as_slice(), &jace())
        .unwrap();
    let outcome = &imported.memories[0];
    assert!(outcome.created && !outcome.by_key, "{outcome:?}");
    let mut bodies: Vec<String> = here
        .memories(PROJECT)
        .unwrap()
        .into_iter()
        .filter_map(|memory| memory.body)
        .collect();
    bodies.sort();
    assert_eq!(bodies, ["Use DuckDB", "Use Postgres", "Use SQLite"]);
}
