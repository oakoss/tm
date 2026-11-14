//! Path Claims: the files a Claim covers, which another session is warned
//! before editing (TM-78).

use tm_core::actor::SessionId;
use tm_core::op::ClaimAction;
use tm_core::{Actor, Change, Hlc};
use tm_store::{Store, StoreError, Taking};
use uuid::Uuid;

use crate::support::{ManualClock, TempDir, held, insert_acked, new_issue, open, remote_op};

const HOUR_MS: u64 = 3_600_000;

fn agent(session: &str) -> Actor {
    Actor::agent("claude", "jace").in_session(SessionId::parse(session).unwrap())
}

fn covering(paths: &[&str]) -> Taking {
    Taking {
        ttl_ms: Some(HOUR_MS),
        branch: None,
        paths: paths.iter().map(|&path| path.to_owned()).collect(),
    }
}

fn on_branch(paths: &[&str]) -> Taking {
    Taking {
        branch: Some("feat/x".into()),
        ..covering(paths)
    }
}

#[test]
fn a_take_keeps_the_files_and_branch_it_covers_through_a_rebuild() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let taking = on_branch(&["crates/tm-store/**", "docs"]);
    let claim = store.take(&agent("s1"), issue, taking).unwrap();
    assert_eq!(claim.paths, ["crates/tm-store/**", "docs"]);
    assert_eq!(claim.branch.as_deref(), Some("feat/x"));
    assert_eq!(held(&store, issue), Some(claim.clone()));
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(held(&store, issue), Some(claim));
}

#[test]
fn only_its_own_session_takes_it_again_and_only_to_cover_other_files() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let first = store.take(&agent("s1"), issue, covering(&["src"])).unwrap();

    let taking = covering(&["lib", "docs/**", "lib"]);
    let again = store.take(&agent("s1"), issue, taking).unwrap();
    assert_ne!(again.id, first.id);
    assert_eq!(held(&store, issue).unwrap().paths, ["docs/**", "lib"]);

    for unchanged in [&[][..], &["lib", "docs/**"]] {
        let same = store
            .take(&agent("s1"), issue, covering(unchanged))
            .unwrap_err();
        assert!(matches!(same, StoreError::AlreadyClaimed { .. }), "{same}");
    }
    for other in [agent("s2"), Actor::agent("codex", "jace")] {
        let err = store.take(&other, issue, covering(&["web"])).unwrap_err();
        assert!(matches!(err, StoreError::AlreadyClaimed { .. }), "{err}");
    }
    assert_eq!(held(&store, issue).unwrap().id, again.id);
}

#[test]
fn a_handoff_keeps_the_files_the_claim_covered() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let taken = store
        .take(&agent("s1"), issue, on_branch(&["src"]))
        .unwrap();
    let (_, handed) = store
        .hand(&agent("s1"), &taken, "codex", taken.ttl_ms, "over to you")
        .unwrap();
    assert_eq!(handed.paths, ["src"]);
    assert_eq!(handed.branch.as_deref(), Some("feat/x"));
    assert_eq!(held(&store, issue), Some(handed));
    assert_eq!(store.rebuild().unwrap(), 0);
    assert_eq!(held(&store, issue).unwrap().paths, ["src"]);
}

#[test]
fn a_pulled_take_covering_paths_not_kept_as_tm_keeps_them_does_not_fit() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let take = |paths: Vec<String>| Change::Claim {
        action: ClaimAction::Take {
            agent: "claude".into(),
            ttl_ms: None,
            prev: None,
            session: None,
            branch: None,
            paths,
        },
    };
    let numbered = |count: usize| {
        let mut paths: Vec<String> = (0..count).map(|n| format!("p{n:02}")).collect();
        paths.sort();
        paths
    };
    let cases = [
        take(vec!["./src".into()]),
        take(vec!["../src".into()]),
        take(vec!["/etc".into()]),
        take(numbered(33)),
        take(vec!["src".into(), "lib".into()]),
        take(vec!["src".into(), "src".into()]),
        take(vec!["a/./b".into()]),
        take(numbered(32)),
    ];
    let mut seq = 0;
    let mut insert = |issue: Uuid, change: &Change| {
        seq += 1;
        let op = remote_op(7, issue, change, Hlc::new(1_000 + seq, 0), 0);
        insert_acked(&store, &op, i64::try_from(seq).unwrap(), 2_000);
        op.op_id
    };
    let mut takes = Vec::new();
    let mut issues = Vec::new();
    for change in &cases {
        let issue = Uuid::now_v7();
        insert(issue, &Change::Create);
        takes.push(insert(issue, change));
        issues.push(issue);
    }
    assert_eq!(store.rebuild().unwrap(), 7);
    let set_aside: Vec<Uuid> = store.unapplied().unwrap().iter().map(|u| u.op_id).collect();
    assert_eq!(set_aside, takes[..7]);
    assert_eq!(held(&store, issues[7]).unwrap().paths.len(), 32);
}

#[test]
fn a_handed_claim_taken_up_naming_no_paths_keeps_the_ones_it_covered() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (by_take, by_start) = (new_issue(&mut store), new_issue(&mut store));
    let receiver = Actor::agent("codex", "jace").in_session(SessionId::parse("c1").unwrap());
    for issue in [by_take, by_start] {
        let taken = store.take(&agent("s1"), issue, covering(&["src"])).unwrap();
        store
            .hand(&agent("s1"), &taken, "codex", taken.ttl_ms, "over to you")
            .unwrap();
    }
    let took = store.take(&receiver, by_take, covering(&[])).unwrap();
    assert_eq!(took.paths, ["src"]);
    let started = store
        .take_with(&receiver, by_start, Some(HOUR_MS), |_| (Vec::new(), ()))
        .unwrap();
    assert!(!started.kept);
    assert_eq!(started.claim.paths, ["src"]);
    for issue in [by_take, by_start] {
        assert_eq!(held(&store, issue).unwrap().paths, ["src"]);
    }
}

