//! Renewing Claims: a session's writes and heartbeats keep its Claims, and a
//! renewal applies by compare-and-set on the Claim it names (ADR 0009).

use serde_json::json;
use tm_core::actor::SessionId;
use tm_core::op::ClaimAction;
use tm_core::{Actor, Change, Hlc, Op};
use tm_store::{Draft, Store, StoreError};
use uuid::Uuid;

use crate::support::{
    ManualClock, TempDir, claude, draft, held, insert_acked, jace, new_issue, open, remote_op,
    set_field,
};

const HOUR_MS: u64 = 3_600_000;
const HOUR: i64 = 3_600_000;
const NONE: [Uuid; 0] = [];

fn in_session(session: &str) -> Actor {
    claude().in_session(SessionId::parse(session).unwrap())
}

/// `writer` retitles `issue`, returning the batch's operations.
fn edit(store: &mut Store, writer: &Actor, issue: Uuid) -> Vec<Op> {
    store
        .append_all(vec![Draft {
            actor: writer.clone(),
            ..draft(issue, set_field("title", json!(Uuid::now_v7().to_string())))
        }])
        .unwrap()
}

fn renews(ops: &[Op]) -> Vec<Uuid> {
    ops.iter()
        .filter(|op| {
            matches!(
                op.change().unwrap(),
                Change::Claim {
                    action: ClaimAction::Renew { .. }
                }
            )
        })
        .map(|op| op.subject)
        .collect()
}

#[test]
fn a_sessions_write_renews_its_claims_once_a_third_of_their_time_has_passed() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let (held_one, written) = (new_issue(&mut store), new_issue(&mut store));
    let s1 = in_session("s1");
    let claim = store.take(&s1, held_one, Some(HOUR_MS)).unwrap();

    clock.set(1_000 + HOUR / 3 - 1);
    assert_eq!(renews(&edit(&mut store, &s1, written)), NONE, "not yet due");
    clock.advance(1);
    assert_eq!(
        renews(&edit(&mut store, &s1, written)),
        [held_one],
        "a write elsewhere renews it"
    );
    let renewed = held(&store, held_one).unwrap();
    assert_eq!(renewed.id, claim.id);
    assert_eq!(
        store
            .connection()
            .query_row(
                "SELECT updated_ms FROM issues WHERE id = ?1",
                [held_one.as_bytes()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1_000,
        "a renewal is not activity"
    );
    assert_eq!(renewed.expires_ms, Some(1_000 + HOUR / 3 + HOUR));
    assert_eq!(
        renews(&edit(&mut store, &s1, written)),
        NONE,
        "renewed just now"
    );
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(held(&store, held_one), Some(renewed));
}

#[test]
fn another_session_or_agent_writing_renews_nothing() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let (held_one, written) = (new_issue(&mut store), new_issue(&mut store));
    store
        .take(&in_session("s1"), held_one, Some(HOUR_MS))
        .unwrap();
    clock.advance(HOUR / 2);
    for writer in [
        in_session("s2"),
        claude(),
        Actor::agent("codex", "jace").in_session(SessionId::parse("s1").unwrap()),
    ] {
        assert_eq!(
            renews(&edit(&mut store, &writer, written)),
            NONE,
            "{writer:?}"
        );
    }
}

#[test]
fn a_batch_of_several_writers_renews_nothing() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let (held_one, written) = (new_issue(&mut store), new_issue(&mut store));
    let s1 = in_session("s1");
    store.take(&s1, held_one, Some(HOUR_MS)).unwrap();
    clock.advance(HOUR / 2);
    let ops = store
        .append_all(vec![
            Draft {
                actor: s1,
                ..draft(written, set_field("title", json!("a")))
            },
            draft(written, set_field("title", json!("b"))),
        ])
        .unwrap();
    assert_eq!(renews(&ops), NONE);
}

#[test]
fn a_lapsed_claim_no_one_took_resumes_on_its_holders_write() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    let s1 = in_session("s1");
    let claim = store.take(&s1, issue, Some(HOUR_MS)).unwrap();
    clock.advance(HOUR + 5);
    assert_eq!(held(&store, issue), None, "lapsed");
    assert_eq!(renews(&edit(&mut store, &s1, issue)), [issue]);
    let resumed = held(&store, issue).unwrap();
    assert_eq!(
        (resumed.id, resumed.expires_ms),
        (claim.id, Some(1_000 + HOUR + 5 + HOUR))
    );
}

