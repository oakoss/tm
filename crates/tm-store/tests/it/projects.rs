//! Projects as logged operations, and the Keys their Issues are given.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use tm_core::{Change, Hlc, OpError, ProjectId};
use tm_store::{Store, StoreError};
use uuid::Uuid;

use crate::support::{ManualClock, PROJECT, TempDir, insert_acked, jace, open, remote_op};

fn key(store: &Store, issue: Uuid) -> String {
    store.issue(issue).unwrap().unwrap().key.unwrap()
}

#[test]
fn issues_take_sequential_keys_from_their_project() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (core, _) = store.create_project(&jace(), "core", "Core").unwrap();
    let first = store.create_issue(&jace(), core, "task", "a").unwrap().0;
    let second = store.create_issue(&jace(), core, "task", "b").unwrap().0;
    assert_eq!(key(&store, first), "CORE-1");
    assert_eq!(key(&store, second), "CORE-2");
    assert_eq!(store.issue_by_key("core-2").unwrap(), Some(second));
    assert_eq!(store.project_by_key("Core").unwrap(), Some(core));
}

#[test]
fn malformed_and_taken_keys_are_refused() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    for bad in ["C", "2X", "TOOLONGKEYX", "CP-L"] {
        let err = store.create_project(&jace(), bad, "x").unwrap_err();
        assert!(
            matches!(err, StoreError::Op(OpError::InvalidProjectKey(_))),
            "{bad}: {err}"
        );
    }
    store.create_project(&jace(), "CORE", "Core").unwrap();
    let err = store.create_project(&jace(), "core", "Again").unwrap_err();
    assert!(matches!(err, StoreError::KeyTaken { .. }), "{err}");
    let err = store.create_project(&jace(), "NEW", "  ").unwrap_err();
    assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
}

#[test]
fn a_renamed_project_keeps_its_old_keys_and_numbering() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (core, _) = store.create_project(&jace(), "CORE", "Core").unwrap();
    let old = store.create_issue(&jace(), core, "task", "a").unwrap().0;
    store
        .rename_project(&jace(), core, Some("KERN"), Some("Kernel"))
        .unwrap();
    let new = store.create_issue(&jace(), core, "task", "b").unwrap().0;
    store
        .rename_project(&jace(), core, Some("ops2"), None)
        .unwrap();
    assert_eq!(store.project_by_key("Ops2").unwrap(), Some(core));
    store
        .rename_project(&jace(), core, Some("KERN"), None)
        .unwrap();
    assert_eq!(key(&store, old), "CORE-1");
    assert_eq!(key(&store, new), "KERN-2");
    assert_eq!(store.issue_by_key("CORE-1").unwrap(), Some(old));
    assert_eq!(store.project_by_key("CORE").unwrap(), Some(core));
    assert_eq!(store.project_by_key("KERN").unwrap(), Some(core));
    let err = store.create_project(&jace(), "CORE", "Thief").unwrap_err();
    assert!(
        matches!(err, StoreError::KeyTaken { project, .. } if project == core),
        "a retired key is never given to another Project: {err}"
    );
    store
        .rename_project(&jace(), core, Some("CORE"), None)
        .unwrap();
    let err = store.rename_project(&jace(), core, None, None).unwrap_err();
    assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
}

#[test]
fn issues_need_an_existing_project() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let err = store
        .create_issue(&jace(), ProjectId::new(), "task", "a")
        .unwrap_err();
    assert!(matches!(err, StoreError::UnknownProject(_)), "{err}");
}

#[test]
fn rebuild_reads_keys_from_the_log_and_never_reuses_one() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (core, _) = store.create_project(&jace(), "CORE", "Core").unwrap();
    let issues: Vec<Uuid> = (0..3)
        .map(|n| {
            store
                .create_issue(&jace(), core, "task", &format!("{n}"))
                .unwrap()
                .0
        })
        .collect();
    // Removing the newest Issue from the projection, as purge will, leaves its
    // number spent: the next Key counts past it, not past the Keys left.
    store
        .connection()
        .execute(
            "DELETE FROM keys WHERE issue_id = ?1",
            [issues[2].as_bytes()],
        )
        .unwrap();
    let next = store.create_issue(&jace(), core, "task", "next").unwrap().0;
    assert_eq!(key(&store, next), "CORE-4");
    store
        .connection()
        .execute_batch("DELETE FROM keys; PRAGMA user_version = 0;")
        .unwrap();
    assert_eq!(store.rebuild().unwrap(), 0);
    for (issue, expected) in issues
        .iter()
        .chain([&next])
        .zip(["CORE-1", "CORE-2", "CORE-3", "CORE-4"])
    {
        assert_eq!(
            key(&store, *issue),
            expected,
            "rebuild reads every Key from the log"
        );
    }
}

