//! Undo: a batch reverted by restoring what each touched field and element
//! held before it (ADR 0010).

use serde_json::json;
use tm_core::{Actor, Change, Hlc};
use tm_store::{Draft, Provenance, Store, StoreError, WallTime};
use uuid::Uuid;

use crate::support::{
    ManualClock, PROJECT, TempDir, claude, draft, field, held, insert_acked, jace, new_issue, open,
    remote_op, set_field,
};

/// Writes `change` on `issue` as jace, returning its batch.
fn write(store: &mut Store, issue: Uuid, change: Change) -> Uuid {
    store.append(draft(issue, change)).unwrap().op_id
}

fn undo(store: &mut Store, batch: Uuid) -> Result<Uuid, StoreError> {
    store.undo(&jace(), batch).map(|(_, ops)| ops[0].op_id)
}

fn ops(store: &Store) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM ops", [], |row| row.get(0))
        .unwrap()
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

fn labels(store: &Store, issue: Uuid) -> Vec<String> {
    store
        .issue(issue)
        .unwrap()
        .unwrap()
        .sets
        .get("labels")
        .map_or_else(Vec::new, |set| set.keys().cloned().collect())
}

#[test]
fn undo_restores_a_field_and_undoing_the_undo_redoes_it() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    write(&mut store, issue, set_field("priority", json!("low")));
    let raise = write(&mut store, issue, set_field("priority", json!("high")));

    let plan = store.undo_plan(&jace(), raise).unwrap();
    assert_eq!(plan.restores.len(), 1);
    assert_eq!(
        (&plan.restores[0].from, &plan.restores[0].to),
        (&json!("high"), &json!("low"))
    );
    let before = ops(&store);
    store.undo_plan(&jace(), raise).unwrap();
    assert_eq!(ops(&store), before, "a plan writes nothing");

    let undone = undo(&mut store, raise).unwrap();
    assert_eq!(field(&store, issue, "priority"), Some(json!("low")));
    undo(&mut store, undone).unwrap();
    assert_eq!(
        field(&store, issue, "priority"),
        Some(json!("high")),
        "redo"
    );
}

#[test]
fn a_field_with_nothing_before_the_batch_is_cleared() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let assign = write(&mut store, issue, set_field("assignee", json!("sam")));
    undo(&mut store, assign).unwrap();
    assert_eq!(field(&store, issue, "assignee"), None);
}

#[test]
fn a_later_write_to_the_same_field_is_a_conflict_and_nothing_is_written() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let first = write(&mut store, issue, set_field("priority", json!("high")));
    write(&mut store, issue, set_field("priority", json!("urgent")));
    let before = ops(&store);

    let err = undo(&mut store, first).unwrap_err();
    let StoreError::UndoConflict { conflicts, .. } = err else {
        panic!("{err}")
    };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].field, "priority");
    assert_eq!(
        conflicts[0].from,
        json!("urgent"),
        "names what it holds now"
    );
    assert_eq!(ops(&store), before);
    assert!(matches!(
        undo(&mut store, first),
        Err(StoreError::UndoConflict { .. })
    ));
}

#[test]
fn undoing_a_repeated_add_keeps_the_element_the_first_add_made() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    write(&mut store, issue, label("x", true));
    let again = write(&mut store, issue, label("x", true));
    let err = undo(&mut store, again).unwrap_err();
    assert!(matches!(err, StoreError::CannotUndo { .. }), "{err}");
    assert_eq!(
        labels(&store, issue),
        ["x"],
        "a flipped inverse would remove it"
    );

    let remove = write(&mut store, issue, label("x", false));
    undo(&mut store, remove).unwrap();
    assert_eq!(labels(&store, issue), ["x"]);
    let fresh = write(&mut store, issue, label("y", true));
    undo(&mut store, fresh).unwrap();
    assert_eq!(labels(&store, issue), ["x"]);
}

