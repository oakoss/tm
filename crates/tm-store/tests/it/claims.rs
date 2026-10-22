//! Claims: a take succeeds only on an unclaimed Issue, and replay applies
//! takes and releases by compare-and-set (ADR 0009).

use std::path::{Path, PathBuf};

use serde_json::json;
use tm_core::actor::SessionId;
use tm_core::op::ClaimAction;
use tm_core::{Actor, Change, Hlc, OpError};
use tm_store::{
    AgentClaims, Claim, ClaimState, Draft, Holder, Provenance, Store, StoreError, Unapplied,
    WallTime,
};
use uuid::Uuid;

fn session(id: &str) -> SessionId {
    SessionId::parse(id).unwrap()
}

use crate::support::{
    ManualClock, TempDir, claude, draft, held, insert_acked, jace, new_issue, open, remote_op,
    run_children, set_field,
};

const HOUR_MS: u64 = 3_600_000;

fn take(agent: &str, prev: Option<Uuid>) -> Change {
    Change::Claim {
        action: ClaimAction::Take {
            agent: agent.into(),
            ttl_ms: None,
            prev,
            session: None,
        },
    }
}

fn release(claim: Uuid) -> Change {
    Change::Claim {
        action: ClaimAction::Release { claim },
    }
}

#[test]
fn a_take_holds_the_issue_until_its_token_releases_it() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let claim = store.take(&claude(), issue, Some(HOUR_MS)).unwrap();
    assert_eq!(claim.expires_ms, Some(1_000 + 3_600_000));
    assert_eq!(held(&store, issue), Some(claim.clone()));
    store.release(&claude(), issue, claim.id).unwrap();
    assert_eq!(held(&store, issue), None);
    let again = store
        .take(&Actor::agent("codex", "jace"), issue, None)
        .unwrap();
    assert_eq!(held(&store, issue), Some(again));
}

#[test]
fn a_take_records_its_session_and_a_refusal_names_it() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let first = claude().in_session(session("eeeceb72-32ea-496f-ab1d-60fb6417eead"));
    let claim = store.take(&first, issue, None).unwrap();
    assert_eq!(
        claim.session.as_deref(),
        Some("eeeceb72-32ea-496f-ab1d-60fb6417eead")
    );
    assert_eq!(held(&store, issue), Some(claim.clone()));
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(held(&store, issue), Some(claim));

    let err = store
        .take(&claude().in_session(session("other")), issue, None)
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::AlreadyClaimed { holder, .. } if holder.session.as_deref().is_some_and(|s| s.ends_with("eead"))),
        "{err}"
    );
    assert!(
        err.to_string()
            .contains("held by claude (session …6417eead) for jace"),
        "{err}"
    );
}

#[test]
fn a_pulled_take_naming_a_session_outside_the_session_alphabet_does_not_fit() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let in_session = |session: &str| Change::Claim {
        action: ClaimAction::Take {
            agent: "claude".into(),
            ttl_ms: None,
            prev: None,
            session: Some(session.into()),
        },
    };
    let (blank, control, spaced, fine) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let ids = insert_all(
        &store,
        &[
            (blank, Change::Create),
            (control, Change::Create),
            (spaced, Change::Create),
            (fine, Change::Create),
            (blank, in_session(" ")),
            (control, in_session("s\u{202e}1")),
            (spaced, in_session(" s1 ")),
            (fine, in_session("s1")),
        ],
    );
    assert_eq!(store.rebuild().unwrap(), 3);
    let set_aside = store.unapplied().unwrap();
    assert_eq!(
        set_aside.iter().map(|u| u.op_id).collect::<Vec<_>>(),
        [ids[4], ids[5], ids[6]]
    );
    assert_eq!(held(&store, fine).unwrap().session.as_deref(), Some("s1"));
}

