//! Record import: `issue` records written as ordinary writes, and a re-import
//! compared three ways (ADR 0013).

use serde_json::{Value, json};
use tm_core::Change;
use tm_store::{RecordsImported, Store, StoreError};
use uuid::Uuid;

use tm_core::Hlc;

use crate::support::{
    ManualClock, TempDir, draft, field, insert_acked, jace, new_issue, open, remote_op, set_field,
};

const WORKSPACE: Uuid = Uuid::from_u128(0x0190_a000_0000_7000_8000_0000_0000_0002);

fn store(dir: &TempDir, clock: &ManualClock) -> Store {
    let mut store = open(&dir.db(), clock);
    store.set_workspace_id(WORKSPACE).unwrap();
    store
}

fn file(lines: &[Value]) -> Vec<u8> {
    lines
        .iter()
        .flat_map(|line| {
            let mut bytes = serde_json::to_vec(line).unwrap();
            bytes.push(b'\n');
            bytes
        })
        .collect()
}

fn import(store: &mut Store, lines: &[Value]) -> Result<RecordsImported, StoreError> {
    store.import_records(&mut file(lines).as_slice(), &jace())
}

fn ops(store: &Store) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM ops", [], |row| row.get(0))
        .unwrap()
}

/// Two beads Issues as an adapter writes them: a child with labels, a
/// comment by an Agent and a blocking Link to its parent.
fn records() -> Vec<Value> {
    vec![
        json!({
            "_type": "issue", "source": "beads:a.1", "project": "CPL", "type": "task",
            "title": "child", "status": "in_progress", "priority": "high",
            "assignee": "ana", "reporter": "ana", "labels": ["Backend", "cli"],
            "parent": "beads:a", "links": [{"type": "blocks", "target": "beads:a"}],
            "comments": [
                {"id": "c1", "actor": "claude", "on_behalf_of": "ana", "body": "first",
                 "at": "2026-01-02T00:00:00Z"},
            ],
            "created": "2026-01-01T00:00:00Z", "updated": "2026-01-03T00:00:00Z",
        }),
        json!({
            "_type": "issue", "source": "beads:a", "project": "CPL", "type": "epic",
            "title": "parent", "status": "todo", "reporter": "ana",
            "created": "2026-01-01T00:00:00Z",
        }),
    ]
}

/// The Issue an outcome names, by its source.
fn issue(imported: &RecordsImported, source: &str) -> Uuid {
    imported
        .records
        .iter()
        .find(|record| record.source == source)
        .unwrap()
        .issue
}

#[test]
fn records_become_issues_with_their_source_times_and_people() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    let imported = import(&mut store, &records()).unwrap();
    assert!(imported.records.iter().all(|record| record.created));
    let (child, parent) = (issue(&imported, "beads:a.1"), issue(&imported, "beads:a"));

    let view = store.issue(child).unwrap().unwrap();
    for (name, value) in [
        ("title", json!("child")),
        ("status", json!("in_progress")),
        ("priority", json!("high")),
        ("assignee", json!("ana")),
        ("reporter", json!("ana")),
        ("parent", json!(parent.to_string())),
        ("created", json!(1_767_225_600_000_i64)),
        ("updated", json!(1_767_398_400_000_i64)),
    ] {
        assert_eq!(view.fields.get(name), Some(&value), "{name}");
    }
    let labels: Vec<&String> = view.sets["labels"].values().collect();
    assert_eq!(labels, ["Backend", "cli"]);
    assert_eq!(view.key.as_deref(), Some("CPL-1"));
    let links = store.links(child).unwrap();
    assert_eq!(
        (links[0].link_type.as_str(), links[0].other),
        ("blocks", parent)
    );
    let comments = store.comments(child).unwrap();
    assert_eq!(
        (
            comments[0].actor.as_str(),
            comments[0].on_behalf_of.as_deref(),
            comments[0].utc_ms
        ),
        ("claude", Some("ana"), 1_767_312_000_000)
    );

    let unsourced: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM ops WHERE issue_id = ?1 AND source IS NULL",
            [child.as_bytes()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unsourced, 0, "every write names its source");
    assert_eq!(
        store.undo_candidates("jace", 10).unwrap(),
        Vec::<Uuid>::new()
    );
    assert_eq!(
        store.undo_candidates("ana", 10).unwrap(),
        Vec::<Uuid>::new()
    );
}