#[test]
fn undoing_a_create_archives_the_issue_and_keeps_its_key() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (issue, created) = store.create_issue(&jace(), PROJECT, "task", "t").unwrap();
    let key = store.issue(issue).unwrap().unwrap().key;
    write(
        &mut store,
        issue,
        Change::Comment {
            body: "later".into(),
        },
    );

    let undone = undo(&mut store, created[0].op_id).unwrap();
    let view = store.issue(issue).unwrap().unwrap();
    assert_eq!(view.fields["archived"], json!(true));
    assert_eq!(view.fields["title"], json!("t"));
    assert_eq!(view.key, key);
    undo(&mut store, undone).unwrap();
    assert_eq!(field(&store, issue, "archived"), Some(json!(false)));
}

#[test]
fn undoing_a_comment_retracts_it_and_undoing_that_brings_it_back() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let comment = write(
        &mut store,
        issue,
        Change::Comment {
            body: "oops".into(),
        },
    );
    let undone = undo(&mut store, comment).unwrap();
    assert!(store.comments(issue).unwrap()[0].retracted);
    undo(&mut store, undone).unwrap();
    assert!(!store.comments(issue).unwrap()[0].retracted);
}

#[test]
fn undoing_a_link_removes_it() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (a, b) = (new_issue(&mut store), new_issue(&mut store));
    let linked = store.link(&jace(), "blocks", a, b).unwrap().op_id;
    undo(&mut store, linked).unwrap();
    assert_eq!(store.links(a).unwrap(), []);
}

#[test]
fn a_take_is_undone_by_release_and_a_release_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let claim = store.take(&claude(), issue, None).unwrap();
    undo(&mut store, claim.id).unwrap();
    assert_eq!(held(&store, issue), None);

    let again = store.take(&claude(), issue, None).unwrap();
    let release = store.release(&claude(), issue, again.id).unwrap().op_id;
    let err = undo(&mut store, release).unwrap_err();
    assert!(
        matches!(&err, StoreError::CannotUndo { reason, .. } if reason.contains("tm claim")),
        "{err}"
    );
}

#[test]
fn an_unknown_batch_or_a_project_write_cannot_be_undone() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    assert!(matches!(
        undo(&mut store, Uuid::now_v7()),
        Err(StoreError::UnknownBatch(_))
    ));
    let (_, project) = store.create_project(&jace(), "NEW", "New").unwrap();
    assert!(matches!(
        undo(&mut store, project.op_id),
        Err(StoreError::CannotUndo { .. })
    ));
}

#[test]
fn an_operation_that_did_not_apply_is_not_a_prior_state() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let ignored = write(&mut store, issue, set_field("priority", json!("urgent")));
    store
        .connection()
        .execute(
            "INSERT INTO unapplied (op_id, reason) VALUES (?1, 'test')",
            [ignored.as_bytes()],
        )
        .unwrap();
    let raise = write(&mut store, issue, set_field("priority", json!("high")));
    let plan = store.undo_plan(&jace(), raise).unwrap();
    undo(&mut store, raise).unwrap();
    assert_eq!(field(&store, issue, "priority"), Some(json!("none")));
    assert_eq!(
        plan.restores[0].to,
        json!("none"),
        "unset, not urgent: {:?}",
        plan.restores
    );
}

#[test]
fn the_prior_state_is_the_merge_winner_not_the_last_logged() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(10_000));
    let issue = Uuid::now_v7();
    let create = remote_op(7, issue, &Change::Create, Hlc::new(1_000, 0), 0);
    let typed = remote_op(
        7,
        issue,
        &set_field("type", json!("task")),
        Hlc::new(1_001, 0),
        1,
    );
    let titled = remote_op(
        7,
        issue,
        &set_field("title", json!("t")),
        Hlc::new(1_002, 0),
        2,
    );
    let status = remote_op(
        7,
        issue,
        &set_field("status", json!("todo")),
        Hlc::new(1_003, 0),
        3,
    );
    // Concurrent: neither saw the other, so the later clock, logged first, wins.
    let high = remote_op(
        7,
        issue,
        &set_field("priority", json!("high")),
        Hlc::new(5_000, 0),
        4,
    );
    let low = remote_op(
        8,
        issue,
        &set_field("priority", json!("low")),
        Hlc::new(3_000, 0),
        4,
    );
    for (op, seq) in [create, typed, titled, status, high, low].iter().zip(1..) {
        insert_acked(&store, op, seq, 6_000);
    }
    store.rebuild().unwrap();
    assert_eq!(field(&store, issue, "priority"), Some(json!("high")));

    let urgent = write(&mut store, issue, set_field("priority", json!("urgent")));
    let plan = store.undo_plan(&jace(), urgent).unwrap();
    assert_eq!(plan.restores[0].to, json!("high"), "{:?}", plan.restores);
}

