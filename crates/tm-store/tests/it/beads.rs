//! The beads adapter: `bd export` lines written as records (ADR 0013).

use std::collections::BTreeMap;

use serde_json::{Value, json};
use tm_store::{BeadsImported, Store, StoreError};
use uuid::Uuid;

use crate::support::{ManualClock, TempDir, jace, links, open};

/// This repo's `bd export` when the adapter was written, with each text
/// shortened and its one person renamed.
const FIXTURE: &str = include_str!("../fixtures/beads.jsonl");

fn import(store: &mut Store, input: &str) -> Result<BeadsImported, StoreError> {
    store.import_beads(&mut input.as_bytes(), "CPL", &jace())
}

fn lines(beads: &[Value]) -> String {
    beads.iter().map(|bead| bead.to_string() + "\n").collect()
}

fn issue(imported: &BeadsImported, id: &str) -> Uuid {
    let source = format!("beads:{id}");
    imported
        .imported
        .records
        .iter()
        .find(|record| record.source == source)
        .unwrap()
        .issue
}

fn field(store: &Store, issue: Uuid, name: &str) -> Value {
    let view = store.issue(issue).unwrap().unwrap();
    view.fields.get(name).cloned().unwrap_or(Value::Null)
}

fn labels(store: &Store, issue: Uuid) -> Vec<String> {
    let view = store.issue(issue).unwrap().unwrap();
    view.sets
        .get("labels")
        .map(|labels| labels.values().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn this_repos_backlog_keeps_its_counts_statuses_dependencies_and_ready_queue() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    let imported = import(&mut store, FIXTURE).unwrap();
    let records = &imported.imported.records;
    assert_eq!(records.len(), 16);
    assert!(records.iter().all(|record| record.created));

    let mut statuses = BTreeMap::new();
    let mut parents = 0;
    for record in records {
        *statuses
            .entry(field(&store, record.issue, "status").to_string())
            .or_insert(0) += 1;
        parents += usize::from(!field(&store, record.issue, "parent").is_null());
        assert_eq!(field(&store, record.issue, "priority"), json!("medium"));
    }
    assert_eq!(
        statuses,
        BTreeMap::from([("\"done\"".to_owned(), 9), ("\"todo\"".to_owned(), 7)])
    );
    assert_eq!(parents, 15);
    let blocks = links(&store)
        .into_iter()
        .filter(|(link_type, _, _)| link_type == "blocks")
        .count();
    assert_eq!(blocks, 20);

    // bd ready listed these six: the epic and the five open tasks whose
    // blockers are closed.
    let mut ready: Vec<Uuid> = store
        .ready()
        .unwrap()
        .into_iter()
        .map(|ready| ready.issue)
        .collect();
    let mut expected: Vec<Uuid> = ["tm-u8g", "tm-u8g.9", "tm-u8g.10", "tm-u8g.11", "tm-u8g.12"]
        .into_iter()
        .chain(["tm-u8g.13"])
        .map(|id| issue(&imported, id))
        .collect();
    ready.sort();
    expected.sort();
    assert_eq!(ready, expected);
}

#[test]
fn a_blocks_dependency_is_filed_on_the_blocker() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    let imported = import(&mut store, FIXTURE).unwrap();
    let (core, adapter) = (issue(&imported, "tm-u8g.8"), issue(&imported, "tm-u8g.10"));
    assert!(links(&store).contains(&("blocks".into(), core, adapter)));
}

#[test]
fn types_statuses_and_text_map_as_the_adr_says() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    let imported = import(&mut store, FIXTURE).unwrap();

    let decision = issue(&imported, "tm-u8g.2");
    assert_eq!(field(&store, decision, "type"), json!("design"));
    assert_eq!(labels(&store, decision), ["beads-type:decision"]);
    let milestone = issue(&imported, "tm-u8g.14");
    assert_eq!(field(&store, milestone, "type"), json!("task"));
    assert_eq!(labels(&store, milestone), ["beads-type:milestone"]);
    assert_eq!(
        labels(&store, issue(&imported, "tm-u8g.10")),
        Vec::<String>::new()
    );

    let budget = issue(&imported, "tm-u8g.13");
    assert_eq!(
        field(&store, budget, "description"),
        json!(
            "Description of tm-u8g.13.\n\n## Acceptance criteria\n\nAcceptance criteria of tm-u8g.13.\n\n## Notes\n\nNotes of tm-u8g.13."
        )
    );
    assert_eq!(field(&store, budget, "reporter"), json!("ana@example.com"));

    let closed = issue(&imported, "tm-u8g.8");
    let comments = store.comments(closed).unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(
        (
            comments[0].actor.as_str(),
            comments[0].body.as_str(),
            comments[0].utc_ms
        ),
        (
            "ana@example.com",
            "Close reason of tm-u8g.8.",
            1_792_376_237_000
        )
    );
    assert_eq!(
        field(&store, closed, "created"),
        json!(1_792_206_248_000_i64)
    );

    assert_eq!(
        imported.dropped,
        BTreeMap::from([
            ("dependency created_at".into(), 35),
            ("dependency created_by".into(), 35),
            ("started_at".into(), 7),
        ])
    );
}