#[test]
fn importing_a_file_twice_writes_nothing_the_second_time() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    import(&mut store, &records()).unwrap();
    let before = ops(&store);
    let again = import(&mut store, &records()).unwrap();
    assert_eq!(ops(&store), before);
    for record in &again.records {
        assert!(!record.created, "{record:?}");
        assert!(record.written.is_empty(), "{record:?}");
        assert!(record.conflicts.is_empty(), "{record:?}");
    }
}

#[test]
fn the_base_is_what_the_latest_import_wrote() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    import(&mut store, &records()).unwrap();
    let mut renamed = records();
    renamed[0]["title"] = json!("renamed");
    assert_eq!(
        import(&mut store, &renamed).unwrap().records[0].written,
        ["title"]
    );
    let before = ops(&store);
    let again = import(&mut store, &renamed).unwrap();
    assert!(
        again.records[0].written.is_empty(),
        "{:?}",
        again.records[0]
    );
    assert_eq!(ops(&store), before);
}

/// Imports `records`, edits the child in tm, changes the source, and
/// imports again; `between` runs before the second import.
fn reimport(between: impl FnOnce(Store, &TempDir) -> Store) -> (Store, RecordsImported, Uuid) {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_900_000_000_000);
    let mut store = store(&dir, &clock);
    let first = import(&mut store, &records()).unwrap();
    let child = issue(&first, "beads:a.1");
    store
        .append_all(vec![
            draft(child, set_field("priority", json!("low"))),
            draft(child, set_field("status", json!("in_review"))),
        ])
        .unwrap();
    let mut changed = records();
    let source = changed[0].as_object_mut().unwrap();
    source.insert("title".into(), json!("child, renamed"));
    source.insert("status".into(), json!("done"));
    source.remove("assignee");
    source.insert("labels".into(), json!(["cli"]));
    source.remove("links");
    source.insert("updated_by".into(), json!("bo"));
    let mut store = between(store, &dir);
    let second = import(&mut store, &changed).unwrap();
    (store, second, child)
}