#[test]
fn a_take_must_name_the_session_writing_it() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let naming = |session: Option<&str>| Change::Claim {
        action: ClaimAction::Take {
            agent: "claude".into(),
            ttl_ms: None,
            prev: None,
            session: session.map(str::to_owned),
        },
    };
    for (writer, session) in [
        (claude().in_session(session("mine")), Some("theirs")),
        (claude().in_session(session("mine")), None),
        (claude(), Some("someone-else")),
    ] {
        let err = store
            .append(Draft {
                actor: writer.clone(),
                ..draft(issue, naming(session))
            })
            .unwrap_err();
        assert!(
            matches!(&err, StoreError::Invalid { reason, .. } if reason.contains("session writing it")),
            "{writer:?} {session:?}: {err}"
        );
    }
    assert_eq!(held(&store, issue), None);
}

#[test]
fn a_session_name_is_shown_whole_and_a_harness_id_by_its_last_eight_digits() {
    let label = |session: &str| {
        Claim {
            id: Uuid::now_v7(),
            issue: Uuid::now_v7(),
            agent: "claude".into(),
            on_behalf_of: "jace".into(),
            expires_ms: None,
            ttl_ms: None,
            session: Some(session.into()),
        }
        .taker()
    };
    // Two UUIDv7s begun the same minute share their first 8 characters.
    assert_eq!(
        label("01a16140-aeca-7572-acbf-99e715d94dfe"),
        "claude (session …15d94dfe)"
    );
    assert_eq!(
        label("eeeceb72-32ea-496f-ab1d-60fb6417eead"),
        "claude (session …6417eead)"
    );
    for name in ["6417eead", "frontend-agent-review", "backend-agent-review"] {
        assert_eq!(label(name), format!("claude (session {name})"));
    }
}

#[test]
fn a_claim_with_no_session_reads_as_taken_in_every_session() {
    let claim = |session: Option<&str>| Claim {
        id: Uuid::now_v7(),
        issue: Uuid::now_v7(),
        agent: "claude".into(),
        on_behalf_of: "jace".into(),
        expires_ms: None,
        ttl_ms: None,
        session: session.map(str::to_owned),
    };
    assert!(claim(None).taken_in(Some("s1")));
    assert!(claim(None).taken_in(None));
    assert!(claim(Some("s1")).taken_in(Some("s1")));
    assert!(!claim(Some("s1")).taken_in(Some("s2")));
    assert!(!claim(Some("s1")).taken_in(None));
}

#[test]
fn a_held_issue_is_refused_even_under_the_holders_own_agent_name() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let claim = store.take(&claude(), issue, Some(HOUR_MS)).unwrap();
    for agent in ["claude", "codex"] {
        let err = store
            .take(&Actor::agent(agent, "kim"), issue, None)
            .unwrap_err();
        let StoreError::AlreadyClaimed { holder, .. } = err else {
            panic!("{err}");
        };
        assert_eq!(*holder, claim);
    }
}

#[test]
fn an_agent_sees_only_the_claims_it_holds_for_its_human() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let (first, second, others, released) = (
        new_issue(&mut store),
        new_issue(&mut store),
        new_issue(&mut store),
        new_issue(&mut store),
    );
    let held = [
        store.take(&claude(), second, Some(HOUR_MS)).unwrap(),
        store.take(&claude(), first, None).unwrap(),
    ];
    store
        .take(&Actor::agent("claude", "kim"), others, None)
        .unwrap();
    let gone = store.take(&claude(), released, None).unwrap();
    store.release(&claude(), released, gone.id).unwrap();
    clock.advance(10);

    let mut expected = held.to_vec();
    expected.sort_by_key(|claim| claim.issue);
    assert_eq!(store.claims_by("claude", "jace").unwrap().held, expected);
    let none = store.claims_by("codex", "jace").unwrap();
    assert_eq!((none.held, none.lapsed), (vec![], vec![]));
}

#[test]
fn a_write_guarded_on_a_claim_is_refused_once_the_claim_changes() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let done = || vec![draft(issue, set_field("status", json!("done")))];
    let before = held(&store, issue);

    // Read unclaimed, then claimed before the write.
    let claim = store.take(&claude(), issue, None).unwrap();
    let err = store
        .append_if_claim(done(), issue, before.map(|claim| claim.id))
        .unwrap_err();
    assert!(
        matches!(err, StoreError::ClaimChanged(id) if id == issue),
        "{err:?}"
    );

    // Read claimed, then released before the write.
    store.release(&claude(), issue, claim.id).unwrap();
    let err = store
        .append_if_claim(done(), issue, Some(claim.id))
        .unwrap_err();
    assert!(
        matches!(err, StoreError::ClaimChanged(id) if id == issue),
        "{err:?}"
    );
    assert_eq!(
        store.issue(issue).unwrap().unwrap().fields["status"],
        json!("todo"),
        "a refused write writes nothing"
    );

    store.append_if_claim(done(), issue, None).unwrap();
    assert_eq!(
        store.issue(issue).unwrap().unwrap().fields["status"],
        json!("done")
    );
}