#[test]
fn a_claim_taken_over_is_not_renewed_and_its_old_holder_is_told() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    let s1 = in_session("s1");
    store.take(&s1, issue, Some(HOUR_MS)).unwrap();
    clock.advance(HOUR);
    let codex = store
        .take(&Actor::agent("codex", "jace"), issue, Some(HOUR_MS))
        .unwrap();
    let ops = edit(&mut store, &s1, issue);
    assert_eq!(renews(&ops), NONE);
    assert_eq!(
        store.taken_over_from(&s1, Some(ops[0].op_id)).unwrap(),
        std::slice::from_ref(&codex)
    );
    let other = new_issue(&mut store);
    let elsewhere = edit(&mut store, &s1, other);
    assert_eq!(
        store
            .taken_over_from(&s1, Some(elsewhere[0].op_id))
            .unwrap(),
        [],
        "a batch names only the Issues it wrote"
    );
    assert_eq!(store.taken_over_from(&s1, None).unwrap(), [codex]);
    assert_eq!(
        store
            .taken_over_from(&in_session("s2"), Some(ops[0].op_id))
            .unwrap(),
        [],
        "only the session it was taken from"
    );
    let theirs = edit(&mut store, &Actor::agent("codex", "jace"), issue);
    assert_eq!(
        store
            .taken_over_from(&Actor::agent("codex", "jace"), Some(theirs[0].op_id))
            .unwrap(),
        []
    );

    let own = new_issue(&mut store);
    store.take(&s1, own, Some(HOUR_MS)).unwrap();
    clock.advance(HOUR);
    store.take(&s1, own, Some(HOUR_MS)).unwrap();
    let ops = edit(&mut store, &s1, own);
    assert_eq!(
        store.taken_over_from(&s1, Some(ops[0].op_id)).unwrap(),
        [],
        "taken again by the session that let it lapse"
    );
}

#[test]
fn a_heartbeat_renews_only_what_is_due_and_writes_nothing_else() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let (one, two, forever) = (
        new_issue(&mut store),
        new_issue(&mut store),
        new_issue(&mut store),
    );
    let s1 = in_session("s1");
    store.take(&s1, one, Some(HOUR_MS)).unwrap();
    store.take(&s1, forever, None).unwrap();
    clock.advance(HOUR / 4);
    store.take(&s1, two, Some(HOUR_MS)).unwrap();
    assert_eq!(store.heartbeat(&s1, None).unwrap(), [], "nothing due");
    clock.set(1_000 + HOUR / 3);
    assert_eq!(renews(&store.heartbeat(&s1, None).unwrap()), [one]);
    assert_eq!(
        renews(&store.heartbeat(&s1, Some(two)).unwrap()),
        [two],
        "one Issue's is renewed before it is due"
    );
    assert_eq!(store.heartbeat(&s1, Some(forever)).unwrap(), []);
    clock.advance(HOUR);
    let mut both = renews(&store.heartbeat(&s1, None).unwrap());
    both.sort();
    let mut expected = vec![one, two];
    expected.sort();
    assert_eq!(both, expected, "lapsed ones resume");
    assert_eq!(store.heartbeat(&in_session("s2"), None).unwrap(), []);
}

#[test]
fn only_the_session_that_took_a_claim_renews_it() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    let claim = store.take(&in_session("s1"), issue, Some(HOUR_MS)).unwrap();
    let renew = Change::Claim {
        action: ClaimAction::Renew {
            claim: claim.id,
            ttl_ms: HOUR_MS,
        },
    };
    for writer in [in_session("s2"), jace()] {
        let err = store
            .append(Draft {
                actor: writer.clone(),
                ..draft(issue, renew.clone())
            })
            .unwrap_err();
        assert!(
            matches!(&err, StoreError::Invalid { reason, .. } if reason.contains("only by the session that took it")),
            "{writer:?}: {err}"
        );
    }
    let err = store
        .append(Draft {
            actor: in_session("s1"),
            ..draft(
                issue,
                Change::Claim {
                    action: ClaimAction::Renew {
                        claim: Uuid::now_v7(),
                        ttl_ms: HOUR_MS,
                    },
                },
            )
        })
        .unwrap_err();
    assert!(matches!(err, StoreError::NotClaimHolder { .. }), "{err}");
}