fn assert_three_way(store: &Store, second: &RecordsImported, child: Uuid) {
    let outcome = &second.records[0];
    assert_eq!(outcome.written, ["title", "assignee"]);
    assert_eq!(
        outcome.removed,
        [
            "labels/backend".to_owned(),
            format!("links/blocks/{}", issue(second, "beads:a"))
        ]
    );
    assert_eq!(outcome.conflicts.len(), 1, "{:?}", outcome.conflicts);
    assert_eq!(
        (
            outcome.conflicts[0].field.as_str(),
            &outcome.conflicts[0].tm,
            &outcome.conflicts[0].source
        ),
        ("status", &json!("in_review"), &json!("done"))
    );
    assert_eq!(field(store, child, "title"), Some(json!("child, renamed")));
    assert_eq!(
        field(store, child, "priority"),
        Some(json!("low")),
        "tm's edit stands"
    );
    assert_eq!(field(store, child, "status"), Some(json!("in_review")));
    assert_eq!(
        field(store, child, "assignee"),
        None,
        "dropped at the source"
    );
    assert_eq!(store.links(child).unwrap(), []);
    let writer: String = store
        .connection()
        .query_row(
            "SELECT actor FROM ops WHERE issue_id = ?1 AND field = 'title' ORDER BY seq DESC",
            [child.as_bytes()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(writer, "bo", "a re-import writes as updated_by");
}

#[test]
fn a_reimport_writes_what_only_the_source_changed_and_keeps_tm_edits() {
    let (store, second, child) = reimport(|store, _| store);
    assert_three_way(&store, &second, child);
}

#[test]
fn what_an_import_wrote_survives_an_export_and_restore() {
    let (store, second, child) = reimport(|store, dir| {
        let mut out = Vec::new();
        store.export(&mut out, 1).unwrap();
        drop(store);
        let path = dir.path().join("restored.db");
        let mut copy = Store::open_with_clock(&path, Box::new(ManualClock::at(1_900_000_000_000)))
            .unwrap()
            .with_scheme(crate::support::scheme());
        copy.set_workspace_id(WORKSPACE).unwrap();
        copy.restore(&mut out.as_slice()).unwrap();
        copy
    });
    assert_three_way(&store, &second, child);
}

/// What tm changed since an export of this Workspace, a set and a cleared
/// value of each kind, survives importing that export's records.
#[test]
fn an_older_export_of_this_workspace_overwrites_nothing() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = store(&dir, &clock);
    let (issue, parent) = (new_issue(&mut store), new_issue(&mut store));
    let comment = |body: &str| Change::Comment { body: body.into() };
    store
        .append_all(vec![
            draft(issue, set_field("assignee", json!("ana"))),
            draft(issue, set_field("parent", json!(parent.to_string()))),
            draft(issue, label("x", true)),
            draft(issue, comment("kept")),
        ])
        .unwrap();
    store.link(&jace(), "blocks", issue, parent).unwrap();
    let mut out = Vec::new();
    store.export(&mut out, 1).unwrap();
    clock.advance(1_000);
    store
        .append_all(vec![
            draft(issue, set_field("title", json!("new"))),
            draft(issue, set_field("assignee", Value::Null)),
            draft(issue, set_field("parent", Value::Null)),
            draft(issue, label("x", false)),
            draft(issue, comment("later")),
        ])
        .unwrap();
    store.unlink(&jace(), "blocks", issue, parent).unwrap();
    let before = ops(&store);

    let imported = store.import_records(&mut out.as_slice(), &jace()).unwrap();
    assert_eq!(ops(&store), before, "nothing written");
    let outcome = &imported.records[0];
    assert!(!outcome.created);
    assert_eq!(outcome.issue, issue);
    assert_eq!(
        outcome
            .conflicts
            .iter()
            .map(|conflict| conflict.field.as_str())
            .collect::<Vec<_>>(),
        [
            "title".to_owned(),
            "assignee".to_owned(),
            "labels/x".to_owned(),
            "parent".to_owned(),
            format!("links/blocks/{parent}"),
        ]
    );
    assert_eq!(imported.records[1].conflicts, []);
    let view = store.issue(issue).unwrap().unwrap();
    assert_eq!(view.fields.get("title"), Some(&json!("new")));
    assert_eq!(view.fields.get("assignee"), None);
    assert_eq!(view.fields.get("parent"), None);
    assert_eq!(view.sets.get("labels"), None);
    assert_eq!(store.links(issue).unwrap(), []);
    let comments = store.comments(issue).unwrap();
    assert_eq!(comments.len(), 2, "no comment repeated: {comments:?}");
}

fn label(elem: &str, add: bool) -> Change {
    let (field, label, elem) = ("labels".into(), String::new(), elem.into());
    if add {
        Change::Add {
            field,
            label,
            elem,
            elem_label: None,
        }
    } else {
        Change::Remove {
            field,
            label,
            elem,
            elem_label: None,
        }
    }
}

#[test]
fn a_record_that_cannot_say_what_it_means_is_refused() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    let refusal = |store: &mut Store, record: Value| match import(store, &[record]) {
        Err(StoreError::RecordRefused { cause, .. }) => cause.to_string(),
        other => panic!("{other:?}"),
    };
    let mut base = records()[1].clone();
    base["source"] = json!("beads:z");
    for (change, field) in [
        (json!({"fields": {"tags": "x"}}), "tags"),
        (json!({"fields": {"tags": [1]}}), "tags"),
        (json!({"fields": {"title": "shadow"}}), "title"),
        (json!({"fields": {"waiting_on": "kim"}}), "waiting_on"),
        (json!({"labels": ["Backend", "backend"]}), "labels"),
        (
            json!({"links": [{"type": "a/b", "target": "beads:z"}]}),
            "links",
        ),
    ] {
        let mut record = base.clone();
        for (key, value) in change.as_object().unwrap() {
            record[key] = value.clone();
        }
        let reason = refusal(&mut store, record);
        assert!(reason.starts_with(field), "{field}: {reason}");
    }

    let mut repeated = base;
    repeated["comments"] = json!([
        {"id": "c1", "actor": "ana", "body": "one"},
        {"id": "c1", "actor": "ana", "body": "two"},
    ]);
    let err = import(&mut store, &[repeated]).unwrap_err();
    assert!(
        matches!(&err, StoreError::InvalidExport { reason, .. } if reason.contains("c1 appears twice")),
        "{err}"
    );
}