#[test]
fn a_store_with_the_previous_claims_table_rebuilds_on_open() {
    let dir = TempDir::new();
    let claim = {
        let mut store = open(&dir.db(), &ManualClock::at(1_000));
        let issue = new_issue(&mut store);
        let claim = store.take(&claude(), issue, None).unwrap();
        store
            .connection()
            .execute_batch(
                "DROP TABLE claims;
                 CREATE TABLE claims (issue_id BLOB PRIMARY KEY, agent TEXT NOT NULL,
                   actor TEXT NOT NULL, op_id BLOB NOT NULL, expires_ms INTEGER) STRICT;
                 PRAGMA user_version = 5;",
            )
            .unwrap();
        claim
    };
    let store = open(&dir.db(), &ManualClock::at(1_000));
    assert_eq!(held(&store, claim.issue), Some(claim));
}

#[test]
fn an_agents_claim_is_held_for_the_human_it_writes_for() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let claim = store.take(&claude(), issue, None).unwrap();
    assert_eq!(claim.on_behalf_of, "jace");
    let err = store
        .take(&Actor::agent("codex", "kim"), issue, None)
        .unwrap_err();
    assert!(err.to_string().contains("held by claude for jace"), "{err}");
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(held(&store, issue), Some(claim));
}

#[test]
fn a_release_must_name_the_current_claim() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let stranger = Uuid::now_v7();
    let err = store.release(&jace(), issue, stranger).unwrap_err();
    assert!(
        matches!(
            err,
            StoreError::NotClaimHolder {
                holder: Holder::Unclaimed,
                ..
            }
        ),
        "{err}"
    );
    let claim = store.take(&claude(), issue, None).unwrap();
    let err = store.release(&jace(), issue, stranger).unwrap_err();
    assert!(
        matches!(&err, StoreError::NotClaimHolder { holder: Holder::Current(state), .. } if **state == ClaimState::Held(claim.clone())),
        "{err}"
    );
    assert_eq!(held(&store, issue).map(|c| c.id), Some(claim.id));
}

#[test]
fn only_an_agent_takes_a_claim_and_the_take_names_it() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let err = store.take(&jace(), issue, None).unwrap_err();
    assert!(
        matches!(&err, StoreError::NoAgent { actor } if actor == "jace"),
        "{err}"
    );
    let raw = store
        .append(draft(issue, take("claude", None)))
        .unwrap_err();
    assert!(matches!(raw, StoreError::NoAgent { .. }), "{raw}");
    let other = store
        .append(Draft {
            actor: claude(),
            ..draft(issue, take("codex", None))
        })
        .unwrap_err();
    assert!(
        matches!(&other, StoreError::Invalid { reason, .. }
            if reason.contains("must name the Agent writing it")),
        "{other}"
    );
    for bad in [" ", "cl\u{7}aude"] {
        let err = store
            .take(&Actor::agent(bad, "jace"), issue, None)
            .unwrap_err();
        assert!(
            matches!(
                err,
                StoreError::Op(OpError::InvalidName { field: "actor", .. })
            ),
            "{bad:?}: {err}"
        );
    }
    assert_eq!(held(&store, issue), None);
}

