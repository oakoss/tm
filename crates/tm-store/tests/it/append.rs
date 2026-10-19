use serde_json::{Value, json};
use tm_core::scheme::Category;
use tm_core::{Actor, Change, Hlc};
use tm_store::{Draft, Store, StoreError};
use uuid::Uuid;

use crate::support::{
    ManualClock, TempDir, claude, draft, field, insert_acked, jace, new_issue, open, remote_op,
    set_field,
};
use crate::support::{OTHER, PROJECT};

fn count_ops(store: &tm_store::Store) -> i64 {
    store
        .connection()
        .query_row(
            "SELECT count(*) FROM ops WHERE kind NOT LIKE '%project'",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn an_agent_write_records_the_human_it_writes_for() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (agent_issue, ops) = store
        .create_issue(&Actor::agent("claude", "jace"), PROJECT, "task", "t")
        .unwrap();
    assert!(
        ops.iter()
            .all(|op| op.on_behalf_of.as_deref() == Some("jace"))
    );
    let (human_issue, _) = store
        .create_issue(&Actor::agent("jace", "jace"), PROJECT, "task", "t")
        .unwrap();
    assert_eq!(store.rebuild().unwrap(), 0, "the stored column must hash");
    let stored = |issue: Uuid| -> Vec<(String, Option<String>)> {
        let conn = store.connection();
        let mut stmt = conn
            .prepare("SELECT DISTINCT actor, on_behalf_of FROM ops WHERE issue_id = ?1")
            .unwrap();
        stmt.query_map([issue.as_bytes()], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(
        stored(agent_issue),
        [("claude".to_owned(), Some("jace".to_owned()))]
    );
    assert_eq!(stored(human_issue), [("jace".to_owned(), None)]);
}

fn batches(store: &tm_store::Store) -> Vec<(Uuid, Uuid)> {
    let mut stmt = store
        .connection()
        .prepare("SELECT op_id, batch FROM op_batches ORDER BY op_id")
        .unwrap();
    stmt.query_map([], |row| {
        Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
    })
    .unwrap()
    .map(|row| {
        let (op, batch) = row.unwrap();
        (
            Uuid::from_slice(&op).unwrap(),
            Uuid::from_slice(&batch).unwrap(),
        )
    })
    .collect()
}

#[test]
fn each_write_is_one_batch_named_by_its_first_operation() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (issue, created) = store.create_issue(&jace(), PROJECT, "task", "t").unwrap();
    let edited = store
        .append(draft(issue, set_field("title", json!("u"))))
        .unwrap();
    let mut expected: Vec<(Uuid, Uuid)> = created
        .iter()
        .map(|op| (op.op_id, created[0].op_id))
        .chain([(edited.op_id, edited.op_id)])
        .collect();
    expected.sort();
    assert_eq!(batches(&store), expected);

    let refused = store.append_all(vec![
        draft(issue, set_field("title", json!("v"))),
        draft(issue, set_field("title", Value::Null)),
    ]);
    assert!(refused.is_err());
    assert_eq!(
        batches(&store),
        expected,
        "a refused write records no batch"
    );
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(batches(&store), expected, "a rebuild keeps the batches");
}

#[test]
fn every_write_helper_records_the_human_an_agent_writes_for() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let claude = Actor::agent("claude", "jace");
    let (a, b) = (new_issue(&mut store), new_issue(&mut store));
    let (project, created) = store.create_project(&claude, "AGT", "Agents").unwrap();
    let claim = store.take(&claude, a, None).unwrap();
    let ops = [
        created,
        store
            .rename_project(&claude, project, None, Some("Agent work"))
            .unwrap(),
        store.link(&claude, "blocks", a, b).unwrap(),
        store.unlink(&claude, "blocks", a, b).unwrap(),
        store.release(&claude, a, claim.id).unwrap(),
    ];
    for op in ops {
        assert_eq!(op.on_behalf_of.as_deref(), Some("jace"), "{}", op.kind);
    }
    let took: Option<String> = store
        .connection()
        .query_row(
            "SELECT on_behalf_of FROM ops WHERE op_id = ?1",
            [claim.id.as_bytes()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(took.as_deref(), Some("jace"), "take");
}

#[test]
fn creating_an_issue_writes_its_required_fields_in_its_starting_status() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (issue, ops) = store
        .create_issue(&jace(), PROJECT, "bug", "Crash")
        .unwrap();
    let kinds: Vec<&str> = ops.iter().map(|op| op.kind.as_str()).collect();
    assert_eq!(kinds, ["create", "set_field", "set_field", "set_field"]);
    assert_eq!(count_ops(&store), 4);
    let view = store.issue(issue).unwrap().unwrap();
    assert_eq!(view.fields["type"], json!("bug"));
    assert_eq!(view.fields["status"], json!("todo"));
    assert_eq!(view.fields["reporter"], json!("jace"));
    assert_eq!(view.fields["priority"], json!("none"));
    assert_eq!(view.category, Some(Category::Todo));
}

#[test]
fn enumerated_values_record_their_display_label() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let op = store
        .append(draft(issue, set_field("status", json!("in_review"))))
        .unwrap();
    let Change::SetField {
        label, value_label, ..
    } = op.change().unwrap()
    else {
        panic!("not a set_field");
    };
    assert_eq!(label, "Status");
    assert_eq!(value_label.as_deref(), Some("In Review"));
}

#[test]
fn an_issue_without_its_required_fields_is_refused_and_nothing_is_logged() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let err = store
        .append(draft(Uuid::now_v7(), Change::Create))
        .unwrap_err();
    let StoreError::MissingFields { fields, .. } = err else {
        panic!("{err}");
    };
    assert_eq!(fields, ["type", "title", "status"]);
    assert_eq!(count_ops(&store), 0);
}

