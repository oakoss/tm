//! Short-list records (ADR 0016): their writes, merges and undo.

use serde_json::json;
use tm_core::shortlist::Field;
use tm_core::{Change, Hlc, OpError};
use tm_store::{Draft, Shortlist, ShortlistEdit, Store, StoreError, Target};
use uuid::Uuid;

use crate::support::{
    ManualClock, OTHER, PROJECT, TempDir, dump, insert_acked, jace, open, remote_op,
};

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

fn create(store: &mut Store, name: &str, actors: &[&str]) -> Uuid {
    store
        .create_shortlist(&jace(), PROJECT, name, &names(actors), None)
        .unwrap()
        .0
}

fn read(store: &Store, id: Uuid) -> Shortlist {
    store.shortlist(id).unwrap().unwrap()
}

fn batch(ops: &[tm_core::Op]) -> Uuid {
    ops[0].op_id
}

fn is_invalid(err: &StoreError, field: &str) -> bool {
    matches!(err, StoreError::Invalid { field: f, .. } if f == field)
}

#[test]
fn a_list_reads_back_as_written() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let actors = names(&["Jace", "claude"]);
    let (id, _) = store
        .create_shortlist(&jace(), PROJECT, "release-0.2", &actors, Some(5))
        .unwrap();
    let docs = create(&mut store, "docs", &["codex"]);
    let list = read(&store, id);
    assert_eq!(list.name.as_deref(), Some("release-0.2"));
    assert_eq!(list.size, 5);
    assert_eq!(list.actors, ["claude", "jace"]);
    assert!(!list.deleted);
    assert_eq!(read(&store, docs).size, tm_core::shortlist::DEFAULT_SIZE);
    let listed = store.shortlists(PROJECT).unwrap();
    let listed: Vec<Uuid> = listed.iter().map(|list| list.id).collect();
    assert_eq!(listed, [id, docs], "oldest first");
    assert_eq!(store.shortlists(OTHER).unwrap(), []);
}

#[test]
fn names_are_unique_in_a_project_without_case() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = create(&mut store, "docs", &["codex"]);
    let err = store
        .create_shortlist(&jace(), PROJECT, "Docs", &names(&["codex"]), None)
        .unwrap_err();
    assert!(is_invalid(&err, "name"), "{err}");
    let other = create(&mut store, "other", &["codex"]);
    let rename = ShortlistEdit {
        name: Some("DOCS"),
        ..ShortlistEdit::default()
    };
    let err = store.edit_shortlist(&jace(), other, &rename).unwrap_err();
    assert!(is_invalid(&err, "name"), "{err}");
    store
        .create_shortlist(&jace(), OTHER, "docs", &names(&["codex"]), None)
        .unwrap();
    store.delete_shortlist(&jace(), docs).unwrap();
    create(&mut store, "docs", &["codex"]);
}

#[test]
fn malformed_names_and_sizes_are_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    for (name, size) in [("my list", None), ("docs", Some(0)), ("docs", Some(1001))] {
        let err = store
            .create_shortlist(&jace(), PROJECT, name, &names(&["codex"]), size)
            .unwrap_err();
        assert!(
            matches!(err, StoreError::Op(OpError::InvalidShortlist { .. })),
            "{name} {size:?}: {err}"
        );
    }
    assert_eq!(store.shortlists(PROJECT).unwrap(), []);
}

#[test]
fn a_list_keeps_at_least_one_actor() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let err = store
        .create_shortlist(&jace(), PROJECT, "docs", &[], None)
        .unwrap_err();
    assert!(is_invalid(&err, "actors"), "{err}");
    let id = create(&mut store, "docs", &["codex"]);
    let remove = names(&["Codex"]);
    let drop_last = ShortlistEdit {
        remove: &remove,
        ..ShortlistEdit::default()
    };
    let err = store.edit_shortlist(&jace(), id, &drop_last).unwrap_err();
    assert!(is_invalid(&err, "actors"), "{err}");
    let add = names(&["claude"]);
    let swap = ShortlistEdit {
        add: &add,
        remove: &remove,
        ..ShortlistEdit::default()
    };
    store.edit_shortlist(&jace(), id, &swap).unwrap();
    assert_eq!(read(&store, id).actors, ["claude"]);
}