/// The merge rule is not transitive across acknowledged writes, so the base
/// is folded in replay order: (4,3,300), (5,4,100), (6,3,200) as server seq,
/// base seq and clock leave the third, while folding them in another order
/// leaves the second.
#[test]
fn the_base_is_folded_in_replay_order() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let source = format!("tm:{issue}");
    let acked = |store: &Store, change: Change, id, (server_seq, base_seq, ms), source| {
        let mut op = remote_op(7, issue, &change, Hlc::new(ms, 0), base_seq);
        op.op_id = Uuid::from_u128(id);
        op.source = source;
        insert_acked(store, &op, server_seq, 10_000_000);
    };
    acked(&store, Change::Create, 1, (1, 0, 10), None);
    acked(
        &store,
        set_field("type", json!("task")),
        2,
        (2, 1, 20),
        None,
    );
    acked(
        &store,
        set_field("status", json!("todo")),
        3,
        (3, 2, 30),
        None,
    );
    let writes = [
        (20, (4, 3, 300), "a"),
        (10, (5, 4, 100), "b"),
        (30, (6, 3, 200), "c"),
    ];
    for (id, version, title) in writes {
        let change = set_field("title", json!(title));
        acked(&store, change, id, version, Some(source.clone()));
    }
    store.rebuild().unwrap();
    assert_eq!(field(&store, issue, "title"), Some(json!("c")));
    let mut record = records()[1].clone();
    record["source"] = json!(source);
    record["title"] = json!("c");
    record["type"] = json!("task");
    let imported = import(&mut store, &[record]).unwrap();
    let outcome = &imported.records[0];
    assert_eq!(
        (outcome.written.len(), outcome.conflicts.len()),
        (0, 0),
        "the source still says c: {outcome:?}"
    );
}

#[test]
fn what_tm_added_since_an_older_export_is_a_conflict() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_000));
    let (issue, other) = (new_issue(&mut store), new_issue(&mut store));
    let mut out = Vec::new();
    store.export(&mut out, 1).unwrap();
    store.append(draft(issue, label("new", true))).unwrap();
    store.link(&jace(), "blocks", issue, other).unwrap();
    let before = ops(&store);

    let imported = store.import_records(&mut out.as_slice(), &jace()).unwrap();
    assert_eq!(ops(&store), before, "nothing written");
    let fields: Vec<&str> = imported.records[0]
        .conflicts
        .iter()
        .map(|conflict| conflict.field.as_str())
        .collect();
    assert_eq!(
        fields,
        ["labels/new".to_owned(), format!("links/blocks/{other}")]
    );
}

#[test]
fn a_parent_reversed_between_two_records_moves() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    let first = import(&mut store, &records()).unwrap();
    let (child, parent) = (issue(&first, "beads:a.1"), issue(&first, "beads:a"));
    let mut reversed = records();
    reversed[0].as_object_mut().unwrap().remove("parent");
    reversed[0].as_object_mut().unwrap().remove("links");
    reversed[1]["parent"] = json!("beads:a.1");
    reversed[1]["links"] = json!([{"type": "blocks", "target": "beads:a.1"}]);
    // The new parent's record first, so attaching before detaching would
    // close a cycle.
    reversed.reverse();
    import(&mut store, &reversed).unwrap();
    assert_eq!(field(&store, child, "parent"), None);
    assert_eq!(
        field(&store, parent, "parent"),
        Some(json!(child.to_string()))
    );
    assert_eq!(store.links(parent).unwrap()[0].other, child);
}

/// x, then y under x and z alone; then x under y and y under z, x's record
/// first: y must let go of x before x can go under y.
#[test]
fn a_parent_moved_to_another_issue_lets_go_first() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    let record = |source: &str, parent: Option<&str>| {
        let mut record = json!({
            "_type": "issue", "source": source, "project": "CPL", "type": "task",
            "title": source, "status": "todo",
        });
        if let Some(parent) = parent {
            record["parent"] = json!(parent);
        }
        record
    };
    let first = import(
        &mut store,
        &[
            record("b:x", None),
            record("b:y", Some("b:x")),
            record("b:z", None),
        ],
    )
    .unwrap();
    import(
        &mut store,
        &[
            record("b:x", Some("b:y")),
            record("b:y", Some("b:z")),
            record("b:z", None),
        ],
    )
    .unwrap();
    let id = |source| issue(&first, source).to_string();
    assert_eq!(
        field(&store, issue(&first, "b:x"), "parent"),
        Some(json!(id("b:y")))
    );
    assert_eq!(
        field(&store, issue(&first, "b:y"), "parent"),
        Some(json!(id("b:z")))
    );
}