#[test]
fn a_take_needs_an_expiry_and_a_live_issue() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let refused = |err: StoreError, why: &str| {
        assert!(
            matches!(&err, StoreError::Invalid { field, reason, .. }
                if field == "claim" && reason.contains(why)),
            "{err}"
        );
    };
    let max = u64::try_from(i64::MAX).unwrap();
    for (ttl, why) in [
        (Some(0), "cannot expire"),
        (Some(max), "out of range"),
        (Some(u64::MAX), "out of range"),
    ] {
        refused(store.take(&claude(), issue, ttl).unwrap_err(), why);
    }
    let named = store.append(Draft {
        actor: claude(),
        ..draft(issue, take("claude", Some(Uuid::now_v7())))
    });
    refused(named.unwrap_err(), "names a previous Claim");
    store
        .append(draft(issue, set_field("archived", json!(true))))
        .unwrap();
    refused(store.take(&claude(), issue, None).unwrap_err(), "archived");
    let missing = Uuid::now_v7();
    for err in [
        store.take(&claude(), missing, None).unwrap_err(),
        store.claim_state(missing).unwrap_err(),
    ] {
        assert!(
            matches!(err, StoreError::UnknownIssue(id) if id == missing),
            "{err}"
        );
    }
    assert_eq!(held(&store, issue), None);
}

#[test]
fn a_pulled_take_is_held_to_the_rules_that_need_no_state_but_not_to_archiving() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (blank, timed, archived, control) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let zero = Change::Claim {
        action: ClaimAction::Take {
            agent: "claude".into(),
            ttl_ms: Some(0),
            prev: None,
            session: None,
        },
    };
    let ids = insert_all(
        &store,
        &[
            (blank, Change::Create),
            (timed, Change::Create),
            (archived, Change::Create),
            (archived, set_field("archived", json!(true))),
            (blank, take(" ", None)),
            (timed, zero),
            (archived, take("claude", None)),
            // Held by now, but it does not fit before it could lose.
            (archived, take(" ", None)),
            (control, Change::Create),
            (control, take("cl\u{7}aude", None)),
        ],
    );
    assert_eq!(store.rebuild().unwrap(), 4);
    let set_aside = store.unapplied().unwrap();
    assert_eq!(
        set_aside.iter().map(|u| u.op_id).collect::<Vec<_>>(),
        [ids[4], ids[5], ids[7], ids[9]]
    );
    assert!(
        set_aside.iter().all(|u| !u.is_lost_claim()),
        "{set_aside:?}"
    );
    assert_eq!(held(&store, blank), None);
    assert_eq!(
        held(&store, archived).map(|c| c.id),
        Some(ids[6]),
        "a merge may archive an Issue someone took"
    );
}

/// Acknowledged operations in order, as sync will deliver them.
fn insert_all(store: &Store, ops: &[(Uuid, Change)]) -> Vec<Uuid> {
    ops.iter()
        .zip(1..)
        .map(|((issue, change), seq)| {
            let op = remote_op(7, *issue, change, Hlc::new(1_000 + seq, 0), 0);
            insert_acked(store, &op, i64::try_from(seq).unwrap(), 2_000);
            op.op_id
        })
        .collect()
}

#[test]
fn of_two_takes_that_saw_the_issue_unclaimed_the_first_acknowledged_wins() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let ids = insert_all(
        &store,
        &[
            (issue, Change::Create),
            (issue, take("claude", None)),
            (issue, take("codex", None)),
        ],
    );
    assert_eq!(store.rebuild().unwrap(), 1);
    let claim = held(&store, issue).unwrap();
    assert_eq!((claim.id, claim.agent.as_str()), (ids[1], "claude"));
    let lost = store.unapplied().unwrap();
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0].op_id, ids[2]);
    assert!(lost[0].reason.starts_with("lost:"), "{}", lost[0].reason);
    assert!(lost[0].reason.contains("claude"), "{}", lost[0].reason);
    assert!(
        !lost[0].reason.contains(&claim.id.to_string()),
        "the winner's token stays out of the reason: {}",
        lost[0].reason
    );
}

