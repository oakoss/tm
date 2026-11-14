//! Protected fields: while a session holds a live Claim, another Agent's
//! write to its Status or description is refused (ADR 0009).

use serde_json::json;
use tm_core::actor::SessionId;
use tm_core::op::ClaimAction;
use tm_core::{Actor, Change};
use tm_store::{
    Claim, Draft, Lineage, Provenance, Store, StoreError, WallTime, WriteOptions, Written,
};
use uuid::Uuid;

use crate::support::{
    ManualClock, TempDir, claude, draft, field, jace, new_issue, open, set_field,
};

const HOUR_MS: u64 = 3_600_000;

fn in_session(session: &str) -> Actor {
    claude().in_session(SessionId::parse(session).unwrap())
}

/// The Claims the write went past.
fn write(
    store: &mut Store,
    writer: &Actor,
    issue: Uuid,
    name: &str,
) -> Result<Vec<Claim>, StoreError> {
    store
        .append_all(vec![Draft {
            actor: writer.clone(),
            ..draft(issue, set_field(name, json!("in_review")))
        }])
        .map(passed)
}

fn passed<T>(written: Written<T>) -> Vec<Claim> {
    let mut passed = Vec::new();
    written.ops_into(&mut passed);
    passed
}

/// A begin that also sets `held`'s Status as jace.
fn and_held(held: Uuid) -> impl FnOnce(&Lineage) -> (Vec<Draft>, ()) {
    move |_| {
        (
            vec![draft(held, set_field("status", json!("in_review")))],
            (),
        )
    }
}

/// A store whose Issue session s1 holds for an hour.
fn held_by_s1(dir: &TempDir, clock: &ManualClock) -> (Store, Uuid) {
    let mut store = open(&dir.db(), clock);
    let issue = new_issue(&mut store);
    store.take(&in_session("s1"), issue, Some(HOUR_MS)).unwrap();
    (store, issue)
}

#[test]
fn another_agent_session_is_refused_a_protected_field_and_names_it() {
    let dir = TempDir::new();
    let (mut store, issue) = held_by_s1(&dir, &ManualClock::at(1_000));
    for name in ["status", "description"] {
        let err = write(&mut store, &in_session("s2"), issue, name).unwrap_err();
        assert!(
            matches!(&err, StoreError::Protected { field, holder, .. }
                if field == name && holder.session.as_deref() == Some("s1")),
            "{err}"
        );
    }
    assert_eq!(field(&store, issue, "status"), Some(json!("todo")));
    assert_eq!(
        write(&mut store, &in_session("s2"), issue, "title").unwrap(),
        []
    );
    assert_eq!(
        write(&mut store, &in_session("s1"), issue, "status").unwrap(),
        []
    );
}

#[test]
fn every_writer_in_a_batch_is_judged_and_only_a_take_or_release_exempts() {
    let dir = TempDir::new();
    let (mut store, issue) = held_by_s1(&dir, &ManualClock::at(1_000));
    let claim = store.claim_state(issue).unwrap().unwrap().claim().id;
    let status = |actor: &Actor| Draft {
        actor: actor.clone(),
        ..draft(issue, set_field("status", json!("in_review")))
    };
    let renew = Draft {
        actor: in_session("s1"),
        ..draft(
            issue,
            Change::Claim {
                action: ClaimAction::Renew {
                    claim,
                    ttl_ms: HOUR_MS,
                },
            },
        )
    };
    for batch in [
        vec![status(&in_session("s1")), status(&in_session("s2"))],
        vec![status(&jace()), status(&in_session("s2"))],
        vec![renew, status(&in_session("s2"))],
    ] {
        let err = store.append_all(batch).unwrap_err();
        assert!(matches!(err, StoreError::Protected { .. }), "{err}");
    }
}