#[test]
fn a_comment_back_at_the_source_is_shown_again_unless_tm_hid_it() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    let first = import(&mut store, &records()).unwrap();
    let child = issue(&first, "beads:a.1");
    let mut without = records();
    without[0]["comments"] = json!([]);
    import(&mut store, &without).unwrap();
    assert!(store.comments(child).unwrap()[0].retracted);

    let back = import(&mut store, &records()).unwrap();
    assert_eq!(back.records[0].written, ["comments/c1"]);
    let comment = store.comments(child).unwrap()[0].clone();
    assert!(!comment.retracted, "the import hid it");
    let gone_again = import(&mut store, &without).unwrap();
    assert_eq!(gone_again.records[0].removed, ["comments/c1"]);
    import(&mut store, &records()).unwrap();
    assert!(!store.comments(child).unwrap()[0].retracted);

    store
        .append(tm_store::Draft {
            actor: tm_core::Actor::agent("claude", "ana"),
            ..draft(
                child,
                Change::Retract {
                    comment: comment.id,
                    retracted: true,
                },
            )
        })
        .unwrap();
    let again = import(&mut store, &records()).unwrap();
    assert_eq!(again.records[0].written, Vec::<String>::new());
    assert_eq!(
        again.records[0].conflicts,
        [],
        "tm's change alone keeps, as a field's does"
    );
    assert!(store.comments(child).unwrap()[0].retracted, "tm hid it");
}

#[test]
fn a_mistake_in_any_record_is_refused_before_anything_is_written() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    let before = ops(&store);
    let mut lines = records();
    lines[1]["links"] = json!([{"type": "dupe", "target": "beads:a.1"}]);
    let err = import(&mut store, &lines).unwrap_err();
    assert!(
        matches!(&err, StoreError::RecordRefused { line: 2, .. }),
        "{err}"
    );
    assert_eq!(ops(&store), before);
}

#[test]
fn a_number_reads_the_same_however_it_was_written() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    let mut record = records()[1].clone();
    record["fields"] = json!({"area": 3.0});
    import(&mut store, &[record.clone()]).unwrap();
    record["fields"] = json!({"area": 4.0});
    let again = import(&mut store, &[record]).unwrap();
    assert_eq!(again.records[0].written, ["area"]);
    assert_eq!(again.records[0].conflicts, []);
}

#[test]
fn a_link_both_records_list_stays_while_either_lists_it() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    let mut lines = records();
    lines[0]["links"] = json!([{"type": "relates_to", "target": "beads:a"}]);
    lines[1]["links"] = json!([{"type": "relates_to", "target": "beads:a.1"}]);
    let first = import(&mut store, &lines).unwrap();
    let child = issue(&first, "beads:a.1");
    lines[0].as_object_mut().unwrap().remove("links");
    let second = import(&mut store, &lines).unwrap();
    assert!(
        second
            .records
            .iter()
            .all(|record| record.removed.is_empty())
    );
    assert_eq!(store.links(child).unwrap().len(), 1);
}

#[test]
fn a_comment_gone_from_the_source_is_retracted_once() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    import(&mut store, &records()).unwrap();
    let mut changed = records();
    changed[0]["comments"] = json!([
        {"id": "c2", "actor": "ana", "body": "second"},
    ]);
    let second = import(&mut store, &changed).unwrap();
    let child = second.records[0].issue;
    assert_eq!(second.records[0].written, ["comments/c2"]);
    assert_eq!(second.records[0].removed, ["comments/c1"]);
    let comments = store.comments(child).unwrap();
    assert_eq!(
        comments
            .iter()
            .map(|comment| (comment.body.as_str(), comment.retracted))
            .collect::<Vec<_>>(),
        [("first", true), ("second", false)]
    );

    // Shown again in tm, it stays shown.
    let first = comments[0].id;
    let retraction: Uuid = store
        .connection()
        .query_row(
            "SELECT b.batch FROM ops o JOIN op_batches b ON b.op_id = o.op_id
             WHERE o.kind = 'retract'",
            [],
            |row| {
                row.get::<_, Vec<u8>>(0)
                    .map(|id| Uuid::from_slice(&id).unwrap())
            },
        )
        .unwrap();
    store.undo(&jace(), retraction).unwrap_err();
    store
        .append(tm_store::Draft {
            actor: tm_core::Actor::agent("claude", "ana"),
            ..draft(
                child,
                Change::Retract {
                    comment: first,
                    retracted: false,
                },
            )
        })
        .unwrap();
    changed[0]["comments"][0]["body"] = json!("second, edited");
    let third = import(&mut store, &changed).unwrap();
    assert!(
        third.records[0].written.is_empty(),
        "{:?}",
        third.records[0]
    );
    assert_eq!(third.records[0].changed_comments, ["c2"]);
    assert!(!store.comments(child).unwrap()[0].retracted);
}