#[test]
fn a_claim_that_never_lapses_is_not_renewed_into_one_that_does() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    let s1 = in_session("s1");
    let claim = store.take(&s1, issue, None).unwrap();
    let renew = Change::Claim {
        action: ClaimAction::Renew {
            claim: claim.id,
            ttl_ms: HOUR_MS,
        },
    };
    let err = store
        .append(Draft {
            actor: s1.clone(),
            ..draft(issue, renew.clone())
        })
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid { reason, .. } if reason.contains("does not expire")),
        "{err}"
    );

    let pulled = remote_op(7, issue, &renew, Hlc::new(5_000, 0), 0);
    insert_acked(&store, &pulled, 1, 6_000);
    assert_eq!(store.rebuild().unwrap(), 1);
    let set_aside = store.unapplied().unwrap();
    assert_eq!(set_aside[0].op_id, pulled.op_id);
    assert!(!set_aside[0].is_lost_claim(), "{set_aside:?}");
    assert_eq!(held(&store, issue).unwrap().expires_ms, None);
}

#[test]
fn the_longest_claim_does_not_overflow_the_renewal_check() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let (issue, written) = (new_issue(&mut store), new_issue(&mut store));
    let s1 = in_session("s1");
    store
        .take(&s1, issue, Some(u64::try_from(i64::MAX - 1_000).unwrap()))
        .unwrap();
    assert_eq!(renews(&edit(&mut store, &s1, written)), NONE);
}

#[test]
fn every_claim_is_listed_by_holder_and_split_at_one_instant() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issues: Vec<Uuid> = (0..4).map(|_| new_issue(&mut store)).collect();
    let codex = Actor::agent("codex", "jace");
    store.take(&codex, issues[0], Some(HOUR_MS)).unwrap();
    store
        .take(&in_session("s2"), issues[1], Some(HOUR_MS))
        .unwrap();
    store.take(&in_session("s1"), issues[2], Some(1)).unwrap();
    store
        .take(&in_session("s1"), issues[3], Some(HOUR_MS))
        .unwrap();
    clock.advance(5);
    let claims = store.claims().unwrap();
    assert_eq!(claims.at_ms, 1_005);
    let held: Vec<(&str, Option<&str>, Uuid)> = claims
        .held
        .iter()
        .map(|claim| (claim.agent.as_str(), claim.session.as_deref(), claim.issue))
        .collect();
    assert_eq!(
        held,
        [
            ("claude", Some("s1"), issues[3]),
            ("claude", Some("s2"), issues[1]),
            ("codex", None, issues[0]),
        ]
    );
    assert_eq!(
        claims
            .lapsed
            .iter()
            .map(|claim| claim.issue)
            .collect::<Vec<_>>(),
        [issues[2]]
    );
}

#[test]
fn a_take_with_keeps_the_sessions_held_claim_and_takes_a_lapsed_one_again() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    let s1 = in_session("s1");
    let none = |_: &[tm_store::IssueView]| (Vec::new(), ());
    let first = store.take_with(&s1, issue, Some(HOUR_MS), none).unwrap();
    assert!(!first.kept);
    let again = store.take_with(&s1, issue, Some(HOUR_MS), none).unwrap();
    assert!(again.kept, "held by this session");
    assert_eq!(again.claim.id, first.claim.id);
    assert_eq!(again.ops, []);
    assert!(matches!(
        store.take_with(&in_session("s2"), issue, Some(HOUR_MS), none),
        Err(StoreError::AlreadyClaimed { .. })
    ));
    clock.advance(HOUR);
    let lapsed = store.take_with(&s1, issue, Some(HOUR_MS), none).unwrap();
    assert!(!lapsed.kept, "a lapsed one is taken again");
    assert_ne!(lapsed.claim.id, first.claim.id);
}

