//! Short-list membership on Issues (ADR 0016): the two fields, how they
//! read, and how finishing an Issue or deleting its list clears them.

use serde_json::{Value, json};
use tm_core::{Change, Hlc};
use tm_store::{Store, StoreError};
use uuid::Uuid;

use crate::support::{
    ManualClock, OTHER, PROJECT, TempDir, draft, field, insert_acked, jace, new_issue, open,
    remote_op, set_field,
};

fn list(store: &mut Store, name: &str) -> Uuid {
    store
        .create_shortlist(&jace(), PROJECT, name, &["codex".to_owned()], None)
        .unwrap()
        .0
}

/// Puts `issue` on `list` at `rank` in one batch, returning the batch.
fn join(store: &mut Store, issue: Uuid, list: Uuid, rank: &str) -> Uuid {
    store
        .append_all(vec![
            draft(issue, set_field("shortlist", json!(list.to_string()))),
            draft(issue, set_field("shortlist_rank", json!(rank))),
        ])
        .unwrap()[0]
        .op_id
}

fn on(store: &Store, issue: Uuid) -> (Option<Value>, Option<Value>) {
    (
        field(store, issue, "shortlist"),
        field(store, issue, "shortlist_rank"),
    )
}

#[test]
fn members_read_in_rank_order_ties_by_id() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs");
    let [a, b, c] = [(); 3].map(|()| new_issue(&mut store));
    join(&mut store, a, docs, "k");
    join(&mut store, b, docs, "V");
    join(&mut store, c, docs, "V");
    let members = store.shortlist_members(docs).unwrap();
    assert_eq!(members, [b.min(c), b.max(c), a]);
    let held = (Some(json!(docs.to_string())), Some(json!("k")));
    assert_eq!(on(&store, a), held);
}

#[test]
fn a_list_and_a_rank_are_held_together_or_not_at_all() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs");
    let issue = new_issue(&mut store);
    let alone = set_field("shortlist", json!(docs.to_string()));
    let err = store.append(draft(issue, alone)).unwrap_err();
    assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
    let err = store
        .append(draft(issue, set_field("shortlist_rank", json!("V"))))
        .unwrap_err();
    assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
    assert_eq!(on(&store, issue), (None, None));
}

#[test]
fn membership_names_a_live_list_in_the_issues_project_and_a_rank() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let (elsewhere, _) = store
        .create_shortlist(&jace(), OTHER, "ops", &["codex".to_owned()], None)
        .unwrap();
    let gone = list(&mut store, "gone");
    store.delete_shortlist(&jace(), gone).unwrap();
    let docs = list(&mut store, "docs");
    let attempts = [
        (elsewhere.to_string(), "V"),
        (gone.to_string(), "V"),
        (Uuid::now_v7().to_string(), "V"),
        ("not-an-id".to_owned(), "V"),
        (docs.to_string(), "V0"),
    ];
    for (list, rank) in attempts {
        let err = store
            .append_all(vec![
                draft(issue, set_field("shortlist", json!(list))),
                draft(issue, set_field("shortlist_rank", json!(rank))),
            ])
            .unwrap_err();
        assert!(
            matches!(
                err,
                StoreError::Invalid { .. } | StoreError::UnknownShortlist(_)
            ),
            "{list} {rank}: {err}"
        );
    }
    assert_eq!(on(&store, issue), (None, None));
}

#[test]
fn finishing_an_issue_takes_it_off_its_list_and_undo_puts_it_back() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs");
    for status in ["done", "cancelled"] {
        let issue = new_issue(&mut store);
        join(&mut store, issue, docs, "V");
        let ops = store
            .append(draft(issue, set_field("status", json!(status))))
            .map(|op| op.op_id)
            .unwrap();
        assert_eq!(on(&store, issue), (None, None), "{status}");
        store.undo(&jace(), ops).unwrap();
        assert_eq!(
            on(&store, issue),
            (Some(json!(docs.to_string())), Some(json!("V")))
        );
    }
    let issue = new_issue(&mut store);
    join(&mut store, issue, docs, "k");
    store
        .append(draft(issue, set_field("status", json!("in_progress"))))
        .unwrap();
    assert_eq!(on(&store, issue).0, Some(json!(docs.to_string())));
}

#[test]
fn deleting_a_list_takes_its_members_off_and_undo_restores_both() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs");
    let (a, b) = (new_issue(&mut store), new_issue(&mut store));
    join(&mut store, a, docs, "V");
    join(&mut store, b, docs, "k");
    let deleted = store.delete_shortlist(&jace(), docs).unwrap()[0].op_id;
    // Stored, not only read, so a restore by any other path finds none.
    let stored: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM issues WHERE shortlist IS NOT NULL OR shortlist_rank IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, 0);
    store.undo(&jace(), deleted).unwrap();
    assert_eq!(store.shortlist_members(docs).unwrap(), [a, b]);
}

