//! Export and restore: the log, verbatim, as JSONL (ADR 0013).

use std::path::Path;

use proptest::prelude::*;
use serde_json::{Value, json};
use tm_core::Hlc;
use tm_store::{Store, StoreError};
use uuid::Uuid;

use crate::rebuild::{Action, ISSUES, action, change};
use crate::support::{
    ManualClock, TempDir, claude, draft, dump, jace, new_issue, open, remote_op, set_field,
};

const WORKSPACE: Uuid = Uuid::from_u128(0x0190_a000_0000_7000_8000_0000_0000_0001);

/// A Workspace's store with data.
fn source(dir: &TempDir, clock: &ManualClock) -> Store {
    let mut store = open(&dir.db(), clock);
    store.set_workspace_id(WORKSPACE).unwrap();
    store
}

/// A Workspace's store as restore finds it on a machine without it: no
/// operations at all.
fn empty(path: &Path, clock: &ManualClock, workspace: Uuid) -> Store {
    let mut store = Store::open_with_clock(path, Box::new(clock.clone())).unwrap();
    store.set_workspace_id(workspace).unwrap();
    store
}

fn export(store: &Store) -> Vec<u8> {
    let mut out = Vec::new();
    store.export(&mut out, 1).unwrap();
    out
}

fn lines(bytes: &[u8]) -> Vec<Value> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect()
}

fn to_bytes(records: &[Value]) -> Vec<u8> {
    records
        .iter()
        .flat_map(|record| {
            let mut line = serde_json::to_vec(record).unwrap();
            line.push(b'\n');
            line
        })
        .collect()
}