#[test]
fn a_take_with_judges_the_held_claim_at_the_instant_it_writes() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    let s1 = in_session("s1");
    let expires = store.take(&s1, issue, Some(HOUR_MS)).unwrap().expires_ms;
    clock.set(expires.unwrap() - 1);
    clock.step_each_read(1);
    let retitle = |_: &[tm_store::IssueView]| {
        let again = Draft {
            actor: s1.clone(),
            ..draft(issue, set_field("title", json!("again")))
        };
        (vec![again], ())
    };
    let started = store.take_with(&s1, issue, Some(HOUR_MS), retitle).unwrap();
    assert!(started.kept);
    let retitled = started.ops.iter().any(
        |op| matches!(op.change().unwrap(), Change::SetField { field, .. } if field == "title"),
    );
    assert!(retitled, "a Claim kept still writes extend's drafts");
    assert!(
        started.ops.iter().all(|op| Some(op.utc_ms) < expires),
        "a Claim kept is unlapsed when the batch is written"
    );
}

#[test]
fn undoing_a_write_leaves_its_renewal() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let (held_one, written) = (new_issue(&mut store), new_issue(&mut store));
    let s1 = in_session("s1");
    store.take(&s1, held_one, Some(HOUR_MS)).unwrap();
    clock.advance(HOUR / 2);
    let ops = edit(&mut store, &s1, written);
    assert_eq!(renews(&ops), [held_one]);
    let renewed = held(&store, held_one).unwrap();
    store.undo(&jace(), ops[0].op_id).unwrap();
    assert_eq!(held(&store, held_one), Some(renewed));
}

/// `ops` written by Replica 7 and acknowledged in order.
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

fn take(agent: &str, ttl_ms: Option<u64>, prev: Option<Uuid>) -> Change {
    Change::Claim {
        action: ClaimAction::Take {
            agent: agent.into(),
            ttl_ms,
            prev,
            session: None,
        },
    }
}

#[test]
fn of_a_pulled_renewal_and_takeover_the_one_ordered_first_wins() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (renewed, taken) = (Uuid::now_v7(), Uuid::now_v7());
    // Each take is written at 1_003 and 1_004 and lapses 8 ms later.
    let mut ops = vec![
        (renewed, Change::Create),
        (taken, Change::Create),
        (renewed, take("claude", Some(8), None)),
        (taken, take("claude", Some(8), None)),
    ];
    let ids = insert_all(&store, &ops);
    let renew = |claim| Change::Claim {
        action: ClaimAction::Renew { claim, ttl_ms: 8 },
    };
    // On `renewed` the renewal stands alone; on `taken` the takeover at 1_012
    // is ordered first, so the renewal after it loses.
    ops = vec![
        (renewed, renew(ids[2])),
        (taken, take("codex", Some(8), Some(ids[3]))),
    ];
    for (at, seq, (issue, change)) in [(1_005, 5, &ops[0]), (1_012, 6, &ops[1])] {
        let op = remote_op(8, *issue, change, Hlc::new(at, 0), 0);
        insert_acked(&store, &op, seq, 2_000);
    }
    let late = remote_op(9, taken, &renew(ids[3]), Hlc::new(1_013, 0), 0);
    insert_acked(&store, &late, 30, 2_000);
    store.rebuild().unwrap();
    let kept = store.claim_state(renewed).unwrap().unwrap();
    assert_eq!(kept.claim().id, ids[2]);
    assert_eq!(kept.claim().expires_ms, Some(1_005 + 8));
    assert_eq!(
        store.claim_state(taken).unwrap().unwrap().claim().agent,
        "codex"
    );
    let lost = store.unapplied().unwrap();
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert_eq!(lost[0].op_id, late.op_id);
    assert!(lost[0].is_lost_claim(), "{lost:?}");
}

#[test]
fn a_pulled_renewal_of_no_duration_does_not_fit() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let ids = insert_all(
        &store,
        &[
            (issue, Change::Create),
            (issue, take("claude", Some(HOUR_MS), None)),
            (
                issue,
                Change::Claim {
                    action: ClaimAction::Renew {
                        claim: Uuid::nil(),
                        ttl_ms: 0,
                    },
                },
            ),
        ],
    );
    assert_eq!(store.rebuild().unwrap(), 1);
    let set_aside = store.unapplied().unwrap();
    assert_eq!(set_aside[0].op_id, ids[2]);
    assert!(!set_aside[0].is_lost_claim(), "{set_aside:?}");
}