#[test]
fn a_project_line_makes_its_project() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    let mut lines = vec![json!({"_type": "project", "key": "tm", "name": "Task management"})];
    let mut record = records()[1].clone();
    record["project"] = json!("TM");
    lines.push(record);
    let imported = import(&mut store, &lines).unwrap();
    assert_eq!(imported.projects, ["TM"]);
    let view = store.issue(imported.records[0].issue).unwrap().unwrap();
    assert_eq!(view.key.as_deref(), Some("TM-1"));
    assert_eq!(
        import(&mut store, &lines).unwrap().projects,
        Vec::<String>::new()
    );
}

#[test]
fn a_refusal_after_a_new_project_names_its_batch() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    let lines = [
        json!({"_type": "project", "key": "AA", "name": "A"}),
        json!({"_type": "project", "key": "1!", "name": "bad"}),
    ];
    let err = import(&mut store, &lines).unwrap_err();
    let StoreError::RecordRefused { line, written, .. } = &err else {
        panic!("{err}");
    };
    assert_eq!((*line, written.len()), (2, 1), "{err}");
    assert!(store.project_by_key("AA").unwrap().is_some());
}

#[test]
fn another_operation_format_is_read_for_its_records() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    let mut lines = vec![
        json!({"_type": "header", "format": "tm-export", "version": 1, "op_format": 99,
               "workspace": Uuid::now_v7(), "exported_ms": 1}),
        json!({"_type": "op", "shape": "whatever format 99 writes"}),
    ];
    lines.extend(records());
    let imported = import(&mut store, &lines).unwrap();
    assert_eq!(imported.records.len(), 2);
    assert_eq!(imported.records[0].line, 3);
}

#[test]
fn a_file_that_is_not_records_is_refused_before_anything_is_written() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    let before = ops(&store);
    let mut twice = records();
    twice.push(records()[1].clone());
    let mut blank = records();
    blank[1]["source"] = json!(" ");
    let mut typo = records();
    typo[1]["asignee"] = json!("ana");
    for (lines, reason) in [
        (twice, "also line 2"),
        (blank, "blank"),
        (typo, "asignee"),
        (vec![json!({"_type": "memory"})], "not a record type"),
    ] {
        let err = import(&mut store, &lines).unwrap_err();
        assert!(
            matches!(&err, StoreError::InvalidExport { reason: r, .. } if r.contains(reason)),
            "{reason}: {err}"
        );
    }
    assert_eq!(ops(&store), before);
}

#[test]
fn a_refused_record_names_its_line_and_keeps_what_came_before() {
    let dir = TempDir::new();
    let mut store = store(&dir, &ManualClock::at(1_900_000_000_000));
    let mut lines = records();
    lines[0]["parent"] = json!("beads:missing");
    let err = import(&mut store, &lines).unwrap_err();
    let StoreError::RecordRefused {
        line,
        record,
        cause,
        written,
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!((*line, record.as_str()), (1, "beads:a.1"));
    assert_eq!(written.len(), 2, "both records' first batches stay: {err}");
    for batch in written {
        assert!(store.can_undo(*batch).unwrap(), "{batch}");
    }
    assert!(
        matches!(&**cause, StoreError::Invalid { field, .. } if field == "parent"),
        "{cause}"
    );
    assert_eq!(
        store
            .find("", tm_core::filter::Context::default())
            .unwrap()
            .len(),
        2
    );

    let mut untyped = records();
    untyped[1].as_object_mut().unwrap().remove("type");
    untyped[1]["source"] = json!("beads:b");
    untyped[1].as_object_mut().unwrap().remove("status");
    let err = import(&mut store, &untyped[1..]).unwrap_err();
    assert!(
        matches!(&err, StoreError::RecordRefused { cause, .. } if matches!(**cause, StoreError::MissingFields { .. })),
        "the ordinary write checks hold: {err}"
    );
}
