//! The store refuses damaged or unreadable state instead of carrying on.

use serde_json::json;
use tm_core::hlc::MAX_MS;
use tm_core::{Change, Hlc};
use tm_store::{Clock, Store, StoreError, WallTime};
use uuid::Uuid;

use crate::support::{
    ManualClock, TempDir, draft, field, insert_acked, new_issue, open, remote_op, set_field,
};
use crate::support::{OTHER, PROJECT};

/// Log position of the edit `store_with_an_edit` makes, after the two seeded
/// Projects and the four operations that create the Issue.
const EDIT_SEQ: i64 = 7;

fn store_with_an_edit(dir: &TempDir) -> (Store, Uuid) {
    let mut store = open(&dir.db(), &ManualClock::at(50_000));
    let issue = new_issue(&mut store);
    store
        .append(draft(issue, set_field("title", json!("a"))))
        .unwrap();
    (store, issue)
}

fn reopen_err(dir: &TempDir) -> StoreError {
    Store::open_with_clock(&dir.db(), Box::new(ManualClock::at(1)))
        .err()
        .unwrap()
}

#[test]
fn stored_operations_cannot_be_edited_or_deleted() {
    let dir = TempDir::new();
    let (store, _) = store_with_an_edit(&dir);
    let conn = store.connection();
    for position in ["seq", "rowid", "oid", "_rowid_"] {
        assert!(
            conn.execute(&format!("UPDATE ops SET {position} = {position} + 100"), [])
                .is_err(),
            "{position} is writable"
        );
    }
    for column in [
        "op_id",
        "op_hash",
        "v",
        "kind",
        "replica_id",
        "actor",
        "project",
        "issue_id",
        "field",
        "elem",
        "hlc",
        "base_seq",
        "utc_ms",
        "offset_min",
        "payload",
    ] {
        assert!(
            conn.execute(&format!("UPDATE ops SET {column} = {column}"), [])
                .is_err(),
            "{column} is writable"
        );
    }
    assert!(conn.execute("DELETE FROM ops", []).is_err());
    assert!(
        conn.execute(
            "INSERT OR REPLACE INTO ops SELECT * FROM ops WHERE seq = 2",
            []
        )
        .is_err()
    );
    assert_eq!(
        conn.execute(
            "UPDATE ops SET server_seq = seq, server_received_ms = 1 WHERE server_seq IS NULL",
            []
        )
        .unwrap(),
        5,
        "the Issue's five operations take a server position"
    );
}

#[test]
fn an_operation_id_is_logged_once() {
    let dir = TempDir::new();
    let (store, _) = store_with_an_edit(&dir);
    let err = store
        .connection()
        .execute(
            "INSERT INTO ops (op_id, op_hash, v, kind, replica_id, actor, project, issue_id,
                              hlc, base_seq, utc_ms, offset_min, payload)
             SELECT op_id, op_hash, v, kind, replica_id, actor, project, issue_id,
                    hlc + 1, base_seq, utc_ms, offset_min, payload
             FROM ops WHERE seq = ?1",
            [EDIT_SEQ],
        )
        .unwrap_err();
    assert!(err.to_string().contains("UNIQUE"), "{err}");
}

#[test]
fn server_effects_are_written_once() {
    let dir = TempDir::new();
    let (store, _) = store_with_an_edit(&dir);
    let conn = store.connection();
    let create = "kind = 'create'";
    for (column, value) in [
        ("assigned_key", "'CPL-99'"),
        ("assigned_key", "NULL"),
        ("server_seq", "99"),
        ("server_received_ms", "99"),
    ] {
        conn.execute(
            "UPDATE ops SET server_seq = 5, server_received_ms = 5 WHERE server_seq IS NULL AND kind = 'create'",
            [],
        )
        .unwrap();
        let err = conn
            .execute(
                &format!("UPDATE ops SET {column} = {value} WHERE {create}"),
                [],
            )
            .unwrap_err();
        assert!(err.to_string().contains("set once"), "{column}: {err}");
    }
}

#[test]
fn rebuild_names_an_operation_whose_content_no_longer_matches_its_hash() {
    let dir = TempDir::new();
    let (mut store, _) = store_with_an_edit(&dir);
    store
        .connection()
        .execute_batch(
            r#"DROP TRIGGER ops_immutable;
               UPDATE ops SET payload = replace(payload, '"a"', '"b"') WHERE kind = 'set_field';"#,
        )
        .unwrap();
    let err = store.rebuild().unwrap_err();
    assert!(
        matches!(err, StoreError::Replay { seq: EDIT_SEQ, .. }),
        "{err}"
    );
}

#[test]
fn rebuild_names_the_position_of_a_row_that_cannot_be_read() {
    for damage in ["v = 70000", "offset_min = 99999", "op_id = x'01'"] {
        let dir = TempDir::new();
        let (mut store, _) = store_with_an_edit(&dir);
        store
            .connection()
            .execute_batch(&format!(
                "DROP TRIGGER ops_immutable; UPDATE ops SET {damage} WHERE seq = 2;"
            ))
            .unwrap();
        let err = store.rebuild().unwrap_err();
        assert!(
            matches!(err, StoreError::Replay { seq: 2, .. }),
            "{damage}: {err}"
        );
        if damage.starts_with("op_id") {
            assert!(
                matches!(err, StoreError::Replay { op_id: None, .. }),
                "{err}"
            );
        }
    }
}

