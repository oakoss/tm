//! Parents and children, and a write that reads an Issue's ancestors inside
//! itself.

use serde_json::json;
use tm_store::{IssueView, Store};
use uuid::Uuid;

use crate::support::{ManualClock, TempDir, draft, field, new_issue, open, set_field};

fn ids(views: &[IssueView]) -> Vec<Uuid> {
    views.iter().map(|view| view.id).collect()
}

/// `issue` and its ancestors as [`Store::append_with_lineage`] reads them.
fn lineage(store: &mut Store, issue: Uuid) -> Vec<Uuid> {
    store
        .append_with_lineage(Vec::new(), issue, |lineage| {
            (Vec::new(), ids(lineage.issues()))
        })
        .unwrap()
        .1
}

/// Points `child` at `parent` in the projection alone, as a merge can leave
/// it: a write refuses a cycle or an Issue that does not exist.
fn force_parent(store: &Store, child: Uuid, parent: Uuid) {
    store
        .connection()
        .execute(
            "UPDATE issues SET parent_id = ?1 WHERE id = ?2",
            [parent.as_bytes(), child.as_bytes()],
        )
        .unwrap();
}

#[test]
fn the_lineage_runs_from_the_issue_up_and_children_include_archived_ones() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let [epic, feature, task, archived] = [(); 4].map(|()| new_issue(&mut store));
    let under =
        |issue: Uuid, above: Uuid| draft(issue, set_field("parent", json!(above.to_string())));
    store
        .append_all(vec![
            under(feature, epic),
            under(task, feature),
            under(archived, feature),
            draft(archived, set_field("archived", json!(true))),
        ])
        .unwrap();

    assert_eq!(lineage(&mut store, task), [task, feature, epic]);
    assert_eq!(lineage(&mut store, epic), [epic]);
    let mut children = ids(&store.children(feature).unwrap());
    children.sort();
    assert_eq!(children, [task, archived]);
}

#[test]
fn the_lineage_stops_at_a_cycle_and_leaves_out_a_missing_ancestor() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let [a, b, c] = [(); 3].map(|()| new_issue(&mut store));
    force_parent(&store, a, b);
    force_parent(&store, b, a);
    assert_eq!(lineage(&mut store, a), [a, b]);

    force_parent(&store, c, Uuid::now_v7());
    assert_eq!(lineage(&mut store, c), [c]);
}

#[test]
fn the_drafts_made_from_the_lineage_share_the_batch() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let [parent, child] = [(); 2].map(|()| new_issue(&mut store));
    store
        .append_all(vec![draft(
            child,
            set_field("parent", json!(parent.to_string())),
        )])
        .unwrap();

    let (ops, ()) = store
        .append_with_lineage(
            vec![draft(child, set_field("status", json!("in_progress")))],
            child,
            |lineage| {
                let more = lineage
                    .issues()
                    .iter()
                    .filter(|issue| issue.id != child)
                    .map(|issue| draft(issue.id, set_field("status", json!("in_progress"))))
                    .collect();
                (more, ())
            },
        )
        .unwrap();
    assert_eq!(ops.len(), 2);
    assert!(store.can_undo(ops[0].op_id).unwrap());
    assert_eq!(field(&store, parent, "status"), Some(json!("in_progress")));
}