#[test]
fn the_prior_state_follows_replay_order_not_arrival() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(10_000));
    let issue = Uuid::now_v7();
    let setup = [
        (Change::Create, 1_000, 0),
        (set_field("type", json!("task")), 1_001, 1),
        (set_field("title", json!("t")), 1_002, 2),
        (set_field("status", json!("todo")), 1_003, 3),
    ];
    let mut server_seq = 0;
    for (change, ms, base) in setup {
        server_seq += 1;
        let op = remote_op(7, issue, &change, Hlc::new(ms, 0), base);
        insert_acked(&store, &op, server_seq, 6_000);
    }
    // Logged high, low, medium, but served high, medium, low. Replayed in
    // server order: medium saw high and replaces it; low saw neither and
    // beats medium on its later clock, so low wins. Folded in log order,
    // low loses to high and medium then replaces high.
    let high = remote_op(
        7,
        issue,
        &set_field("priority", json!("high")),
        Hlc::new(5_000, 0),
        4,
    );
    let low = remote_op(
        8,
        issue,
        &set_field("priority", json!("low")),
        Hlc::new(3_000, 0),
        4,
    );
    let medium = remote_op(
        9,
        issue,
        &set_field("priority", json!("medium")),
        Hlc::new(1_000, 0),
        5,
    );
    insert_acked(&store, &high, 5, 6_000);
    insert_acked(&store, &low, 7, 6_000);
    insert_acked(&store, &medium, 6, 6_000);
    store.rebuild().unwrap();
    assert_eq!(field(&store, issue, "priority"), Some(json!("low")));

    let urgent = write(&mut store, issue, set_field("priority", json!("urgent")));
    let plan = store.undo_plan(&jace(), urgent).unwrap();
    assert_eq!(plan.restores[0].to, json!("low"), "{:?}", plan.restores);
}

#[test]
fn a_batch_the_server_acknowledged_is_not_undone() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let raise = write(&mut store, issue, set_field("priority", json!("high")));
    store
        .connection()
        .execute(
            "UPDATE ops SET server_seq = seq WHERE op_id = ?1",
            [raise.as_bytes()],
        )
        .unwrap();
    let err = undo(&mut store, raise).unwrap_err();
    assert!(
        matches!(&err, StoreError::CannotUndo { reason, .. } if reason.contains("acknowledged")),
        "{err}"
    );
}

#[test]
fn a_restored_label_keeps_its_spelling() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    write(&mut store, issue, label("Backend", true));
    let remove = write(&mut store, issue, label("backend", false));
    undo(&mut store, remove).unwrap();
    let view = store.issue(issue).unwrap().unwrap();
    assert_eq!(view.sets["labels"]["backend"], "Backend");
}

#[test]
fn a_batch_that_set_a_default_has_nothing_to_undo() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let none = write(&mut store, issue, set_field("priority", json!("none")));
    let before = ops(&store);
    assert!(matches!(
        undo(&mut store, none),
        Err(StoreError::CannotUndo { .. })
    ));
    assert_eq!(ops(&store), before);
}

#[test]
fn a_plan_refuses_what_the_undo_would() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let comment = write(
        &mut store,
        issue,
        Change::Comment {
            body: "mine".into(),
        },
    );
    let stranger = Actor::human("sam");
    let err = store.undo_plan(&stranger, comment).unwrap_err();
    assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
    assert!(store.undo_plan(&jace(), comment).is_ok());
}

#[test]
fn undo_writes_as_the_actor_undoing() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let raise = store
        .append(Draft {
            actor: Actor::agent("claude", "jace"),
            ..draft(issue, set_field("priority", json!("high")))
        })
        .unwrap()
        .op_id;
    let (_, ops) = store.undo(&jace(), raise).unwrap();
    assert_eq!(ops[0].actor, "jace");
}