#[test]
fn a_deleted_list_is_kept_and_an_undo_restores_it() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let id = create(&mut store, "docs", &["codex"]);
    let deleted = store.delete_shortlist(&jace(), id).unwrap();
    assert!(read(&store, id).deleted);
    assert_eq!(store.shortlists(PROJECT).unwrap(), []);
    let rename = ShortlistEdit {
        name: Some("later"),
        ..ShortlistEdit::default()
    };
    let err = store.edit_shortlist(&jace(), id, &rename).unwrap_err();
    assert!(matches!(err, StoreError::UnknownShortlist(_)), "{err}");
    let err = store.delete_shortlist(&jace(), id).unwrap_err();
    assert!(matches!(err, StoreError::UnknownShortlist(_)), "{err}");

    let (plan, _) = store.undo(&jace(), batch(&deleted)).unwrap();
    assert_eq!(
        plan.restores[0].target,
        Target::Shortlist {
            shortlist: id,
            name: Some("docs".into())
        }
    );
    let list = read(&store, id);
    assert!(!list.deleted);
    assert_eq!(list.name.as_deref(), Some("docs"));
}

#[test]
fn restoring_a_list_whose_name_was_taken_since_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let id = create(&mut store, "docs", &["codex"]);
    let deleted = store.delete_shortlist(&jace(), id).unwrap();
    create(&mut store, "docs", &["claude"]);
    let err = store.undo(&jace(), batch(&deleted)).unwrap_err();
    assert!(is_invalid(&err, "name"), "{err}");
    assert!(read(&store, id).deleted);
}

#[test]
fn undoing_a_create_deletes_the_list() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (id, ops) = store
        .create_shortlist(&jace(), PROJECT, "docs", &names(&["codex"]), None)
        .unwrap();
    store.undo(&jace(), batch(&ops)).unwrap();
    assert!(read(&store, id).deleted);
}

#[test]
fn undoing_an_edit_restores_name_size_and_actors() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let id = create(&mut store, "docs", &["codex"]);
    let (add, remove) = (names(&["claude"]), names(&["codex"]));
    let edit = ShortlistEdit {
        name: Some("guides"),
        size: Some(3),
        add: &add,
        remove: &remove,
    };
    let ops = store.edit_shortlist(&jace(), id, &edit).unwrap();
    let after = read(&store, id);
    assert_eq!((after.name.as_deref(), after.size), (Some("guides"), 3));
    assert_eq!(after.actors, ["claude"]);
    store.undo(&jace(), batch(&ops)).unwrap();
    let back = read(&store, id);
    assert_eq!((back.name.as_deref(), back.size), (Some("docs"), 10));
    assert_eq!(back.actors, ["codex"]);
}

#[test]
fn a_list_merges_field_by_field() {
    let dir = TempDir::new();
    let store = open(&dir.db(), &ManualClock::at(1_000));
    let id = Uuid::now_v7();
    let name = |name: &str| set(Field::Name, json!(name));
    let size = set(Field::Size, json!(4));
    let actor = |name: &str, present| Change::ShortlistActor {
        actor: name.into(),
        present,
    };
    // Two Replicas edit one list at once: each field keeps its later write,
    // and an Actor one adds survives the other's unrelated removal.
    let ops = [
        remote_op(7, id, &Change::CreateShortlist, Hlc::new(1_000, 0), 0),
        remote_op(7, id, &name("docs"), Hlc::new(1_000, 1), 0),
        remote_op(7, id, &actor("codex", true), Hlc::new(1_000, 2), 0),
        remote_op(7, id, &name("guides"), Hlc::new(3_000, 0), 3),
        remote_op(8, id, &size, Hlc::new(2_000, 0), 3),
        remote_op(8, id, &name("manuals"), Hlc::new(2_000, 1), 3),
        remote_op(8, id, &actor("claude", true), Hlc::new(2_000, 2), 3),
        remote_op(7, id, &actor("codex", false), Hlc::new(3_000, 1), 3),
    ];
    for (seq, op) in (1..).zip(&ops) {
        insert_acked(&store, op, seq + 2, 3_000);
    }
    let mut store = store;
    assert_eq!(store.rebuild().unwrap(), 0);
    let list = read(&store, id);
    assert_eq!((list.name.as_deref(), list.size), (Some("guides"), 4));
    assert_eq!(list.actors, ["claude"]);
}