#[test]
fn a_project_operation_must_name_itself_as_subject() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let project = ProjectId::new();
    let err = store
        .append(tm_store::Draft {
            actor: jace(),
            project,
            subject: Uuid::now_v7(),
            change: Change::CreateProject {
                key: "ODD".into(),
                name: "Odd".into(),
            },
        })
        .unwrap_err();
    assert!(matches!(err, StoreError::SubjectNotProject { .. }), "{err}");
}

#[test]
fn a_remote_project_taking_a_held_key_is_recorded_not_applied() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let thief = ProjectId::new();
    let change = Change::CreateProject {
        key: "CPL".into(),
        name: "Thief".into(),
    };
    let mut op = remote_op(7, thief.uuid(), &change, Hlc::new(2_000, 0), 0);
    op.project = thief;
    insert_acked(&store, &op, 1, 2_000);
    assert_eq!(store.rebuild().unwrap(), 1);
    assert!(store.unapplied().unwrap()[0].reason.contains("CPL"));
}

#[test]
fn the_next_key_counts_past_the_highest_number_even_with_gaps() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let remote = Uuid::now_v7();
    let op = remote_op(7, remote, &Change::Create, Hlc::new(2_000, 0), 0);
    insert_acked(&store, &op, 1, 2_000);
    store
        .connection()
        .execute(
            "UPDATE ops SET assigned_key = 'CPL-10' WHERE op_id = ?1",
            [op.op_id.as_bytes()],
        )
        .unwrap();
    assert_eq!(store.rebuild().unwrap(), 0);
    let next = store
        .create_issue(&jace(), PROJECT, "task", "next")
        .unwrap()
        .0;
    assert_eq!(key(&store, next), "CPL-11");
}

#[test]
fn a_remote_issue_in_an_unknown_project_is_recorded_not_applied() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let mut op = remote_op(7, issue, &Change::Create, Hlc::new(2_000, 0), 0);
    op.project = ProjectId::new();
    insert_acked(&store, &op, 1, 2_000);
    assert_eq!(store.rebuild().unwrap(), 1);
    assert!(store.issue(issue).unwrap().is_none());
}

/// An acknowledged create from another Replica in PROJECT carrying `key`.
fn remote_create(store: &Store, seq: i64, key: &str) -> Uuid {
    let issue = Uuid::now_v7();
    let op = remote_op(7, issue, &Change::Create, Hlc::new(2_000, 0), 0);
    insert_acked(store, &op, seq, 2_000);
    store
        .connection()
        .execute(
            "UPDATE ops SET assigned_key = ?1 WHERE op_id = ?2",
            rusqlite::params![key, op.op_id.as_bytes()],
        )
        .unwrap();
    issue
}

#[test]
fn a_create_left_unapplied_keeps_its_number() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (issue, _) = store.create_issue(&jace(), PROJECT, "task", "a").unwrap();
    assert_eq!(key(&store, issue), "CPL-1");
    let duplicate = remote_op(7, issue, &Change::Create, Hlc::new(500, 0), 0);
    insert_acked(&store, &duplicate, 1, 500);
    assert_eq!(
        store.rebuild().unwrap(),
        1,
        "the local create no longer applies"
    );
    let next = store.create_issue(&jace(), PROJECT, "task", "b").unwrap().0;
    assert_eq!(key(&store, next), "CPL-2");
}

#[test]
fn an_issue_whose_key_collides_keeps_everything_but_the_key() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (local, _) = store
        .create_issue(&jace(), PROJECT, "task", "mine")
        .unwrap();
    store
        .append(crate::support::draft(
            local,
            Change::Comment { body: "hi".into() },
        ))
        .unwrap();
    let remote = remote_create(&store, 1, "CPL-1");
    assert_eq!(
        store.rebuild().unwrap(),
        1,
        "the local create applies without its Key"
    );
    let view = store.issue(local).unwrap().unwrap();
    assert_eq!(view.key, None);
    assert_eq!(view.fields["title"], "mine");
    let comments: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM comments WHERE issue_id = ?1",
            [local.as_bytes()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(comments, 1);
    assert_eq!(key(&store, remote), "CPL-1");
    let unapplied = store.unapplied().unwrap();
    assert!(
        unapplied
            .iter()
            .any(|row| row.reason.starts_with("applied without its Key")),
        "{unapplied:?}"
    );
}