#[test]
fn importing_the_backlog_twice_writes_nothing_the_second_time() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    import(&mut store, FIXTURE).unwrap();
    let ops = |store: &Store| -> i64 {
        store
            .connection()
            .query_row("SELECT count(*) FROM ops", [], |row| row.get(0))
            .unwrap()
    };
    let before = ops(&store);
    let again = import(&mut store, FIXTURE).unwrap();
    assert_eq!(ops(&store), before);
    assert!(again.imported.records.iter().all(|record| {
        !record.created && record.written.is_empty() && record.conflicts.is_empty()
    }));
}

#[test]
fn what_tm_cannot_hold_is_counted_or_kept_as_a_label() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    let input = lines(&[
        json!({"_type": "issue", "id": "b-1", "title": "one", "status": "deferred",
        "issue_type": "feature", "priority": 4, "due_at": "2026-12-01T00:00:00Z",
        "comments": [{"id": 7, "author": "bo", "text": "hi", "reactions": ["+1"],
                      "created_at": "2026-01-02T00:00:00Z"}],
        "dependencies": [
            {"issue_id": "b-1", "depends_on_id": "b-2", "type": "caused-by"},
            {"issue_id": "b-1", "depends_on_id": "b-2", "type": "waits-for"},
            {"issue_id": "b-1", "depends_on_id": "gone", "type": "blocks"},
        ]}),
        json!({"_type": "memory", "key": "k"}),
        json!({"_type": "issue", "id": "b-2", "title": "two", "status": "open",
               "issue_type": "chore", "priority": 9}),
    ]);
    let imported = import(&mut store, &input).unwrap();
    let (one, two) = (issue(&imported, "b-1"), issue(&imported, "b-2"));

    assert_eq!(field(&store, one, "type"), json!("story"));
    assert_eq!(field(&store, one, "status"), json!("todo"));
    assert_eq!(field(&store, one, "priority"), json!("none"));
    assert_eq!(
        labels(&store, one),
        ["beads-status:deferred", "beads-type:feature"]
    );
    assert_eq!(field(&store, two, "priority"), json!("none"));
    assert_eq!(labels(&store, two), ["beads-type:chore"]);
    assert_eq!(links(&store), [("causes".into(), two, one)]);
    let comments = store.comments(one).unwrap();
    assert_eq!(
        (comments[0].actor.as_str(), comments[0].body.as_str()),
        ("bo", "hi")
    );
    assert_eq!(
        imported.dropped,
        BTreeMap::from([
            ("due_at".into(), 1),
            ("comment reactions".into(), 1),
            ("dependency on an Issue not imported".into(), 1),
            ("dependency waits-for".into(), 1),
            ("memory line".into(), 1),
            ("priority".into(), 1),
        ])
    );
}

#[test]
fn a_file_naming_one_issue_twice_or_one_depending_on_itself_writes_nothing() {
    for (beads, line, reason) in [
        (
            vec![
                json!({"id": "b-1", "title": "one"}),
                json!({"id": "b-1", "title": "again"}),
            ],
            2,
            "beads:b-1 is also line 1",
        ),
        (
            vec![json!({"id": "b-1", "title": "one", "dependencies": [
                {"issue_id": "b-1", "depends_on_id": "b-1", "type": "blocks"}]})],
            1,
            "beads:b-1 depends on itself",
        ),
    ] {
        let dir = TempDir::new();
        let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
        let err = import(&mut store, &lines(&beads)).unwrap_err();
        assert!(
            matches!(&err, StoreError::InvalidExport { line: l, reason: r } if *l == line && r == reason),
            "{err:?}"
        );
        assert_eq!(store.ready().unwrap(), []);
    }
}

#[test]
fn a_time_the_store_would_refuse_is_refused_when_read() {
    let comment = json!({"id": "c-1", "issue_id": "b-1", "author": "jace", "text": "hi",
        "created_at": "noon"});
    let bead = |extra: Value| {
        let mut bead = json!({"id": "b-1", "title": "one"});
        bead.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        bead
    };
    for (bead, at) in [
        (bead(json!({"created_at": "noon"})), "noon"),
        (bead(json!({"updated_at": "later"})), "later"),
        (bead(json!({"comments": [comment]})), "noon"),
    ] {
        let err = tm_store::read_beads(&mut lines(&[bead]).as_bytes(), "CPL").unwrap_err();
        assert!(
            matches!(&err, StoreError::InvalidExport { line: 1, reason }
                if reason.starts_with(&format!("{at:?} is not an RFC 3339 time"))),
            "{err:?}"
        );
    }
}