#[test]
fn undoes_step_back_through_writes_to_one_field() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let low = write(&mut store, issue, set_field("priority", json!("low")));
    let high = write(&mut store, issue, set_field("priority", json!("high")));

    undo(&mut store, high).unwrap();
    undo(&mut store, low).unwrap();
    assert_eq!(field(&store, issue, "priority"), Some(json!("none")));
}

#[test]
fn a_redone_batch_can_be_undone_again() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let raise = write(&mut store, issue, set_field("priority", json!("high")));
    let undone = undo(&mut store, raise).unwrap();
    undo(&mut store, undone).unwrap();

    undo(&mut store, raise).unwrap();
    assert_eq!(field(&store, issue, "priority"), Some(json!("none")));
}

#[test]
fn an_undone_pair_around_the_batch_still_conflicts() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let low = write(&mut store, issue, set_field("priority", json!("low")));
    let high = write(&mut store, issue, set_field("priority", json!("high")));
    let high_undone = undo(&mut store, high).unwrap();
    undo(&mut store, low).unwrap();

    // Undoing low is older than high_undone, so it is not ignored; redoing
    // high over it would hide that low was undone.
    assert!(matches!(
        undo(&mut store, high_undone),
        Err(StoreError::UndoConflict { .. })
    ));
    assert_eq!(field(&store, issue, "priority"), Some(json!("none")));
}

#[test]
fn an_undo_that_did_not_apply_undoes_nothing() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let low = write(&mut store, issue, set_field("priority", json!("low")));
    let high = write(&mut store, issue, set_field("priority", json!("high")));
    let undone = undo(&mut store, high).unwrap();
    store
        .connection()
        .execute(
            "INSERT INTO unapplied (op_id, reason) VALUES (?1, 'test')",
            [undone.as_bytes()],
        )
        .unwrap();

    assert!(matches!(
        undo(&mut store, low),
        Err(StoreError::UndoConflict { .. })
    ));
}

#[test]
fn a_batch_with_an_agents_write_is_not_the_humans() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let mixed = store
        .append_all(vec![
            draft(issue, set_field("priority", json!("high"))),
            Draft {
                actor: claude(),
                ..draft(issue, set_field("title", json!("by claude")))
            },
        ])
        .unwrap()[0]
        .op_id;
    assert!(!store.undo_candidates("jace", 10).unwrap().contains(&mixed));
}

#[test]
fn a_bare_undo_steps_back_through_the_humans_own_batches() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let low = write(&mut store, issue, set_field("priority", json!("low")));
    store
        .append(Draft {
            actor: claude(),
            ..draft(issue, set_field("title", json!("by claude")))
        })
        .unwrap();
    let high = write(&mut store, issue, set_field("priority", json!("high")));
    store.create_project(&jace(), "NEW", "New").unwrap();

    let candidates = store.undo_candidates("jace", 10).unwrap();
    assert_eq!(
        &candidates[..2],
        [high, low],
        "neither the Project nor the Agent's batch"
    );
    assert_eq!(candidates.len(), 3, "and the create");
    assert_eq!(
        store.undo_candidates("claude", 10).unwrap(),
        Vec::<Uuid>::new()
    );

    let before = store.undo_candidates("jace", 10).unwrap();
    store.undo_plan(&jace(), high).unwrap();
    assert_eq!(
        store.undo_candidates("jace", 10).unwrap(),
        before,
        "a plan records nothing"
    );

    let undone = undo(&mut store, high).unwrap();
    assert_eq!(store.undo_candidates("jace", 1).unwrap(), [low]);
    undo(&mut store, undone).unwrap();
    assert_eq!(
        store.undo_candidates("jace", 1).unwrap(),
        [high],
        "a redone batch is a candidate again"
    );

    undo(&mut store, high).unwrap();
    undo(&mut store, low).unwrap();
    assert_eq!(field(&store, issue, "priority"), Some(json!("none")));
}