#[test]
fn keys_from_other_replicas_must_be_canonical_and_their_projects() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    for (seq, bad) in [(1, "CPL-007"), (2, "cpl-5"), (3, "CP-L-3"), (4, "OPS-1")] {
        let issue = remote_create(&store, seq, bad);
        store.rebuild().unwrap();
        assert_eq!(store.issue(issue).unwrap().unwrap().key, None, "{bad}");
    }
    let (ops_issue, _) = store
        .create_issue(&jace(), crate::support::OTHER, "task", "a")
        .unwrap();
    assert_eq!(key(&store, ops_issue), "OPS-1");
}

#[test]
fn a_key_this_store_chose_is_recorded_or_the_create_fails() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    store
        .connection()
        .execute(
            "INSERT INTO keys (key, issue_id, project, number, provisional)
             VALUES ('CPL-1', ?1, ?2, 1, 0)",
            rusqlite::params![Uuid::now_v7().as_bytes(), PROJECT.to_string()],
        )
        .unwrap();
    let err = store
        .create_issue(&jace(), PROJECT, "task", "a")
        .unwrap_err();
    assert!(matches!(err, StoreError::Corrupt(_)), "{err}");
}

#[test]
fn a_spent_key_counter_is_an_error_not_a_panic() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    remote_create(&store, 1, "OPS-9223372036854775807");
    let last = remote_create(&store, 2, "CPL-9223372036854775807");
    store.rebuild().unwrap();
    let last_op: Vec<u8> = store
        .connection()
        .query_row(
            "SELECT op_id FROM ops WHERE issue_id = ?1",
            [last.as_bytes()],
            |row| row.get(0),
        )
        .unwrap();
    let err = store
        .create_issue(&jace(), PROJECT, "task", "a")
        .unwrap_err();
    match err {
        StoreError::KeysExhausted { key, op_id } => {
            assert_eq!(key, "CPL");
            assert_eq!(op_id.as_bytes().as_slice(), last_op.as_slice());
        }
        other => panic!("{other}"),
    }
}

#[test]
fn another_projects_prefix_never_spends_numbers() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    remote_create(&store, 1, "OPS-9223372036854775807");
    store.rebuild().unwrap();
    let (issue, _) = store.create_issue(&jace(), PROJECT, "task", "a").unwrap();
    assert_eq!(key(&store, issue), "CPL-1");
}

#[test]
fn a_number_counts_before_its_prefix_is_held() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    remote_create(&store, 1, "KERN-4");
    let rename = Change::RenameProject {
        key: Some("KERN".into()),
        name: None,
    };
    let op = remote_op(7, PROJECT.uuid(), &rename, Hlc::new(2_500, 0), 0);
    insert_acked(&store, &op, 2, 2_500);
    store.rebuild().unwrap();
    let (issue, _) = store.create_issue(&jace(), PROJECT, "task", "a").unwrap();
    assert_eq!(key(&store, issue), "KERN-5");
}

#[test]
fn the_next_key_follows_the_highest_number_not_the_last() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    remote_create(&store, 1, "CPL-10");
    remote_create(&store, 2, "CPL-3");
    store.rebuild().unwrap();
    let (issue, _) = store.create_issue(&jace(), PROJECT, "task", "a").unwrap();
    assert_eq!(key(&store, issue), "CPL-11");
}

#[test]
fn one_number_under_two_prefixes_names_one_issue() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let rename = Change::RenameProject {
        key: Some("KERN".into()),
        name: None,
    };
    let op = remote_op(7, PROJECT.uuid(), &rename, Hlc::new(1_500, 0), 0);
    insert_acked(&store, &op, 1, 1_500);
    let first = remote_create(&store, 2, "KERN-1");
    let second = remote_create(&store, 3, "CPL-1");
    assert_eq!(store.rebuild().unwrap(), 1);
    assert_eq!(key(&store, first), "KERN-1");
    assert_eq!(store.issue(second).unwrap().unwrap().key, None);
    assert_eq!(store.issue_by_key("CPL-1").unwrap(), Some(first));
}

#[test]
fn a_remote_project_operation_naming_another_subject_is_unapplied() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let rename = Change::RenameProject {
        key: None,
        name: Some("Elsewhere".into()),
    };
    let op = remote_op(7, Uuid::now_v7(), &rename, Hlc::new(1_500, 0), 0);
    insert_acked(&store, &op, 1, 1_500);
    assert_eq!(store.rebuild().unwrap(), 1);
    let unapplied = store.unapplied().unwrap();
    assert!(unapplied[0].reason.contains("subject"), "{unapplied:?}");
}