#[test]
fn a_membership_naming_a_list_no_longer_live_reads_as_none() {
    let dir = TempDir::new();
    let store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let unknown = Uuid::now_v7();
    // Another Replica's writes: an Issue joined to a list this store never
    // saw created, as a merge with a concurrent delete can leave it.
    let ops = [
        remote_op(7, issue, &Change::Create, Hlc::new(1_000, 0), 0),
        remote_op(
            7,
            issue,
            &set_field("shortlist", json!(unknown.to_string())),
            Hlc::new(1_000, 1),
            0,
        ),
        remote_op(
            7,
            issue,
            &set_field("shortlist_rank", json!("V")),
            Hlc::new(1_000, 2),
            0,
        ),
    ];
    for (seq, op) in (1..).zip(&ops) {
        insert_acked(&store, op, seq + 2, 1_000);
    }
    let mut store = store;
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(on(&store, issue), (None, None));
    let none: [Uuid; 0] = [];
    assert_eq!(store.shortlist_members(unknown).unwrap(), none);
}

/// A merge can join an Issue to a list another Replica deleted, or to a list
/// in another Project; neither reads as a membership.
#[test]
fn a_merged_membership_of_a_deleted_or_foreign_list_reads_as_none() {
    let (issue, deleted, foreign) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let name = |text: &str| Change::SetShortlist {
        field: tm_core::shortlist::Field::Name,
        value: json!(text),
    };
    let tombstone = Change::SetShortlist {
        field: tm_core::shortlist::Field::Deleted,
        value: json!(true),
    };
    let mut foreign_ops = [
        remote_op(8, foreign, &Change::CreateShortlist, Hlc::new(1_000, 0), 0),
        remote_op(8, foreign, &name("ops"), Hlc::new(1_000, 1), 0),
    ];
    for op in &mut foreign_ops {
        op.project = OTHER;
    }
    let mut ops = vec![
        remote_op(8, deleted, &Change::CreateShortlist, Hlc::new(1_000, 2), 0),
        remote_op(8, deleted, &name("docs"), Hlc::new(1_000, 3), 0),
        remote_op(8, deleted, &tombstone, Hlc::new(2_000, 0), 0),
        remote_op(7, issue, &Change::Create, Hlc::new(1_000, 4), 0),
    ];
    ops.extend(foreign_ops);
    let rank = set_field("shortlist_rank", json!("V"));
    for list in [deleted, foreign] {
        let joined = set_field("shortlist", json!(list.to_string()));
        let all: Vec<_> = ops
            .iter()
            .cloned()
            .chain([
                remote_op(7, issue, &joined, Hlc::new(1_500, 0), 0),
                remote_op(7, issue, &rank, Hlc::new(1_500, 1), 0),
            ])
            .collect();
        let dir = TempDir::new();
        let mut store = open(&dir.db(), &ManualClock::at(1_000));
        for (seq, op) in (1..).zip(&all) {
            insert_acked(&store, op, seq + 2, 2_000);
        }
        assert_eq!(store.rebuild().unwrap(), 0);
        assert_eq!(on(&store, issue), (None, None), "{list}");
        assert_eq!(store.shortlist_members(list).unwrap(), Vec::<Uuid>::new());
    }
}

#[test]
fn a_finished_issue_cannot_join_a_list() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs");
    let issue = new_issue(&mut store);
    store
        .append(draft(issue, set_field("status", json!("done"))))
        .unwrap();
    let err = store
        .append_all(vec![
            draft(issue, set_field("shortlist", json!(docs.to_string()))),
            draft(issue, set_field("shortlist_rank", json!("V"))),
        ])
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid { reason, .. } if reason.contains("done or cancelled")),
        "{err}"
    );
}

/// ADR 0016: an undo may restore a membership after its list was deleted;
/// readers hide it, and it shows again if the delete is undone too.
#[test]
fn a_finish_undone_after_its_list_was_deleted_restores_a_hidden_membership() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs");
    let issue = new_issue(&mut store);
    join(&mut store, issue, docs, "V");
    let finished = store
        .append(draft(issue, set_field("status", json!("done"))))
        .unwrap()
        .op_id;
    let deleted = store.delete_shortlist(&jace(), docs).unwrap()[0].op_id;
    store.undo(&jace(), finished).unwrap();
    assert_eq!(field(&store, issue, "status"), Some(json!("todo")));
    assert_eq!(on(&store, issue), (None, None));
    store.undo(&jace(), deleted).unwrap();
    assert_eq!(store.shortlist_members(docs).unwrap(), [issue]);
}