#[test]
fn a_merge_may_leave_two_lists_one_name() {
    let dir = TempDir::new();
    let store = open(&dir.db(), &ManualClock::at(1_000));
    let name = set(Field::Name, json!("docs"));
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    let ops = [
        remote_op(7, a, &Change::CreateShortlist, Hlc::new(1_000, 0), 0),
        remote_op(7, a, &name, Hlc::new(1_000, 1), 0),
        remote_op(8, b, &Change::CreateShortlist, Hlc::new(1_000, 0), 0),
        remote_op(8, b, &name, Hlc::new(1_000, 1), 0),
    ];
    for (seq, op) in (1..).zip(&ops) {
        insert_acked(&store, op, seq + 2, 1_000);
    }
    let mut store = store;
    assert_eq!(store.rebuild().unwrap(), 0);
    let listed = store.shortlists(PROJECT).unwrap();
    assert_eq!(listed.len(), 2);
    let docs = Some("docs");
    assert!(listed.iter().all(|list| list.name.as_deref() == docs));
}

#[test]
fn list_writes_replay_to_the_same_tables() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let id = create(&mut store, "docs", &["codex", "claude"]);
    let remove = names(&["codex"]);
    let edit = ShortlistEdit {
        name: Some("guides"),
        remove: &remove,
        ..ShortlistEdit::default()
    };
    store.edit_shortlist(&jace(), id, &edit).unwrap();
    let deleted = store.delete_shortlist(&jace(), id).unwrap();
    store.undo(&jace(), batch(&deleted)).unwrap();
    let incremental = dump(&store);
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(dump(&store), incremental);
}

fn set(field: Field, value: serde_json::Value) -> Change {
    Change::SetShortlist { field, value }
}

fn list_draft(id: Uuid, change: Change) -> Draft {
    Draft {
        actor: jace(),
        project: PROJECT,
        subject: id,
        change,
    }
}

#[test]
fn list_operations_replay_cannot_apply_are_recorded_not_fatal() {
    let dir = TempDir::new();
    let store = open(&dir.db(), &ManualClock::at(1_000));
    let (stray, twice, there) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (resize, create) = (set(Field::Size, json!(3)), Change::CreateShortlist);
    let mut moved = remote_op(7, there, &resize, Hlc::new(1_000, 4), 0);
    moved.project = OTHER;
    let ops = [
        remote_op(7, stray, &resize, Hlc::new(1_000, 0), 0),
        remote_op(7, twice, &create, Hlc::new(1_000, 1), 0),
        remote_op(8, twice, &create, Hlc::new(1_000, 2), 0),
        remote_op(7, there, &create, Hlc::new(1_000, 3), 0),
        moved,
    ];
    for (seq, op) in (1..).zip(&ops) {
        insert_acked(&store, op, seq + 2, 1_000);
    }
    let mut store = store;
    // The stray write, the second create and the write filed elsewhere.
    assert_eq!(store.rebuild().unwrap(), 3);
    let reasons: Vec<String> = store
        .unapplied()
        .unwrap()
        .into_iter()
        .map(|u| u.reason)
        .collect();
    assert!(reasons[2].starts_with("Short-list"), "{reasons:?}");
    assert_eq!(read(&store, there).size, 10);
}

#[test]
fn a_batch_cannot_create_a_list_without_a_name_or_an_actor() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let id = Uuid::now_v7();
    let err = store
        .append_all(vec![list_draft(id, Change::CreateShortlist)])
        .unwrap_err();
    assert!(is_invalid(&err, "name"), "{err}");
    let named = vec![
        list_draft(id, Change::CreateShortlist),
        list_draft(id, set(Field::Name, json!("docs"))),
    ];
    let err = store.append_all(named).unwrap_err();
    assert!(is_invalid(&err, "actors"), "{err}");
    assert_eq!(store.shortlist(id).unwrap(), None);
}

#[test]
fn actors_are_filed_in_one_spelling() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let id = create(&mut store, "docs", &[" Claude "]);
    let add = names(&["CLAUDE", "codex"]);
    let edit = ShortlistEdit {
        add: &add,
        ..ShortlistEdit::default()
    };
    store.edit_shortlist(&jace(), id, &edit).unwrap();
    assert_eq!(read(&store, id).actors, ["claude", "codex"]);
    let raw = Change::ShortlistActor {
        actor: "Codex".into(),
        present: true,
    };
    let err = store.append(list_draft(id, raw)).unwrap_err();
    assert!(
        matches!(
            err,
            StoreError::Op(OpError::InvalidShortlist { field: "actor", .. })
        ),
        "{err}"
    );
}