#[test]
fn a_number_names_one_issue_per_project() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    store.create_issue(&jace(), PROJECT, "task", "a").unwrap();
    let err = store
        .connection()
        .execute(
            "INSERT INTO keys (key, issue_id, project, number, provisional)
             VALUES ('XYZ-1', ?1, ?2, 1, 0)",
            rusqlite::params![Uuid::now_v7().as_bytes(), PROJECT.to_string()],
        )
        .unwrap_err();
    assert!(err.to_string().contains("UNIQUE"), "{err}");
}

#[test]
fn any_key_a_project_held_finds_its_issues() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (issue, _) = store.create_issue(&jace(), PROJECT, "task", "a").unwrap();
    store
        .rename_project(&jace(), PROJECT, Some("KERN"), None)
        .unwrap();
    assert_eq!(store.issue_by_key("cpl-1").unwrap(), Some(issue));
    assert_eq!(store.issue_by_key("KERN-1").unwrap(), Some(issue));
    assert_eq!(store.issue_by_key("NOPE-1").unwrap(), None);
    for bad in [" CPL-1", "CPL-007", "CPL"] {
        let err = store.issue_by_key(bad).unwrap_err();
        assert!(matches!(err, StoreError::Invalid { .. }), "{bad}: {err}");
    }
}

#[test]
fn only_a_create_carries_a_key() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (issue, ops) = store.create_issue(&jace(), PROJECT, "task", "a").unwrap();
    let _ = issue;
    let set_field = &ops[1];
    let err = store
        .connection()
        .execute(
            "UPDATE ops SET assigned_key = 'CPL-9' WHERE op_id = ?1",
            [set_field.op_id.as_bytes()],
        )
        .unwrap_err();
    assert!(err.to_string().contains("only a create"), "{err}");
    let err = store
        .connection()
        .execute(
            "INSERT INTO ops (op_id, op_hash, v, kind, replica_id, actor, project, issue_id,
                              hlc, base_seq, utc_ms, offset_min, payload, assigned_key)
             SELECT ?1, op_hash, v, kind, replica_id, actor, project, issue_id,
                    hlc + 1, base_seq, utc_ms, offset_min, payload, 'CPL-9'
             FROM ops WHERE op_id = ?2",
            [Uuid::now_v7().as_bytes(), set_field.op_id.as_bytes()],
        )
        .unwrap_err();
    assert!(err.to_string().contains("only a create"), "{err}");
}

const CHILD_DB: &str = "TM_STORE_TEST_KEYS_DB";
const CHILD_PROJECT: &str = "TM_STORE_TEST_KEYS_PROJECT";
const PROCESSES: usize = 20;
const CREATES_EACH: usize = 5;

#[test]
fn concurrent_creates_never_share_a_key() {
    if let (Ok(db), Ok(project)) = (std::env::var(CHILD_DB), std::env::var(CHILD_PROJECT)) {
        let mut store = Store::open(&PathBuf::from(db)).unwrap();
        let project: ProjectId = project.parse().unwrap();
        for n in 0..CREATES_EACH {
            store
                .create_issue(&jace(), project, "task", &format!("{n}"))
                .unwrap();
        }
        return;
    }
    let dir = TempDir::new();
    let project = Store::open(&dir.db())
        .unwrap()
        .create_project(&jace(), "RACE", "Race")
        .unwrap()
        .0;
    let exe = std::env::current_exe().unwrap();
    let children: Vec<_> = (0..PROCESSES)
        .map(|_| {
            Command::new(&exe)
                .args(["--exact", "projects::concurrent_creates_never_share_a_key"])
                .args(["--test-threads", "1", "--quiet"])
                .env(CHILD_DB, dir.db())
                .env(CHILD_PROJECT, project.to_string())
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success(), "a creating process failed");
    }
    let store = Store::open(&dir.db()).unwrap();
    let mut stmt = store
        .connection()
        .prepare("SELECT assigned_key FROM ops WHERE kind = 'create'")
        .unwrap();
    let keys: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let total = PROCESSES * CREATES_EACH;
    let expected: BTreeSet<String> = (1..=total).map(|n| format!("RACE-{n}")).collect();
    assert_eq!(keys.len(), total);
    assert_eq!(keys.into_iter().collect::<BTreeSet<_>>(), expected);
}