#[test]
fn a_take_or_release_against_a_claim_no_longer_current_loses() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let ids = insert_all(
        &store,
        &[(issue, Change::Create), (issue, take("claude", None))],
    );
    let released = remote_op(7, issue, &release(ids[1]), Hlc::new(1_010, 0), 2);
    insert_acked(&store, &released, 3, 2_000);
    // Replica 8 saw claude's Claim and takes it over, then releases one it never held.
    let stale_take = remote_op(
        8,
        issue,
        &take("codex", Some(ids[1])),
        Hlc::new(1_011, 0),
        2,
    );
    insert_acked(&store, &stale_take, 4, 2_000);
    let stale_release = remote_op(8, issue, &release(Uuid::now_v7()), Hlc::new(1_012, 0), 2);
    insert_acked(&store, &stale_release, 5, 2_000);
    assert_eq!(store.rebuild().unwrap(), 2);
    assert_eq!(held(&store, issue), None);
    let lost = store.unapplied().unwrap();
    assert_eq!(
        lost.iter().map(|u| u.op_id).collect::<Vec<_>>(),
        [stale_take.op_id, stale_release.op_id]
    );
    assert!(lost.iter().all(Unapplied::is_lost_claim), "{lost:?}");
}

#[test]
fn a_pulled_take_over_a_claim_with_no_expiry_loses() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let ids = insert_all(
        &store,
        &[(issue, Change::Create), (issue, take("claude", None))],
    );
    let over = remote_op(
        8,
        issue,
        &take("codex", Some(ids[1])),
        Hlc::new(1_010, 0),
        2,
    );
    insert_acked(&store, &over, 3, 2_000);
    assert_eq!(store.rebuild().unwrap(), 1);
    assert_eq!(held(&store, issue).map(|c| c.id), Some(ids[1]));
    let lost = store.unapplied().unwrap();
    assert_eq!(lost[0].op_id, over.op_id);
    assert!(lost[0].is_lost_claim(), "{lost:?}");
}

#[test]
fn a_pulled_take_over_a_claim_wins_only_once_its_time_is_past_the_expiry() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let lapsing = Change::Claim {
        action: ClaimAction::Take {
            agent: "claude".into(),
            ttl_ms: Some(8),
            prev: None,
            session: None,
        },
    };
    // claude's take is written at 1_002, so it lapses at 1_010.
    let ids = insert_all(&store, &[(issue, Change::Create), (issue, lapsing)]);
    let early = remote_op(
        8,
        issue,
        &take("codex", Some(ids[1])),
        Hlc::new(1_009, 0),
        2,
    );
    insert_acked(&store, &early, 3, 2_000);
    let late = remote_op(
        9,
        issue,
        &take("gemini", Some(ids[1])),
        Hlc::new(1_010, 0),
        2,
    );
    insert_acked(&store, &late, 4, 2_000);
    assert_eq!(store.rebuild().unwrap(), 1);
    let lost = store.unapplied().unwrap();
    assert_eq!(lost[0].op_id, early.op_id);
    assert!(lost[0].is_lost_claim(), "{lost:?}");
    let claim = held(&store, issue).unwrap();
    assert_eq!((claim.id, claim.agent.as_str()), (late.op_id, "gemini"));
}

#[test]
fn a_lapsed_claim_stops_holding_and_another_agent_takes_it_over() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    let lapsing = store.take(&claude(), issue, Some(HOUR_MS)).unwrap();
    let codex = Actor::agent("codex", "jace");
    clock.set(lapsing.expires_ms.unwrap() - 1);
    assert_eq!(held(&store, issue), Some(lapsing.clone()));
    assert_eq!(store.claims_by("claude", "jace").unwrap().held.len(), 1);
    let err = store.take(&codex, issue, None).unwrap_err();
    assert!(matches!(err, StoreError::AlreadyClaimed { .. }), "{err}");

    clock.advance(1);
    assert_eq!(held(&store, issue), None);
    assert_eq!(store.claims_by("claude", "jace").unwrap().held, []);
    let over = store.take(&codex, issue, None).unwrap();
    assert_eq!(held(&store, issue), Some(over.clone()));
    let err = store.release(&claude(), issue, lapsing.id).unwrap_err();
    assert!(
        matches!(&err, StoreError::NotClaimHolder { holder: Holder::TookOver(held), .. } if **held == over),
        "{err}"
    );
    assert!(
        err.to_string()
            .ends_with("; it lapsed and codex took the Issue over for jace"),
        "{err}"
    );

    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(held(&store, issue), Some(over.clone()));
    store.undo(&jace(), over.id).unwrap();
    assert_eq!(held(&store, issue), None, "undo restores no lapsed Claim");
}