#[test]
fn a_change_to_a_missing_issue_is_refused_and_nothing_is_logged() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let err = store
        .append(draft(Uuid::now_v7(), set_field("title", json!("x"))))
        .unwrap_err();
    assert!(matches!(err, StoreError::UnknownIssue(_)), "{err}");
    assert_eq!(count_ops(&store), 0);
}

#[test]
fn creating_an_issue_twice_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let err = store.append(draft(issue, Change::Create)).unwrap_err();
    assert!(matches!(err, StoreError::DuplicateIssue(_)), "{err}");
}

#[test]
fn an_invalid_field_name_is_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let err = store
        .append(draft(issue, set_field("a.b", json!(1))))
        .unwrap_err();
    assert!(matches!(err, StoreError::Op(_)), "{err}");
}

#[test]
fn an_unknown_field_lists_the_fields_that_can_be_set() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let err = store
        .append(draft(issue, set_field("severity", json!("high"))))
        .unwrap_err();
    let StoreError::UnknownField { valid, .. } = err else {
        panic!("{err}");
    };
    assert!(valid.contains(&"title".to_owned()));
    assert!(valid.contains(&"area".to_owned()));
    assert!(!valid.contains(&"created".to_owned()));
}

#[test]
fn values_the_scheme_does_not_allow_are_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let cases = [
        set_field("status", json!("shipped")),
        set_field("type", json!("saga")),
        set_field("priority", json!("p0")),
        set_field("tier", json!("silver")),
        set_field("area", json!("ten")),
        set_field("title", json!("  ")),
        set_field("parent", json!(issue.to_string())),
        Change::Add {
            field: "title".into(),
            label: String::new(),
            elem: "x".into(),
            elem_label: None,
        },
    ];
    for change in cases {
        let shown = format!("{change:?}");
        let err = store.append(draft(issue, change)).unwrap_err();
        assert!(matches!(err, StoreError::Invalid { .. }), "{shown}: {err}");
    }
    let err = store
        .append(draft(issue, set_field("status", json!("shipped"))))
        .unwrap_err();
    let StoreError::Invalid { valid, .. } = err else {
        panic!("{err}");
    };
    assert!(valid.contains(&"in_progress".to_owned()));
}

#[test]
fn computed_and_multi_value_fields_name_why_they_cannot_be_set() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let cases = [
        ("reporter", json!("codex"), "is computed and cannot be set"),
        (
            "labels",
            json!("bug"),
            "holds several values; add or remove them",
        ),
    ];
    for (name, value, expected) in cases {
        let err = store
            .append(draft(issue, set_field(name, value)))
            .unwrap_err();
        let StoreError::Invalid { reason, .. } = err else {
            panic!("{name}: {err}");
        };
        assert_eq!(reason, expected, "{name}");
    }
}

#[test]
fn required_fields_cannot_be_cleared() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let err = store
        .append(draft(issue, set_field("title", serde_json::Value::Null)))
        .unwrap_err();
    assert!(matches!(err, StoreError::MissingFields { .. }), "{err}");
}