#[test]
fn a_list_renamed_to_its_own_name_in_other_case_keeps_it() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let id = create(&mut store, "docs", &["codex"]);
    let rename = ShortlistEdit {
        name: Some("Docs"),
        ..ShortlistEdit::default()
    };
    store.edit_shortlist(&jace(), id, &rename).unwrap();
    assert_eq!(read(&store, id).name.as_deref(), Some("Docs"));
}

#[test]
fn a_list_merged_without_name_or_size_can_still_be_edited() {
    let dir = TempDir::new();
    let store = open(&dir.db(), &ManualClock::at(1_000));
    let id = Uuid::now_v7();
    let actor = Change::ShortlistActor {
        actor: "codex".into(),
        present: true,
    };
    let created = remote_op(7, id, &Change::CreateShortlist, Hlc::new(1_000, 0), 0);
    insert_acked(&store, &created, 3, 1_000);
    let joined = remote_op(7, id, &actor, Hlc::new(1_000, 1), 0);
    insert_acked(&store, &joined, 4, 1_000);
    let mut store = store;
    store.rebuild().unwrap();
    assert_eq!(read(&store, id).name, None);

    let resize = ShortlistEdit {
        size: Some(3),
        ..ShortlistEdit::default()
    };
    let resized = store.edit_shortlist(&jace(), id, &resize).unwrap();
    store.undo(&jace(), batch(&resized)).unwrap();
    // A size never set restores to the default.
    assert_eq!(read(&store, id).size, 10);

    let rename = ShortlistEdit {
        name: Some("docs"),
        ..ShortlistEdit::default()
    };
    let renamed = store.edit_shortlist(&jace(), id, &rename).unwrap();
    assert_eq!(read(&store, id).name.as_deref(), Some("docs"));
    let err = store.undo(&jace(), batch(&renamed)).unwrap_err();
    assert!(matches!(err, StoreError::CannotUndo { .. }), "{err}");
}

#[test]
fn undoing_an_edit_to_a_list_deleted_since_asks_for_the_delete_undone() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let id = create(&mut store, "docs", &["codex"]);
    let resize = ShortlistEdit {
        size: Some(3),
        ..ShortlistEdit::default()
    };
    let resized = store.edit_shortlist(&jace(), id, &resize).unwrap();
    store.delete_shortlist(&jace(), id).unwrap();
    let err = store.undo(&jace(), batch(&resized)).unwrap_err();
    assert!(
        matches!(&err, StoreError::CannotUndo { reason, .. } if reason.contains("undo its delete")),
        "{err}"
    );
    assert!(!store.can_undo(batch(&resized)).unwrap());
}

#[test]
fn undoing_a_batch_that_removed_an_actor_and_deleted_the_list_restores_both() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let id = create(&mut store, "docs", &["codex", "claude"]);
    let remove = Change::ShortlistActor {
        actor: "codex".into(),
        present: false,
    };
    let ops = store
        .append_all(vec![
            list_draft(id, remove),
            list_draft(id, set(Field::Deleted, json!(true))),
        ])
        .unwrap();
    store.undo(&jace(), batch(&ops)).unwrap();
    let list = read(&store, id);
    assert!(!list.deleted);
    assert_eq!(list.actors, ["claude", "codex"]);
}

#[test]
fn an_undo_of_a_rename_and_delete_can_itself_be_undone() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let id = create(&mut store, "docs", &["codex"]);
    let ops = store
        .append_all(vec![
            list_draft(id, set(Field::Name, json!("guides"))),
            list_draft(id, set(Field::Deleted, json!(true))),
        ])
        .unwrap();
    let (_, undone) = store.undo(&jace(), batch(&ops)).unwrap();
    assert!(store.can_undo(batch(&undone)).unwrap());
    store.undo(&jace(), batch(&undone)).unwrap();
    let list = read(&store, id);
    assert_eq!((list.name.as_deref(), list.deleted), (Some("guides"), true));
}