#[test]
fn a_take_naming_no_claim_or_the_wrong_one_is_refused_over_a_lapsed_claim() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    store.take(&claude(), issue, Some(HOUR_MS)).unwrap();
    clock.advance(i64::try_from(HOUR_MS).unwrap());
    for prev in [None, Some(Uuid::now_v7())] {
        let err = store
            .append(Draft {
                actor: Actor::agent("codex", "jace"),
                ..draft(issue, take("codex", prev))
            })
            .unwrap_err();
        assert!(
            matches!(&err, StoreError::Invalid { reason, .. } if reason.contains("lapsed")),
            "{prev:?}: {err}"
        );
    }
}

#[test]
fn an_imported_claim_is_refused_before_the_batch_s_times() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let at = |utc_ms| Provenance {
        source: "test".into(),
        at: WallTime {
            utc_ms,
            offset_min: 0,
        },
    };
    let claim = Draft {
        actor: claude(),
        ..draft(issue, take("claude", None))
    };
    let err = store
        .append_imported(vec![
            (claim, at(500)),
            (draft(issue, set_field("title", json!("u"))), at(-1)),
        ])
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid { reason, .. } if reason == "cannot be imported"),
        "{err}"
    );
}

#[test]
fn a_claim_is_never_imported() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let provenance = Provenance {
        source: "test".into(),
        at: WallTime {
            utc_ms: 500,
            offset_min: 0,
        },
    };
    let draft = Draft {
        actor: claude(),
        ..draft(issue, take("claude", None))
    };
    let err = store
        .append_imported(vec![(draft, provenance)])
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid { reason, .. } if reason == "cannot be imported"),
        "{err}"
    );
    assert_eq!(held(&store, issue), None);
}

#[test]
fn a_lapsed_claim_reads_as_lapsed_until_its_holder_releases_it() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    let lapsing = store.take(&claude(), issue, Some(HOUR_MS)).unwrap();
    assert_eq!(
        store.claim_state(issue).unwrap(),
        Some(ClaimState::Held(lapsing.clone()))
    );
    clock.advance(i64::try_from(HOUR_MS).unwrap());
    assert_eq!(
        store.claim_state(issue).unwrap(),
        Some(ClaimState::Lapsed(lapsing.clone()))
    );
    assert_eq!(
        store.claims_by("claude", "jace").unwrap(),
        AgentClaims {
            held: Vec::new(),
            lapsed: vec![lapsing.clone()],
            at_ms: store.utc_ms().unwrap(),
        }
    );
    let err = store.release(&claude(), issue, Uuid::now_v7()).unwrap_err();
    assert!(
        matches!(&err, StoreError::NotClaimHolder { holder: Holder::Current(state), .. } if **state == ClaimState::Lapsed(lapsing.clone())),
        "{err}"
    );
    assert!(
        err.to_string()
            .ends_with("; the Claim by claude for jace on it has lapsed"),
        "{err}"
    );
    assert!(matches!(
        store.claim_state(Uuid::now_v7()),
        Err(StoreError::UnknownIssue(_))
    ));
    let edit = || vec![draft(issue, set_field("title", json!("u")))];
    let err = store.append_if_claim(edit(), issue, None).unwrap_err();
    assert!(matches!(err, StoreError::ClaimChanged(_)), "{err}");
    store
        .append_if_claim(edit(), issue, Some(lapsing.id))
        .unwrap();
    store.release(&claude(), issue, lapsing.id).unwrap();
    assert_eq!(store.claim_state(issue).unwrap(), None);
    let none = store.claims_by("claude", "jace").unwrap();
    assert_eq!((none.held, none.lapsed), (vec![], vec![]));
    assert_eq!(store.rebuild().unwrap(), 0);
    let codex = store
        .take(&Actor::agent("codex", "jace"), issue, None)
        .unwrap();
    assert_eq!(held(&store, issue), Some(codex));
}