/// Applies `action` as the rebuild test does, but any refusal is fine: what
/// matters is the log it leaves.
fn play(store: &mut Store, issues: &[Uuid], action: &Action, clock: &ManualClock) {
    match action {
        Action::Tick(ms) => clock.advance(i64::from(*ms)),
        Action::Link {
            from,
            to,
            link_type,
            present,
        } => {
            let link_type = ["blocks", "relates_to"][*link_type];
            let _ = if *present {
                store.link(&jace(), link_type, issues[*from], issues[*to])
            } else {
                store.unlink(&jace(), link_type, issues[*from], issues[*to])
            };
        }
        Action::Claim { issue, release } => {
            let held = store.claim(issues[*issue]).unwrap();
            let _ = match (release, held) {
                (true, Some(claim)) => store.release(&jace(), issues[*issue], claim.id).map(|_| ()),
                _ => store.take(&claude(), issues[*issue], None).map(|_| ()),
            };
        }
        other => {
            if let Some((issue, change)) = change(other) {
                let _ = store.append(draft(issues[issue], change));
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// ADR 0013: a restored log is the same store, and exports the same bytes.
    #[test]
    fn a_restored_export_is_the_same_store_and_exports_the_same_bytes(
        actions in proptest::collection::vec(action(), 0..50),
    ) {
        let (dir, other) = (TempDir::new(), TempDir::new());
        let clock = ManualClock::at(1_000);
        let mut store = source(&dir, &clock);
        let issues: Vec<Uuid> = (0..ISSUES).map(|_| new_issue(&mut store)).collect();
        for action in &actions {
            play(&mut store, &issues, action, &clock);
        }
        let first = export(&store);

        let mut copy = empty(&other.db(), &clock, WORKSPACE);
        let restored = copy.restore(&mut first.as_slice()).unwrap();
        prop_assert!(!restored.already_held);
        prop_assert_eq!(restored.unapplied, store.unapplied().unwrap().len());
        prop_assert_eq!(dump(&copy), dump(&store));
        prop_assert_eq!(export(&copy), first);
    }
}

#[test]
fn a_retracted_comment_keeps_its_words_through_a_restore() {
    let (dir, other) = (TempDir::new(), TempDir::new());
    let clock = ManualClock::at(1_000);
    let mut store = source(&dir, &clock);
    let issue = new_issue(&mut store);
    let comment = store
        .append(draft(
            issue,
            tm_core::Change::Comment {
                body: "kept in the log".into(),
            },
        ))
        .unwrap()
        .op_id;
    store.undo(&jace(), comment).unwrap();
    let mut copy = empty(&other.db(), &clock, WORKSPACE);
    copy.restore(&mut export(&store).as_slice()).unwrap();

    let retracted = copy.undo_candidates("jace", 10).unwrap();
    assert!(
        retracted.is_empty(),
        "undo state stays behind: {retracted:?}"
    );
    copy.append(draft(
        issue,
        tm_core::Change::Retract {
            comment,
            retracted: false,
        },
    ))
    .unwrap();
    let shown = copy.comments(issue).unwrap();
    assert_eq!(
        (shown[0].body.as_str(), shown[0].retracted),
        ("kept in the log", false)
    );
}

#[test]
fn restoring_what_a_store_holds_writes_nothing() {
    let (dir, other) = (TempDir::new(), TempDir::new());
    let clock = ManualClock::at(1_000);
    let mut store = source(&dir, &clock);
    new_issue(&mut store);
    let file = export(&store);
    let mut copy = empty(&other.db(), &clock, WORKSPACE);
    copy.restore(&mut file.as_slice()).unwrap();

    let again = copy.restore(&mut file.as_slice()).unwrap();
    assert!(again.already_held);
    assert_eq!(export(&copy), file);

    new_issue(&mut copy);
    let err = copy.restore(&mut file.as_slice()).unwrap_err();
    assert!(
        matches!(err, StoreError::WorkspaceNotEmpty { .. }),
        "a store that wrote since holds more than the file: {err}"
    );
}

#[test]
fn a_store_holding_other_operations_or_another_workspace_is_refused() {
    let (dir, other, third) = (TempDir::new(), TempDir::new(), TempDir::new());
    let clock = ManualClock::at(1_000);
    let mut store = source(&dir, &clock);
    new_issue(&mut store);
    let file = export(&store);

    let mut busy = source(&other, &clock);
    let before = export(&busy);
    let err = busy.restore(&mut file.as_slice()).unwrap_err();
    assert!(matches!(err, StoreError::WorkspaceNotEmpty { .. }), "{err}");
    assert_eq!(export(&busy), before, "nothing written");

    let mut elsewhere = empty(&third.db(), &clock, Uuid::now_v7());
    let err = elsewhere.restore(&mut file.as_slice()).unwrap_err();
    assert!(
        matches!(err, StoreError::WorkspaceMismatch { given, .. } if given == WORKSPACE),
        "{err}"
    );
}

#[test]
fn a_damaged_or_foreign_file_is_refused_by_line() {
    let (dir, other) = (TempDir::new(), TempDir::new());
    let clock = ManualClock::at(1_000);
    let mut store = source(&dir, &clock);
    new_issue(&mut store);
    let records = lines(&export(&store));
    let mut copy = empty(&other.db(), &clock, WORKSPACE);

    let mut tampered = records.clone();
    let last = tampered.len() - 1;
    tampered[last]["payload"] = json!(
        tampered[last]["payload"]
            .as_str()
            .unwrap()
            .replace("todo", "done")
    );
    let err = copy
        .restore(&mut to_bytes(&tampered).as_slice())
        .unwrap_err();
    assert!(
        matches!(err, StoreError::InvalidExport { line, .. } if line == records.len()),
        "{err}"
    );

    let mut moved = records.clone();
    moved[last]["field"] = json!("description");
    let err = copy.restore(&mut to_bytes(&moved).as_slice()).unwrap_err();
    assert!(
        matches!(&err, StoreError::InvalidExport { reason, .. } if reason.contains("field")),
        "{err}"
    );

    let mut repeated = records.clone();
    repeated[last] = records[last - 1].clone();
    let err = copy
        .restore(&mut to_bytes(&repeated).as_slice())
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::InvalidExport { reason, .. } if reason.contains("twice")),
        "{err}"
    );

    let mut unreadable = records.clone();
    let mut op = remote_op(
        7,
        Uuid::now_v7(),
        &tm_core::Change::Create,
        Hlc::new(1_000, 0),
        0,
    );
    op.project = serde_json::from_value(records[last]["project"].clone()).unwrap();
    op.payload = "not json".into();
    let mut line = records[last].clone();
    line["op_id"] = json!(op.op_id);
    line["issue_id"] = json!(op.subject);
    line["kind"] = json!(op.kind);
    line["replica_id"] = json!(op.replica_id);
    line["actor"] = json!(op.actor);
    line["hlc"] = json!(op.hlc.raw());
    line["base_seq"] = json!(op.base_seq);
    line["utc_ms"] = json!(op.utc_ms);
    line["offset_min"] = json!(op.offset_min);
    line["payload"] = json!(op.payload);
    line["field"] = Value::Null;
    line["elem"] = Value::Null;
    line["op_hash"] = json!(op.op_hash().iter().fold(String::new(), |mut hex, b| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
        hex
    }));
    unreadable.push(line);
    let err = copy
        .restore(&mut to_bytes(&unreadable).as_slice())
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::InvalidExport { line, reason } if *line == unreadable.len() && reason.contains("parse")),
        "{err}"
    );

    let mut newer = records.clone();
    newer[0]["op_format"] = json!(99);
    let err = copy.restore(&mut to_bytes(&newer).as_slice()).unwrap_err();
    assert!(
        matches!(err, StoreError::ExportFormat { found: 99, .. }),
        "{err}"
    );

    let err = copy
        .restore(&mut to_bytes(&records[1..]).as_slice())
        .unwrap_err();
    assert!(
        matches!(err, StoreError::InvalidExport { line: 1, .. }),
        "{err}"
    );
    assert!(
        export(&copy).len() < export(&store).len(),
        "nothing written"
    );
}

#[test]
fn the_clock_moves_past_a_restored_log_but_not_far_ahead() {
    let (dir, other) = (TempDir::new(), TempDir::new());
    let mut store = source(&dir, &ManualClock::at(10_000_000));
    new_issue(&mut store);
    let file = export(&store);

    let behind = ManualClock::at(1_000);
    let mut copy = empty(&other.db(), &behind, WORKSPACE);
    copy.restore(&mut file.as_slice()).unwrap();
    let issue = new_issue(&mut copy);
    let written = copy
        .append(draft(issue, set_field("title", json!("after"))))
        .unwrap();
    assert!(
        written.hlc > Hlc::new(1_000 + 5 * 60 * 1000, u16::MAX),
        "capped at now plus the drift, then ticked: {:?}",
        written.hlc
    );
    assert!(written.hlc < Hlc::new(10_000_000, 0));
}