#[test]
fn scheme_fields_are_stored_and_cleared() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    store
        .append(draft(issue, set_field("area", json!(3))))
        .unwrap();
    store
        .append(draft(issue, set_field("tier", json!("gold"))))
        .unwrap();
    assert_eq!(field(&store, issue, "area"), Some(json!(3)));
    assert_eq!(field(&store, issue, "tier"), Some(json!("gold")));
    store
        .append(draft(issue, set_field("area", serde_json::Value::Null)))
        .unwrap();
    assert_eq!(field(&store, issue, "area"), None);
}

#[test]
fn labels_ignore_case_and_keep_their_written_form() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let add = |elem: &str| Change::Add {
        field: "labels".into(),
        label: String::new(),
        elem: elem.into(),
        elem_label: None,
    };
    store.append(draft(issue, add("Bug"))).unwrap();
    let op = store.append(draft(issue, add("bug"))).unwrap();
    assert_eq!(
        op.change().unwrap().elem().as_deref(),
        Some("bug"),
        "stored lowercased"
    );
    let display: Option<String> = store
        .connection()
        .query_row("SELECT display FROM set_members", [], |r| r.get(0))
        .unwrap();
    assert_eq!(display, None, "the later add wrote the lowercase form");
    let sets = store.issue(issue).unwrap().unwrap().sets;
    assert_eq!(sets["labels"].len(), 1);
}

#[test]
fn a_batch_is_written_whole_or_not_at_all() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let before = count_ops(&store);
    let batch = vec![
        draft(issue, set_field("title", json!("renamed"))),
        draft(issue, set_field("status", json!("shipped"))),
    ];
    assert!(store.append_all(batch).is_err());
    assert_eq!(count_ops(&store), before);
    assert_eq!(field(&store, issue, "title"), Some(json!("t")));
}

#[test]
fn a_type_change_that_would_invalidate_the_status_is_refused() {
    let dir = TempDir::new();
    let mut two_workflows = crate::support::scheme();
    let mut bugs = two_workflows.workflows[0].clone();
    bugs.name = "bugs".into();
    bugs.statuses.retain(|status| status.id != "in_review");
    two_workflows.workflows.push(bugs);
    for issue_type in &mut two_workflows.types {
        if issue_type.id == "bug" {
            issue_type.workflow = "bugs".into();
        }
    }
    let mut store = open(&dir.db(), &ManualClock::at(1_000)).with_scheme(two_workflows);
    let issue = new_issue(&mut store);
    store
        .append(draft(issue, set_field("status", json!("in_review"))))
        .unwrap();
    let err = store
        .append(draft(issue, set_field("type", json!("bug"))))
        .unwrap_err();
    assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
    store
        .append(draft(issue, set_field("status", json!("todo"))))
        .unwrap();
    store
        .append(draft(issue, set_field("type", json!("bug"))))
        .unwrap();
}

#[test]
fn a_status_the_scheme_no_longer_defines_reads_as_triage() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    store
        .append(draft(issue, set_field("status", json!("done"))))
        .unwrap();
    assert_eq!(
        store.issue(issue).unwrap().unwrap().category,
        Some(Category::Done)
    );
    store
        .connection()
        .execute("UPDATE issues SET status = 'legacy'", [])
        .unwrap();
    assert_eq!(
        store.issue(issue).unwrap().unwrap().category,
        Some(Category::Triage)
    );
}

#[test]
fn comments_read_back_in_log_order_with_who_wrote_them() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    let other = new_issue(&mut store);
    let first = store
        .append(draft(
            issue,
            Change::Comment {
                body: "first".into(),
            },
        ))
        .unwrap();
    clock.set(2_000);
    store
        .append(Draft {
            actor: claude(),
            ..draft(
                issue,
                Change::Comment {
                    body: "second".into(),
                },
            )
        })
        .unwrap();
    store
        .append(draft(
            other,
            Change::Comment {
                body: "elsewhere".into(),
            },
        ))
        .unwrap();

    let comments = store.comments(issue).unwrap();
    let read: Vec<(&str, &str, Option<&str>, i64)> = comments
        .iter()
        .map(|c| {
            (
                c.body.as_str(),
                c.actor.as_str(),
                c.on_behalf_of.as_deref(),
                c.utc_ms,
            )
        })
        .collect();
    assert_eq!(
        read,
        [
            ("first", "jace", None, 1_000),
            ("second", "claude", Some("jace"), 2_000),
        ]
    );
    assert_eq!(comments[0].id, first.op_id);
    assert!(matches!(
        store.comments(Uuid::now_v7()),
        Err(StoreError::UnknownIssue(_))
    ));
}