#[test]
fn keys_follow_creation_order_not_the_files_newest_first_order() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    let imported = import(&mut store, FIXTURE).unwrap();
    // The epic and tm-u8g.1 share a second, as do .9 and .10.
    for (id, key) in [
        ("tm-u8g", "CPL-1"),
        ("tm-u8g.1", "CPL-2"),
        ("tm-u8g.10", "CPL-11"),
        ("tm-u8g.15", "CPL-16"),
    ] {
        let view = store.issue(issue(&imported, id)).unwrap().unwrap();
        assert_eq!(view.key.as_deref(), Some(key), "{id}");
    }
}

#[test]
fn each_dependency_kind_becomes_its_link_from_the_right_end() {
    for (kind, link_type, from_on) in [
        ("blocks", "blocks", true),
        ("caused-by", "causes", true),
        ("related", "relates_to", false),
        ("relates-to", "relates_to", false),
        ("duplicates", "duplicates", false),
        ("supersedes", "supersedes", false),
        ("discovered-from", "derived_from", false),
    ] {
        let dir = TempDir::new();
        let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
        let input = lines(&[
            json!({"id": "b-1", "title": "one", "dependencies": [
                {"issue_id": "b-1", "depends_on_id": "b-2", "type": kind}]}),
            json!({"id": "b-2", "title": "two"}),
        ]);
        let imported = import(&mut store, &input).unwrap();
        let (one, two) = (issue(&imported, "b-1"), issue(&imported, "b-2"));
        let (from, to) = if from_on { (two, one) } else { (one, two) };
        let found = links(&store);
        // A symmetric Link is keyed from either end.
        let symmetric = (link_type.to_owned(), to, from);
        assert!(
            found == [(link_type.to_owned(), from, to)]
                || (link_type == "relates_to" && found == [symmetric]),
            "{kind}: {found:?}"
        );
    }
}

#[test]
fn a_bead_with_no_owner_is_reported_and_closed_by_its_creator() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    let input = lines(&[
        json!({"id": "b-1", "title": "one", "status": "hooked", "created_by": "Bo",
               "close_reason": "won't do", "closed_at": "2026-01-02T00:00:00Z"}),
        json!({"id": "b-2", "title": "two", "status": "in_progress", "dependencies": [
            {"issue_id": "b-2", "depends_on_id": "b-1", "type": "parent-child"},
            {"issue_id": "b-2", "depends_on_id": "b-3", "type": "parent-child"}]}),
        json!({"id": "b-3", "title": "three"}),
    ]);
    let imported = import(&mut store, &input).unwrap();
    let (one, two) = (issue(&imported, "b-1"), issue(&imported, "b-2"));
    assert_eq!(field(&store, one, "status"), json!("in_progress"));
    assert_eq!(labels(&store, one), ["beads-status:hooked"]);
    assert_eq!(field(&store, one, "reporter"), json!("Bo"));
    let comments = store.comments(one).unwrap();
    assert_eq!(
        (comments[0].actor.as_str(), comments[0].body.as_str()),
        ("Bo", "won't do")
    );
    assert_eq!(field(&store, two, "status"), json!("in_progress"));
    assert_eq!(field(&store, two, "parent"), json!(one.to_string()));
    assert_eq!(
        imported.dropped,
        BTreeMap::from([("second parent".into(), 1)])
    );
}

#[test]
fn a_label_the_bead_already_holds_is_not_added_twice() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    let input = lines(
        &[json!({"id": "b-1", "title": "one", "issue_type": "feature",
                               "labels": ["Beads-Type:feature"]})],
    );
    let imported = import(&mut store, &input).unwrap();
    assert_eq!(
        labels(&store, issue(&imported, "b-1")),
        ["Beads-Type:feature"]
    );
}

#[test]
fn a_deleted_bead_and_an_unmapped_edge_to_itself_are_counted_not_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    let input = lines(&[
        json!({"id": "b-1", "title": "one", "dependencies": [
            {"issue_id": "b-1", "depends_on_id": "b-1", "type": "waits-for"},
            {"issue_id": "b-1", "depends_on_id": "b-2", "type": "blocks"}]}),
        json!({"id": "b-2", "title": "gone", "status": "tombstone"}),
    ]);
    let imported = import(&mut store, &input).unwrap();
    assert_eq!(imported.imported.records.len(), 1);
    assert_eq!(
        imported.dropped,
        BTreeMap::from([
            ("dependency on an Issue not imported".into(), 1),
            ("dependency waits-for".into(), 1),
            ("tombstone Issue".into(), 1),
        ])
    );
}

#[test]
fn a_comment_id_given_twice_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    let comment = json!({"id": 1, "author": "bo", "text": "hi"});
    let input = lines(&[json!({"id": "b-1", "title": "one", "comments": [comment, comment]})]);
    let err = import(&mut store, &input).unwrap_err();
    assert!(
        matches!(&err, StoreError::InvalidExport { line: 1, reason } if reason == "comment id 1 appears twice"),
        "{err:?}"
    );
}