#[test]
fn a_human_or_a_forced_write_goes_past_the_claim_and_is_reported() {
    let dir = TempDir::new();
    let (mut store, issue) = held_by_s1(&dir, &ManualClock::at(1_000));
    let went_past = write(&mut store, &jace(), issue, "description").unwrap();
    assert_eq!(went_past.len(), 1);
    assert_eq!(went_past[0].session.as_deref(), Some("s1"));
    assert_eq!(
        write(&mut store, &jace(), issue, "description")
            .unwrap()
            .len(),
        1,
        "a write reports only the Claims it went past itself"
    );

    let forced = Draft {
        actor: in_session("s2"),
        ..draft(issue, set_field("status", json!("in_review")))
    };
    let force = WriteOptions {
        force: true,
        ..WriteOptions::default()
    };
    assert_eq!(passed(store.write(vec![forced], &force).unwrap()).len(), 1);
    let err = write(&mut store, &in_session("s2"), issue, "description").unwrap_err();
    assert!(
        matches!(err, StoreError::Protected { .. }),
        "force covers only its own write: {err}"
    );
}

#[test]
fn a_lapsed_claim_protects_nothing() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let (mut store, issue) = held_by_s1(&dir, &clock);
    clock.advance(i64::try_from(HOUR_MS).unwrap());
    assert_eq!(
        write(&mut store, &in_session("s2"), issue, "status").unwrap(),
        []
    );
}

#[test]
fn a_batch_releasing_the_claim_is_judged_by_the_release() {
    let dir = TempDir::new();
    let (mut store, issue) = held_by_s1(&dir, &ManualClock::at(1_000));
    let claim = store.claim_state(issue).unwrap().unwrap().claim().id;
    let s2 = in_session("s2");
    let written = store
        .append_all(vec![
            Draft {
                actor: s2.clone(),
                ..draft(issue, set_field("status", json!("done")))
            },
            Draft {
                actor: s2,
                ..draft(
                    issue,
                    Change::Claim {
                        action: ClaimAction::Release { claim },
                    },
                )
            },
        ])
        .unwrap();
    assert_eq!(passed(written), []);
    assert_eq!(field(&store, issue, "status"), Some(json!("done")));
}

#[test]
fn an_undo_returns_the_claims_its_write_goes_past_rolled_back_or_not() {
    let dir = TempDir::new();
    let (mut store, issue) = held_by_s1(&dir, &ManualClock::at(1_000));
    let mut first = Vec::new();
    let batch = store
        .append(draft(issue, set_field("description", json!("first"))))
        .unwrap()
        .ops_into(&mut first)
        .op_id;
    let options = WriteOptions::default();
    for commit in [false, true] {
        let (_, written) = store.undo_with(&jace(), batch, &options, commit).unwrap();
        assert_eq!(passed(written).len(), 1, "commit {commit}");
    }
}

#[test]
fn a_take_or_next_returns_the_claims_its_begun_drafts_go_past() {
    let dir = TempDir::new();
    let (mut store, held) = held_by_s1(&dir, &ManualClock::at(1_000));
    let taken = new_issue(&mut store);
    let taking = store
        .take_with(&in_session("s2"), taken, Some(HOUR_MS), and_held(held))
        .unwrap();
    assert_eq!(passed(taking.written).len(), 1, "take_with");

    let next = new_issue(&mut store);
    let started = store
        .take_next(&jace(), &[next], None, and_held(held))
        .unwrap()
        .unwrap();
    assert_eq!(started.issue, next);
    assert_eq!(passed(started.written).len(), 1, "take_next");
}

#[test]
fn an_import_records_what_happened_elsewhere_unrefused() {
    let dir = TempDir::new();
    let (mut store, issue) = held_by_s1(&dir, &ManualClock::at(1_000));
    let provenance = Provenance {
        source: "test".into(),
        at: WallTime {
            utc_ms: 500,
            offset_min: 0,
        },
    };
    let imported = Draft {
        actor: in_session("s2"),
        ..draft(issue, set_field("description", json!("from elsewhere")))
    };
    store.append_imported(vec![(imported, provenance)]).unwrap();
    assert_eq!(
        field(&store, issue, "description"),
        Some(json!("from elsewhere"))
    );
}