#[test]
fn a_comment_is_retracted_by_its_author_or_their_human_and_can_come_back() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let comment = store
        .append(Draft {
            actor: claude(),
            ..draft(
                issue,
                Change::Comment {
                    body: "oops".into(),
                },
            )
        })
        .unwrap()
        .op_id;
    let retract = |retracted| Change::Retract { comment, retracted };
    let retracted = |store: &Store| store.comments(issue).unwrap()[0].retracted;

    for stranger in [
        Actor::agent("codex", "jace"),
        Actor::agent("claude", "kim"),
        Actor::human("claude"),
        Actor::agent("jace", "kim"),
    ] {
        let err = store
            .append(Draft {
                actor: stranger.clone(),
                ..draft(issue, retract(true))
            })
            .unwrap_err();
        assert!(
            matches!(err, StoreError::Invalid { ref field, .. } if field == "comments"),
            "{stranger:?}: {err}"
        );
    }
    assert!(!retracted(&store));

    store.append(draft(issue, retract(true))).unwrap();
    assert!(
        retracted(&store),
        "jace, the human claude wrote it for, may retract it"
    );
    store
        .append(Draft {
            actor: claude(),
            ..draft(issue, retract(false))
        })
        .unwrap();
    assert!(!retracted(&store), "and its author may bring it back");
    store.append(draft(issue, retract(true))).unwrap();
    store.rebuild().unwrap();
    assert!(retracted(&store), "a retraction replays");
    assert_eq!(
        store.comments(issue).unwrap()[0].body,
        "oops",
        "the words stay in the log"
    );

    let err = store
        .append(draft(
            issue,
            Change::Retract {
                comment: Uuid::now_v7(),
                retracted: true,
            },
        ))
        .unwrap_err();
    assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
    let err = store
        .append(draft(issue, set_field("comments", json!("x"))))
        .unwrap_err();
    assert!(
        err.to_string().contains("comments"),
        "only a retraction writes it: {err}"
    );
}

#[test]
fn concurrent_retractions_of_a_comment_merge_by_clock_not_arrival() {
    let dir = TempDir::new();
    let store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let comment = remote_op(
        7,
        issue,
        &Change::Comment { body: "x".into() },
        Hlc::new(2_000, 0),
        1,
    );
    let retract = |retracted, ms| {
        let change = Change::Retract {
            comment: comment.op_id,
            retracted,
        };
        remote_op(8, issue, &change, Hlc::new(ms, 0), 2)
    };
    let create = remote_op(7, issue, &Change::Create, Hlc::new(1_000, 0), 0);
    // Both saw the comment and neither saw the other; the later clock wins
    // though it arrives first.
    for (seq, op) in [
        (1, create),
        (2, comment.clone()),
        (3, retract(true, 5_000)),
        (4, retract(false, 3_000)),
    ] {
        insert_acked(&store, &op, seq, 6_000);
    }
    let mut store = store;
    store.rebuild().unwrap();
    assert!(store.comments(issue).unwrap()[0].retracted);
}

#[test]
fn a_retraction_that_replays_before_its_comment_still_hides_it() {
    let dir = TempDir::new();
    let store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let comment = remote_op(
        7,
        issue,
        &Change::Comment { body: "x".into() },
        Hlc::new(2_000, 0),
        1,
    );
    let retract = remote_op(
        8,
        issue,
        &Change::Retract {
            comment: comment.op_id,
            retracted: true,
        },
        Hlc::new(3_000, 0),
        1,
    );
    let create = remote_op(7, issue, &Change::Create, Hlc::new(1_000, 0), 0);
    for (seq, op) in [(1, create), (2, retract), (3, comment)] {
        insert_acked(&store, &op, seq, 4_000);
    }
    let mut store = store;
    store.rebuild().unwrap();
    assert!(store.comments(issue).unwrap()[0].retracted);
    assert_eq!(store.unapplied().unwrap(), []);
}

