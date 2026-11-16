//! Parents and children, and a write that reads an Issue's ancestors inside
//! itself.

use serde_json::json;
use tm_core::Actor;
use tm_store::{Draft, IssueView, NewIssue, ParentOutcome, Store, WriteOptions};
use uuid::Uuid;

use crate::support::{
    ManualClock, PROJECT, TempDir, claude, draft, field, jace, new_issue, open, set_field, unpassed,
};

fn ids(views: &[IssueView]) -> Vec<Uuid> {
    views.iter().map(|view| view.id).collect()
}

/// `issue` and its ancestors as [`Store::append_with_lineage`] reads them.
fn lineage(store: &mut Store, issue: Uuid) -> Vec<Uuid> {
    store
        .append_with_lineage(Vec::new(), issue, &WriteOptions::default(), |lineage| {
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
    unpassed(
        store
            .append_all(vec![
                under(feature, epic),
                under(task, feature),
                under(archived, feature),
                draft(archived, set_field("archived", json!(true))),
            ])
            .unwrap(),
    );

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
fn an_agent_starting_work_starts_each_parent_but_one_archived_or_held_elsewhere() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let [task, archived, held, epic] = [(); 4].map(|()| new_issue(&mut store));
    let under =
        |issue: Uuid, above: Uuid| draft(issue, set_field("parent", json!(above.to_string())));
    unpassed(
        store
            .append_all(vec![
                under(task, archived),
                under(archived, held),
                under(held, epic),
            ])
            .unwrap(),
    );
    let codex = Actor::agent("codex", "jace");
    store.take(&codex, held, None).unwrap();
    // Archived as well as held, so archived must win.
    store.take(&codex, archived, None).unwrap();
    let archive = Draft {
        actor: codex.clone(),
        ..draft(archived, set_field("archived", json!(true)))
    };
    unpassed(store.append(archive).unwrap());

    let start = |store: &mut Store, actor: &Actor| {
        let moved = Draft {
            actor: actor.clone(),
            ..draft(task, set_field("status", json!("in_progress")))
        };
        let options = WriteOptions::default();
        let (written, starts) = store
            .write_starting_parents(actor, vec![moved], task, &options)
            .unwrap();
        let mut passed = Vec::new();
        written.into_inner(&mut passed);
        let starts = starts
            .into_iter()
            .map(|start| (start.id, start.outcome))
            .collect::<Vec<_>>();
        (starts, passed)
    };
    let started = || ParentOutcome::Started {
        from: "todo".into(),
        to: "in_progress".into(),
    };
    let holder = crate::support::held(&store, held).unwrap();
    let (starts, passed) = start(&mut store, &claude());
    assert_eq!(passed, []);
    assert_eq!(
        starts,
        [
            (archived, ParentOutcome::Archived),
            (held, ParentOutcome::Held(holder.clone())),
            (epic, started()),
        ]
    );
    assert_eq!(field(&store, epic, "status"), Some(json!("in_progress")));
    assert_eq!(field(&store, held, "status"), Some(json!("todo")));
    assert_eq!(field(&store, archived, "status"), Some(json!("todo")));

    unpassed(
        store
            .append(draft(task, set_field("status", json!("todo"))))
            .unwrap(),
    );
    let (starts, passed) = start(&mut store, &jace());
    assert_eq!(passed, [holder], "a human's start goes past codex's Claim");
    assert_eq!(
        starts,
        [(archived, ParentOutcome::Archived), (held, started())]
    );
}

#[test]
fn a_new_issue_starts_its_parents_only_when_created_active() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let parent = new_issue(&mut store);
    let child = |status: &str| NewIssue {
        issue_type: Some("task".into()),
        title: Some("t".into()),
        parent: Some(parent),
        fields: vec![("status".into(), json!(status))],
        labels: Vec::new(),
    };

    let (written, created) = store.create(&jace(), PROJECT, &child("todo")).unwrap();
    unpassed(written);
    assert_eq!(created.starts, []);
    assert_eq!(field(&store, parent, "status"), Some(json!("todo")));

    let (written, created) = store
        .create(&jace(), PROJECT, &child("in_progress"))
        .unwrap();
    let ops = unpassed(written);
    let started = ParentOutcome::Started {
        from: "todo".into(),
        to: "in_progress".into(),
    };
    assert_eq!(created.starts.len(), 1);
    assert_eq!(created.starts[0].outcome, started);
    assert_eq!(field(&store, parent, "status"), Some(json!("in_progress")));
    assert!(
        ops.iter().any(|op| op.subject == parent),
        "the start shares the create's batch"
    );
}

#[test]
fn the_drafts_made_from_the_lineage_share_the_batch() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let [parent, child] = [(); 2].map(|()| new_issue(&mut store));
    unpassed(
        store
            .append_all(vec![draft(
                child,
                set_field("parent", json!(parent.to_string())),
            )])
            .unwrap(),
    );

    let (written, ()) = store
        .append_with_lineage(
            vec![draft(child, set_field("status", json!("in_progress")))],
            child,
            &WriteOptions::default(),
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
    let ops = unpassed(written);
    assert_eq!(ops.len(), 2);
    assert!(store.can_undo(ops[0].op_id).unwrap());
    assert_eq!(field(&store, parent, "status"), Some(json!("in_progress")));
}