#[test]
fn an_issue_a_merge_left_half_on_a_list_stays_editable() {
    let dir = TempDir::new();
    let store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let mut store = store;
    let docs = list(&mut store, "docs");
    let ops = [
        remote_op(7, issue, &Change::Create, Hlc::new(1_000, 0), 0),
        remote_op(
            7,
            issue,
            &set_field("shortlist", json!(docs.to_string())),
            Hlc::new(1_000, 1),
            0,
        ),
    ];
    for (seq, op) in (1..).zip(&ops) {
        insert_acked(&store, op, seq + 10, 1_000);
    }
    store.rebuild().unwrap();
    assert_eq!(on(&store, issue), (None, None));
    assert_eq!(store.shortlist_members(docs).unwrap(), Vec::<Uuid>::new());
    store
        .append(draft(issue, set_field("title", json!("edited"))))
        .unwrap();

    // A batch that leaves a whole Issue half on a list is refused, naming it.
    let other = new_issue(&mut store);
    let alone = set_field("shortlist", json!(docs.to_string()));
    let err = store.append(draft(other, alone)).unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid { reason, .. } if reason.contains("of Issue CPL-")),
        "{err}"
    );

    // So is a write to the half-held Issue's membership that leaves it half.
    let moved = set_field("shortlist", json!(list(&mut store, "ops").to_string()));
    let err = store.append(draft(issue, moved)).unwrap_err();
    assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
}

#[test]
fn a_membership_cleared_by_finishing_is_written_by_whoever_finished() {
    for finish_first in [false, true] {
        let dir = TempDir::new();
        let mut store = open(&dir.db(), &ManualClock::at(1_000));
        let docs = list(&mut store, "docs");
        let issue = new_issue(&mut store);
        join(&mut store, issue, docs, "V");
        let mut edit = draft(issue, set_field("title", json!("edited")));
        edit.actor = crate::support::claude();
        let finish = draft(issue, set_field("status", json!("done")));
        let batch = if finish_first {
            vec![finish, edit]
        } else {
            vec![edit, finish]
        };
        store.append_all(batch).unwrap();
        let writers: Vec<(String, Option<String>)> = store
            .connection()
            .prepare("SELECT actor, on_behalf_of FROM ops WHERE field LIKE 'shortlist%' ORDER BY seq DESC LIMIT 2")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let jace = ("jace".to_owned(), None);
        let expected = [jace.clone(), jace];
        assert_eq!(writers, expected, "finish first: {finish_first}");
    }
}

/// Only a batch that finishes an Issue takes it off; a later write to a
/// finished Issue a merge left on a list does not.
#[test]
fn a_write_to_an_already_finished_member_leaves_its_membership() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs");
    let issue = Uuid::now_v7();
    let changes = [
        Change::Create,
        set_field("type", json!("task")),
        set_field("title", json!("t")),
        set_field("status", json!("done")),
        set_field("shortlist", json!(docs.to_string())),
        set_field("shortlist_rank", json!("V")),
    ];
    for (n, change) in (0..).zip(&changes) {
        let op = remote_op(7, issue, change, Hlc::new(1_000, n), 0);
        insert_acked(&store, &op, i64::from(n) + 10, 1_000);
    }
    store.rebuild().unwrap();
    let comment = Change::Comment {
        body: "noted".into(),
    };
    store.append(draft(issue, comment)).unwrap();
    assert_eq!(store.shortlist_members(docs).unwrap(), [issue]);
}

/// ADR 0012: an undo that restored a membership readers hide can itself be undone.
#[test]
fn an_undo_restoring_a_hidden_membership_can_be_undone() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let docs = list(&mut store, "docs");
    let issue = new_issue(&mut store);
    join(&mut store, issue, docs, "V");
    let left = store
        .append_all(vec![
            draft(issue, set_field("shortlist", Value::Null)),
            draft(issue, set_field("shortlist_rank", Value::Null)),
        ])
        .unwrap()[0]
        .op_id;
    store.delete_shortlist(&jace(), docs).unwrap();
    let (_, restored) = store.undo(&jace(), left).unwrap();
    let back = restored[0].op_id;
    assert!(store.can_undo(back).unwrap());
    store.undo(&jace(), back).unwrap();
    let stored: Option<Vec<u8>> = store
        .connection()
        .query_row(
            "SELECT shortlist FROM issues WHERE id = ?1",
            [issue.as_bytes()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, None);
}

#[test]
fn a_record_export_of_members_imports_again() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    store.set_workspace_id(Uuid::now_v7()).unwrap();
    let docs = list(&mut store, "docs");
    let issue = new_issue(&mut store);
    join(&mut store, issue, docs, "V");
    let mut out = Vec::new();
    store.export(&mut out, 1).unwrap();
    store.import_records(&mut out.as_slice(), &jace()).unwrap();
    assert_eq!(store.shortlist_members(docs).unwrap(), [issue]);
}

#[test]
fn a_malformed_rank_from_another_replica_is_not_applied() {
    let dir = TempDir::new();
    let store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let ops = [
        remote_op(7, issue, &Change::Create, Hlc::new(1_000, 0), 0),
        remote_op(
            7,
            issue,
            &set_field("shortlist_rank", json!("V0")),
            Hlc::new(1_000, 1),
            0,
        ),
    ];
    for (seq, op) in (1..).zip(&ops) {
        insert_acked(&store, op, seq + 2, 1_000);
    }
    let mut store = store;
    assert_eq!(store.rebuild().unwrap(), 1);
}
