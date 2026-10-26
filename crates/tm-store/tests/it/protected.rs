//! Protected fields: while a session holds a live Claim, another Agent's
//! write to its Status or description is refused (ADR 0009).

use serde_json::json;
use tm_core::actor::SessionId;
use tm_core::op::ClaimAction;
use tm_core::{Actor, Change};
use tm_store::{Draft, Provenance, Store, StoreError, WallTime};
use uuid::Uuid;

use crate::support::{
    ManualClock, TempDir, claude, draft, field, jace, new_issue, open, set_field,
};

const HOUR_MS: u64 = 3_600_000;

fn in_session(session: &str) -> Actor {
    claude().in_session(SessionId::parse(session).unwrap())
}

fn write(store: &mut Store, writer: &Actor, issue: Uuid, name: &str) -> Result<(), StoreError> {
    store
        .append_all(vec![Draft {
            actor: writer.clone(),
            ..draft(issue, set_field(name, json!("in_review")))
        }])
        .map(drop)
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
    write(&mut store, &in_session("s2"), issue, "title").unwrap();
    write(&mut store, &in_session("s1"), issue, "status").unwrap();
    assert_eq!(store.passed_claims(), []);
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
    write(&mut store, &jace(), issue, "description").unwrap();
    let passed = store.passed_claims();
    assert_eq!(passed.len(), 1);
    assert_eq!(passed[0].session.as_deref(), Some("s1"));
    assert_eq!(store.passed_claims(), [], "reported once");

    store.force_past_claims();
    write(&mut store, &in_session("s2"), issue, "status").unwrap();
    assert_eq!(store.passed_claims().len(), 1);
}

#[test]
fn a_lapsed_claim_protects_nothing() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let (mut store, issue) = held_by_s1(&dir, &clock);
    clock.advance(i64::try_from(HOUR_MS).unwrap());
    write(&mut store, &in_session("s2"), issue, "status").unwrap();
    assert_eq!(store.passed_claims(), []);
}

#[test]
fn a_batch_releasing_the_claim_is_judged_by_the_release() {
    let dir = TempDir::new();
    let (mut store, issue) = held_by_s1(&dir, &ManualClock::at(1_000));
    let claim = store.claim_state(issue).unwrap().unwrap().claim().id;
    let s2 = in_session("s2");
    store
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
    assert_eq!(field(&store, issue, "status"), Some(json!("done")));
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