/// Names are checked on the batch's end state, so undo may move a name from
/// one list to another in whatever order its writes fall.
#[test]
fn names_moved_between_lists_undo_in_either_direction() {
    for older_takes_the_name in [true, false] {
        let dir = TempDir::new();
        let mut store = open(&dir.db(), &ManualClock::at(1_000));
        let older = create(&mut store, "x", &["codex"]);
        let newer = create(&mut store, "y", &["codex"]);
        let (giver, taker) = if older_takes_the_name {
            (newer, older)
        } else {
            (older, newer)
        };
        let moved = read(&store, giver).name.unwrap();
        let ops = store
            .append_all(vec![
                list_draft(giver, set(Field::Name, json!("z"))),
                list_draft(taker, set(Field::Name, json!(moved))),
            ])
            .unwrap();
        let (_, undone) = store.undo(&jace(), batch(&ops)).unwrap();
        assert_eq!(read(&store, giver).name, Some(moved.clone()));
        store.undo(&jace(), batch(&undone)).unwrap();
        assert_eq!(read(&store, taker).name, Some(moved));
        assert_eq!(read(&store, giver).name.as_deref(), Some("z"));
    }
}

#[test]
fn undo_may_hand_a_name_back_to_a_list_a_batch_created_it_for() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let a = create(&mut store, "docs", &["codex"]);
    let b = Uuid::now_v7();
    let ops = store
        .append_all(vec![
            list_draft(a, set(Field::Name, json!("guides"))),
            list_draft(b, Change::CreateShortlist),
            list_draft(b, set(Field::Name, json!("docs"))),
            list_draft(
                b,
                Change::ShortlistActor {
                    actor: "codex".into(),
                    present: true,
                },
            ),
        ])
        .unwrap();
    store.undo(&jace(), batch(&ops)).unwrap();
    assert_eq!(read(&store, a).name.as_deref(), Some("docs"));
    assert!(read(&store, b).deleted);
}

#[test]
fn restoring_a_list_checks_the_name_it_ends_with() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let a = create(&mut store, "docs", &["codex"]);
    let ops = store
        .append_all(vec![
            list_draft(a, set(Field::Name, json!("guides"))),
            list_draft(a, set(Field::Deleted, json!(true))),
        ])
        .unwrap();
    create(&mut store, "guides", &["claude"]);
    assert!(store.can_undo(batch(&ops)).unwrap());
    store.undo(&jace(), batch(&ops)).unwrap();
    let list = read(&store, a);
    assert_eq!((list.name.as_deref(), list.deleted), (Some("docs"), false));
}

#[test]
fn undoing_a_list_edit_written_over_since_is_a_conflict() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let id = create(&mut store, "docs", &["codex"]);
    let rename = |name| ShortlistEdit {
        name: Some(name),
        ..ShortlistEdit::default()
    };
    let first = store
        .edit_shortlist(&jace(), id, &rename("guides"))
        .unwrap();
    store
        .edit_shortlist(&jace(), id, &rename("manuals"))
        .unwrap();
    let err = store.undo(&jace(), batch(&first)).unwrap_err();
    assert!(matches!(err, StoreError::UndoConflict { .. }), "{err}");
    assert_eq!(read(&store, id).name.as_deref(), Some("manuals"));
}

#[test]
fn undoing_the_create_of_a_list_deleted_since_changes_nothing() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (id, created) = store
        .create_shortlist(&jace(), PROJECT, "docs", &names(&["codex"]), None)
        .unwrap();
    store.delete_shortlist(&jace(), id).unwrap();
    let err = store.undo(&jace(), batch(&created)).unwrap_err();
    assert!(
        matches!(&err, StoreError::CannotUndo { reason, .. } if reason.contains("already holds")),
        "{err}"
    );
}

#[test]
fn a_list_restore_names_the_list_not_an_issue() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let id = create(&mut store, "docs", &["codex"]);
    let deleted = store.delete_shortlist(&jace(), id).unwrap();
    let plan = store.undo_plan(&jace(), batch(&deleted)).unwrap();
    let json = serde_json::to_value(&plan.restores[0]).unwrap();
    assert_eq!(json["shortlist"], json!(id));
    assert_eq!(json["name"], json!("docs"));
    assert_eq!(json.get("issue"), None);
}