#[test]
fn a_pending_take_loses_to_one_the_server_acknowledged_first() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    insert_all(&store, &[(issue, Change::Create)]);
    assert_eq!(store.rebuild().unwrap(), 0);
    let mine = store.take(&claude(), issue, None).unwrap();
    let theirs = remote_op(8, issue, &take("codex", None), Hlc::new(1_005, 0), 1);
    insert_acked(&store, &theirs, 2, 2_000);
    assert_eq!(store.rebuild().unwrap(), 1);
    assert_eq!(held(&store, issue).map(|c| c.id), Some(theirs.op_id));
    assert_eq!(store.unapplied().unwrap()[0].op_id, mine.id);
}

#[test]
fn a_lost_take_or_release_is_not_activity() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    insert_all(
        &store,
        &[
            (issue, Change::Create),
            (issue, take("claude", None)),
            (issue, take("codex", None)),
            (issue, release(Uuid::now_v7())),
        ],
    );
    assert_eq!(store.rebuild().unwrap(), 2);
    let updated = store.issue(issue).unwrap().unwrap().fields["updated"].clone();
    assert_eq!(updated, json!(1_002));
}

/// Set in child processes: the store, the shared Issue, the Agent name to take
/// it under, and the file to report the outcome in.
const CHILD_DB: &str = "TM_STORE_TEST_CLAIM_DB";
const CHILD_ISSUE: &str = "TM_STORE_TEST_CLAIM_ISSUE";
const CHILD_AGENT: &str = "TM_STORE_TEST_CLAIM_AGENT";
const CHILD_REPORT: &str = "TM_STORE_TEST_CLAIM_REPORT";

const TAKERS: usize = 50;

#[test]
fn of_fifty_processes_taking_one_issue_exactly_one_wins() {
    if let (Ok(db), Ok(issue), Ok(agent), Ok(report)) = (
        std::env::var(CHILD_DB),
        std::env::var(CHILD_ISSUE),
        std::env::var(CHILD_AGENT),
        std::env::var(CHILD_REPORT),
    ) {
        take_as_child(
            Path::new(&db),
            Uuid::parse_str(&issue).unwrap(),
            &agent,
            Path::new(&report),
        );
        return;
    }
    for shared_name in [false, true] {
        race(shared_name);
    }
}

fn race(shared_name: bool) {
    let dir = TempDir::new();
    let mut store = Store::open(&dir.db()).unwrap();
    let (project, _) = store.create_project(&jace(), "CPL", "Core").unwrap();
    let (issue, _) = store.create_issue(&jace(), project, "task", "t").unwrap();
    drop(store);
    let reports: Vec<PathBuf> = (0..TAKERS)
        .map(|i| dir.path().join(format!("taker-{i}")))
        .collect();
    run_children(
        "claims::of_fifty_processes_taking_one_issue_exactly_one_wins",
        TAKERS,
        |i, command| {
            let agent = if shared_name {
                "claude".to_owned()
            } else {
                format!("agent-{i}")
            };
            command
                .env(CHILD_DB, dir.db())
                .env(CHILD_ISSUE, issue.to_string())
                .env(CHILD_AGENT, agent)
                .env(CHILD_REPORT, &reports[i]);
        },
    );
    let winner = held(&Store::open(&dir.db()).unwrap(), issue).unwrap();
    let outcomes: Vec<String> = reports
        .iter()
        .map(|report| std::fs::read_to_string(report).unwrap())
        .collect();
    let won = outcomes.iter().filter(|o| o.starts_with("won ")).count();
    let refused = outcomes
        .iter()
        .filter(|o| **o == format!("held {}", winner.id))
        .count();
    assert_eq!(
        (won, refused),
        (1, TAKERS - 1),
        "shared name: {shared_name}"
    );
    assert!(outcomes.contains(&format!("won {}", winner.id)));
}

fn take_as_child(db: &Path, issue: Uuid, agent: &str, report: &Path) {
    let mut store = Store::open(db).unwrap();
    let outcome = match store.take(&Actor::agent(agent, "jace"), issue, None) {
        Ok(claim) => {
            assert_eq!(claim.agent, agent, "a take returns its own Claim");
            format!("won {}", claim.id)
        }
        Err(StoreError::AlreadyClaimed { holder, .. }) => format!("held {}", holder.id),
        Err(err) => panic!("{err}"),
    };
    std::fs::write(report, outcome).unwrap();
}