#[test]
fn comments_at_the_same_clock_order_by_replica() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let create = remote_op(7, issue, &Change::Create, Hlc::new(1_000, 0), 0);
    insert_acked(&store, &create, 1, 1_000);
    // Replica 8's operation gets the smaller id, so ordering by id alone
    // would put it first.
    for (seq, replica) in [(2, 8), (3, 7)] {
        let body = format!("replica {replica}");
        let op = remote_op(
            replica,
            issue,
            &Change::Comment { body },
            Hlc::new(5_000, 0),
            0,
        );
        insert_acked(&store, &op, seq, 5_000);
    }
    store.rebuild().unwrap();
    assert!(
        store.unapplied().unwrap().is_empty(),
        "{:?}",
        store.unapplied().unwrap()
    );
    let bodies: Vec<String> = store
        .comments(issue)
        .unwrap()
        .into_iter()
        .map(|comment| comment.body)
        .collect();
    assert_eq!(bodies, ["replica 7", "replica 8"]);
}

#[test]
fn every_change_advances_updated() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    clock.set(5_000);
    store
        .append(draft(issue, Change::Comment { body: "hi".into() }))
        .unwrap();
    assert_eq!(field(&store, issue, "created"), Some(json!(1_000)));
    assert_eq!(field(&store, issue, "updated"), Some(json!(5_000)));
    let other = new_issue(&mut store);
    clock.set(9_000);
    store.link(&jace(), "blocks", issue, other).unwrap();
    assert_eq!(field(&store, issue, "updated"), Some(json!(9_000)));
}

#[test]
fn the_clock_never_moves_backwards_across_reopens() {
    let dir = TempDir::new();
    let clock = ManualClock::at(50_000);
    let (issue, first) = {
        let mut store = open(&dir.db(), &clock);
        let (issue, ops) = store.create_issue(&jace(), PROJECT, "task", "t").unwrap();
        (issue, ops.last().unwrap().hlc)
    };
    clock.set(10_000);
    let mut store = open(&dir.db(), &clock);
    let second = store
        .append(draft(issue, set_field("title", json!("a"))))
        .unwrap();
    assert!(second.hlc > first);
    assert_eq!(second.hlc, Hlc::new(50_000, 4));
}

#[test]
fn the_replica_id_is_stable_across_reopens() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let first = open(&dir.db(), &clock).replica_id().unwrap();
    let second = open(&dir.db(), &clock).replica_id().unwrap();
    assert_eq!(first, second);
}

#[test]
fn a_store_records_its_workspace_once() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    assert_eq!(store.workspace_id().unwrap(), None);
    let id = Uuid::now_v7();
    store.set_workspace_id(id).unwrap();
    store.set_workspace_id(id).unwrap();
    let other = Uuid::now_v7();
    let err = store.set_workspace_id(other).unwrap_err();
    assert!(
        matches!(err, StoreError::WorkspaceMismatch { stored, given } if stored == id && given == other),
        "{err}"
    );
    assert_eq!(open(&dir.db(), &clock).workspace_id().unwrap(), Some(id));
}

#[test]
fn the_stored_hash_matches_the_operation() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (_, ops) = store.create_issue(&jace(), PROJECT, "task", "t").unwrap();
    for op in ops {
        let stored: Vec<u8> = store
            .connection()
            .query_row(
                "SELECT op_hash FROM ops WHERE op_id = ?1",
                [op.op_id.as_bytes()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, op.op_hash());
    }
}

#[test]
fn a_stale_projection_version_is_rebuilt_on_open() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let issue = {
        let mut store = open(&dir.db(), &clock);
        let issue = new_issue(&mut store);
        store
            .append(draft(issue, set_field("status", json!("done"))))
            .unwrap();
        store
            .connection()
            .execute_batch(
                "PRAGMA user_version = 0; DELETE FROM field_versions; UPDATE issues SET status = NULL;",
            )
            .unwrap();
        issue
    };
    let store = open(&dir.db(), &clock);
    assert_eq!(field(&store, issue, "status"), Some(json!("done")));
}

#[test]
fn a_draft_may_name_any_project_for_a_new_issue() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (issue, _) = store.create_issue(&jace(), OTHER, "task", "t").unwrap();
    let draft = Draft {
        actor: jace(),
        project: OTHER,
        subject: issue,
        change: set_field("title", json!("u")),
    };
    store.append(draft).unwrap();
    assert_eq!(store.issue(issue).unwrap().unwrap().project, OTHER);
}