#[test]
fn start_keeps_its_sessions_claim_unless_it_is_to_cover_other_files() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let start = |store: &mut Store, paths: &[&str]| {
        store
            .take_with(&agent("s1"), issue, covering(paths), |_| (Vec::new(), ()))
            .unwrap()
    };
    let first = start(&mut store, &["src"]).claim;
    for unchanged in [&["src"][..], &[]] {
        let again = start(&mut store, unchanged);
        assert!(again.kept, "{unchanged:?}");
        assert_eq!(again.claim.id, first.id);
    }
    let other = start(&mut store, &["lib"]);
    assert!(!other.kept);
    assert_ne!(other.claim.id, first.id);
    assert_eq!(held(&store, issue), Some(other.claim));
}

#[test]
fn next_takes_its_issue_covering_the_paths_and_branch() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let taking = on_branch(&["web/**"]);
    let next = store
        .take_next(&agent("s1"), &[issue], taking, |_| (Vec::new(), ()))
        .unwrap()
        .unwrap();
    let claim = next.claim.unwrap();
    assert_eq!(claim.paths, ["web/**"]);
    assert_eq!(claim.branch.as_deref(), Some("feat/x"));
    assert_eq!(held(&store, issue), Some(claim));
}

#[test]
fn a_pulled_take_of_a_live_claim_stands_only_from_its_session_covering_other_files() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = Uuid::now_v7();
    let take = |agent: &str, session: &str, prev: Option<Uuid>, paths: &[&str]| Change::Claim {
        action: ClaimAction::Take {
            agent: agent.into(),
            ttl_ms: None,
            prev,
            session: Some(session.into()),
            branch: None,
            paths: paths.iter().map(|&path| path.to_owned()).collect(),
        },
    };
    let mut seq = 0;
    let mut insert = |change: &Change| {
        seq += 1;
        let op = remote_op(7, issue, change, Hlc::new(1_000 + seq, 0), 0);
        insert_acked(&store, &op, i64::try_from(seq).unwrap(), 2_000);
        op.op_id
    };
    insert(&Change::Create);
    let first = insert(&take("claude", "s1", None, &["src"]));
    let refused = [
        insert(&take("other", "s1", Some(first), &["lib"])),
        insert(&take("claude", "s2", Some(first), &["lib"])),
        insert(&take("claude", "s1", Some(first), &[])),
        insert(&take("claude", "s1", Some(first), &["src"])),
    ];
    let again = insert(&take("claude", "s1", Some(first), &["lib"]));
    store.rebuild().unwrap();
    let lost: Vec<Uuid> = store.unapplied().unwrap().iter().map(|u| u.op_id).collect();
    assert_eq!(lost, refused);
    let claim = held(&store, issue).unwrap();
    assert_eq!((claim.id, claim.paths), (again, vec!["lib".to_owned()]));
}

#[test]
fn undoing_a_change_of_paths_is_refused_but_a_lapsed_claim_taken_back_is_released() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let live = new_issue(&mut store);
    store.take(&agent("s1"), live, covering(&["src"])).unwrap();
    let changed = store.take(&agent("s1"), live, covering(&["lib"])).unwrap();
    let err = store.undo(&agent("s1"), changed.id).unwrap_err();
    assert!(matches!(err, StoreError::CannotUndo { .. }), "{err}");
    assert_eq!(held(&store, live).unwrap().paths, ["lib"]);
    assert_eq!(store.rebuild().unwrap(), 0);
    let err = store.undo(&agent("s1"), changed.id).unwrap_err();
    assert!(
        matches!(err, StoreError::CannotUndo { .. }),
        "after a rebuild: {err}"
    );

    for again in [&["src"][..], &["lib"], &[]] {
        let issue = new_issue(&mut store);
        store.take(&agent("s1"), issue, covering(&["src"])).unwrap();
        clock.advance(i64::try_from(HOUR_MS).unwrap() + 1);
        let took = store.take(&agent("s1"), issue, covering(again)).unwrap();
        let _ = store.undo(&agent("s1"), took.id).unwrap();
        assert_eq!(held(&store, issue), None, "taken back covering {again:?}");
    }
}

#[test]
fn only_the_agent_a_claim_was_handed_to_takes_up_its_paths() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let (lapsed, handed) = (new_issue(&mut store), new_issue(&mut store));
    store
        .take(&agent("s1"), lapsed, covering(&["src"]))
        .unwrap();
    let taken = store
        .take(&agent("s1"), handed, covering(&["src"]))
        .unwrap();
    store
        .hand(&agent("s1"), &taken, "codex", taken.ttl_ms, "over to you")
        .unwrap();
    clock.advance(i64::try_from(HOUR_MS).unwrap() + 1);
    for issue in [lapsed, handed] {
        let took = store.take(&agent("s2"), issue, covering(&[])).unwrap();
        assert!(took.paths.is_empty(), "{issue}: {:?}", took.paths);
    }
}

#[test]
fn undoing_a_live_handed_claim_taken_up_releases_it() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let taken = store.take(&agent("s1"), issue, covering(&["src"])).unwrap();
    store
        .hand(&agent("s1"), &taken, "codex", taken.ttl_ms, "over to you")
        .unwrap();
    let receiver = Actor::agent("codex", "jace").in_session(SessionId::parse("c1").unwrap());
    let up = store.take(&receiver, issue, covering(&["lib"])).unwrap();
    assert_eq!(store.rebuild().unwrap(), 0);
    let _ = store.undo(&receiver, up.id).unwrap();
    assert_eq!(held(&store, issue), None);
}