#[test]
fn a_write_knows_whether_it_can_be_undone() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let ops = [
        Change::Create,
        set_field("type", json!("feature")),
        set_field("title", json!("t")),
        set_field("status", json!("shipped")),
    ];
    for (seq, change) in (1..).zip(&ops) {
        insert_acked(
            &store,
            &remote_op(7, issue, change, Hlc::new(1_000, 0), 0),
            seq,
            1_000,
        );
    }
    store.rebuild().unwrap();
    // Undoing either would put back what the Scheme refuses, a type it lacks
    // or a Status the type lacks, which the plan alone cannot see.
    let typed = write(&mut store, issue, set_field("type", json!("task")));
    assert!(!store.can_undo(typed).unwrap());
    let mut store = store.with_scheme(crate::support::scheme());
    assert!(
        store.can_undo(typed).unwrap(),
        "another Scheme drops the answer; the plan alone sees no refusal"
    );
    assert!(matches!(
        store.undo_plan(&jace(), typed),
        Err(StoreError::Invalid { .. })
    ));
    let fixed = write(&mut store, issue, set_field("status", json!("todo")));
    assert!(!store.can_undo(fixed).unwrap());
    store.rebuild().unwrap();
    assert!(store.can_undo(fixed).unwrap(), "so does a rebuild");
    assert!(matches!(
        undo(&mut store, fixed),
        Err(StoreError::Invalid { .. })
    ));
    assert_eq!(field(&store, issue, "status"), Some(json!("todo")));

    let raised = write(&mut store, issue, set_field("priority", json!("high")));
    assert!(store.can_undo(raised).unwrap());
}

#[test]
fn what_a_write_knew_of_its_undo_lapses_when_anyone_writes_after() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    let raise = write(&mut store, issue, set_field("priority", json!("high")));
    assert!(store.can_undo(raise).unwrap());

    let mut other = open(&dir.db(), &clock);
    write(&mut other, issue, set_field("priority", json!("low")));
    assert!(!store.can_undo(raise).unwrap(), "a later write conflicts");
}

#[test]
fn a_batch_of_several_writers_is_judged_by_its_plan() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    // claude may not retract codex's comment, but jace, for whom both write,
    // may undo the whole batch.
    let mixed = store
        .append_all(vec![
            Draft {
                actor: claude(),
                ..draft(issue, set_field("priority", json!("high")))
            },
            Draft {
                actor: Actor::agent("codex", "jace"),
                ..draft(
                    issue,
                    Change::Comment {
                        body: "by codex".into(),
                    },
                )
            },
        ])
        .unwrap()[0]
        .op_id;
    assert!(store.can_undo(mixed).unwrap());
    undo(&mut store, mixed).unwrap();
}

#[test]
fn an_imported_batch_keeps_its_source_and_time_and_only_a_named_undo_reverts_it() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(5_000));
    let issue = new_issue(&mut store);
    let at = WallTime {
        utc_ms: 1_200,
        offset_min: 60,
    };
    let imported = store
        .append_imported(vec![(
            draft(issue, set_field("priority", json!("high"))),
            Provenance {
                source: "beads:tm-1".into(),
                at,
            },
        )])
        .unwrap()
        .remove(0);
    assert_eq!(
        (
            imported.source.as_deref(),
            imported.utc_ms,
            imported.offset_min
        ),
        (Some("beads:tm-1"), 1_200, 60)
    );
    assert!(imported.hlc.ms() >= 5_000, "its clock is this Replica's");

    let candidates = store.undo_candidates("jace", 10).unwrap();
    assert!(!candidates.contains(&imported.op_id), "{candidates:?}");
    assert_eq!(candidates.len(), 1, "only the create");

    let (_, inverse) = store.undo(&jace(), imported.op_id).unwrap();
    assert!(inverse.iter().all(|op| op.source.is_none()), "{inverse:?}");
    assert_eq!(field(&store, issue, "priority"), Some(json!("none")));
}

#[test]
fn an_imported_time_an_operation_cannot_hold_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(5_000));
    let issue = new_issue(&mut store);
    let before = ops(&store);
    for at in [
        WallTime {
            utc_ms: -1,
            offset_min: 0,
        },
        WallTime {
            utc_ms: 1_000,
            offset_min: 26 * 60,
        },
    ] {
        let err = store
            .append_imported(vec![(
                draft(issue, set_field("priority", json!("high"))),
                Provenance {
                    source: "beads:tm-1".into(),
                    at,
                },
            )])
            .unwrap_err();
        assert!(
            matches!(&err, StoreError::Invalid { field, .. } if field == "time"),
            "{err:?}"
        );
    }
    assert_eq!(ops(&store), before);
}