#[test]
fn a_lost_or_stale_clock_state_is_recovered_from_the_log() {
    for damage in [
        "DELETE FROM meta WHERE k = 'last_hlc'",
        "UPDATE meta SET v = 0 WHERE k = 'last_hlc'",
    ] {
        let dir = TempDir::new();
        let clock = ManualClock::at(50_000);
        let issue = {
            let (store, issue) = store_with_an_edit(&dir);
            store.connection().execute(damage, []).unwrap();
            issue
        };
        clock.set(10_000);
        let mut store = open(&dir.db(), &clock);
        let op = store
            .append(draft(issue, set_field("title", json!("b"))))
            .unwrap();
        assert!(
            op.hlc.ms() >= 50_000,
            "{damage}: clock moved back to {}",
            op.hlc.ms()
        );
        assert_eq!(field(&store, issue, "title"), Some(json!("b")), "{damage}");
    }
}

#[test]
fn a_clock_state_ahead_of_the_log_is_kept() {
    let dir = TempDir::new();
    let clock = ManualClock::at(50_000);
    let ahead = Hlc::new(9_000_000, 3);
    let issue = {
        let (store, issue) = store_with_an_edit(&dir);
        store
            .connection()
            .execute(
                "UPDATE meta SET v = ?1 WHERE k = 'last_hlc'",
                [i64::try_from(ahead.raw()).unwrap()],
            )
            .unwrap();
        issue
    };
    let mut store = open(&dir.db(), &clock);
    let op = store
        .append(draft(issue, set_field("title", json!("b"))))
        .unwrap();
    assert!(op.hlc > ahead, "the clock moved back to {:?}", op.hlc);
}

#[test]
fn clock_recovery_does_not_adopt_another_replicas_clock() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let issue = {
        let store = open(&dir.db(), &clock);
        let issue = Uuid::now_v7();
        let far = Hlc::new(10 * 3_600_000, 0);
        insert_acked(
            &store,
            &remote_op(7, issue, &Change::Create, far, 0),
            1,
            1_000,
        );
        store
            .connection()
            .execute("DELETE FROM meta WHERE k = 'last_hlc'", [])
            .unwrap();
        issue
    };
    let mut store = open(&dir.db(), &clock);
    store.rebuild().unwrap();
    let op = store
        .append(draft(issue, set_field("title", json!("x"))))
        .unwrap();
    assert_eq!(op.hlc, Hlc::new(1_000, 0));
}

#[test]
fn a_lost_replica_identity_with_operations_is_corruption() {
    let dir = TempDir::new();
    {
        let (store, _) = store_with_an_edit(&dir);
        store
            .connection()
            .execute("DELETE FROM meta WHERE k = 'replica_id'", [])
            .unwrap();
    }
    let err = reopen_err(&dir);
    assert!(matches!(err, StoreError::Corrupt(_)), "{err}");
}

#[test]
fn a_store_format_this_tm_never_wrote_is_refused() {
    for (format, newer) in [(2, true), (0, false)] {
        let dir = TempDir::new();
        {
            let (store, _) = store_with_an_edit(&dir);
            store
                .connection()
                .execute("UPDATE meta SET v = ?1 WHERE k = 'format'", [format])
                .unwrap();
        }
        let err = reopen_err(&dir);
        if newer {
            assert!(
                matches!(err, StoreError::NewerStore { found: 2, .. }),
                "{err}"
            );
        } else {
            assert!(matches!(err, StoreError::Corrupt(_)), "{err}");
        }
    }
}

#[test]
fn an_unusable_clock_is_refused() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = Uuid::now_v7();
    for bad in [-5_000, i64::try_from(MAX_MS).unwrap() + 1] {
        clock.set(bad);
        let err = store.append(draft(issue, Change::Create)).unwrap_err();
        assert!(matches!(err, StoreError::Clock(_)), "{bad}: {err}");
    }
}

struct Offset(i16);

impl Clock for Offset {
    fn now(&self) -> Result<WallTime, String> {
        Ok(WallTime {
            utc_ms: 1_000,
            offset_min: self.0,
        })
    }
}

#[test]
fn an_offset_no_time_zone_has_is_refused() {
    let dir = TempDir::new();
    for (offset, ok) in [
        (1_559, true),
        (-1_559, true),
        (1_560, false),
        (i16::MIN, false),
    ] {
        let mut store = Store::open_with_clock(&dir.db(), Box::new(Offset(offset))).unwrap();
        crate::support::seed_projects(&mut store);
        let result = store.create_issue("jace", PROJECT, "task", "t");
        assert_eq!(result.is_ok(), ok, "{offset}: {result:?}");
    }
}

#[test]
fn an_operation_filed_under_another_project_is_refused() {
    let dir = TempDir::new();
    let (mut store, issue) = store_with_an_edit(&dir);
    let mut wrong = draft(issue, set_field("title", json!("b")));
    wrong.project = OTHER;
    let err = store.append(wrong).unwrap_err();
    assert!(
        matches!(err, StoreError::ProjectMismatch { ref expected, .. } if *expected == PROJECT),
        "{err}"
    );
}
