//! Asking the human a question an Issue waits on, and answering it.

use serde_json::json;
use tm_core::actor::SessionId;
use tm_core::{Actor, Change};
use tm_store::CommentRole;
use uuid::Uuid;

use crate::support::{ManualClock, TempDir, draft, field, held, jace, new_issue, open, set_field};

const HOUR_MS: u64 = 3_600_000;

fn claude(session: &str) -> Actor {
    Actor::agent("claude", "jace").in_session(SessionId::parse(session).unwrap())
}

fn ready(store: &tm_store::Store) -> Vec<Uuid> {
    store
        .ready()
        .unwrap()
        .into_iter()
        .map(|ready| ready.issue)
        .collect()
}

#[test]
fn a_question_waits_on_the_human_and_keeps_the_work_for_the_asking_agent() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let taken = store.take(&claude("s1"), issue, Some(HOUR_MS)).unwrap();

    store
        .ask(
            &claude("s1"),
            issue,
            "SQLite or Postgres?",
            &["SQLite".to_owned(), "Postgres".to_owned()],
            "jace",
            Some(&taken),
        )
        .unwrap();
    assert_eq!(field(&store, issue, "waiting_on"), Some(json!("jace")));
    let comments = store.comments(issue).unwrap();
    assert_eq!(comments[0].body, "SQLite or Postgres?");
    assert_eq!(comments[0].role, Some(CommentRole::Question));
    assert_eq!(comments[0].options, ["SQLite", "Postgres"]);
    let open = store.open_question(issue).unwrap().unwrap();
    assert_eq!(open.options, comments[0].options);
    let claim = held(&store, issue).unwrap();
    assert!(claim.handed, "held for the Agent in no session");
    assert_eq!(
        (claim.agent.as_str(), claim.session.as_deref()),
        ("claude", None)
    );
    assert_eq!(
        claim.expires_ms, None,
        "a question can wait past any expiry"
    );

    let question = store.open_question(issue).unwrap().unwrap().id;
    store.answer(&jace(), issue, question, "SQLite").unwrap();
    assert_eq!(field(&store, issue, "waiting_on"), None);
    let comments = store.comments(issue).unwrap();
    assert_eq!(comments[1].body, "SQLite");
    assert_eq!(comments[1].role, Some(CommentRole::Answer));
    assert_eq!(
        held(&store, issue),
        Some(claim.clone()),
        "answering leaves the Claim"
    );
    let resumed = store.take(&claude("s2"), issue, Some(HOUR_MS)).unwrap();
    assert_eq!(resumed.session.as_deref(), Some("s2"));
}

#[test]
fn a_question_on_unclaimed_work_keeps_it_out_of_the_ready_queue() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    assert_eq!(ready(&store), [issue]);
    store
        .ask(&jace(), issue, "Worth doing?", &[], "kim", None)
        .unwrap();
    assert_eq!(held(&store, issue), None);
    assert_eq!(ready(&store), Vec::<Uuid>::new());
    let question = store.open_question(issue).unwrap().unwrap().id;
    store
        .answer(&Actor::human("kim"), issue, question, "yes")
        .unwrap();
    assert_eq!(ready(&store), [issue]);
}

#[test]
fn a_question_is_guarded_inside_the_write() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    store
        .ask(&jace(), issue, "First?", &[], "kim", None)
        .unwrap();
    let first = store.open_question(issue).unwrap().unwrap().id;
    let again = store.ask(&jace(), issue, "Second?", &[], "kim", None);
    assert!(again.is_err(), "already waiting: {again:?}");
    let kim = Actor::human("kim");
    store.answer(&kim, issue, first, "yes").unwrap();
    store
        .ask(&jace(), issue, "Second?", &[], "kim", None)
        .unwrap();
    let stale = store.answer(&kim, issue, first, "late reply");
    assert!(
        stale.is_err(),
        "answers a question no longer open: {stale:?}"
    );
    assert_eq!(field(&store, issue, "waiting_on"), Some(json!("kim")));
    let second = store.open_question(issue).unwrap().unwrap().id;
    store.answer(&kim, issue, second, "no").unwrap();
    let twice = store.answer(&kim, issue, second, "no again");
    assert!(
        twice.is_err(),
        "answers a question already answered: {twice:?}"
    );

    let taken = new_issue(&mut store);
    store.take(&claude("s1"), taken, Some(HOUR_MS)).unwrap();
    let unheld = store.ask(&jace(), taken, "Which?", &[], "kim", None);
    assert!(unheld.is_err(), "another holds a live Claim: {unheld:?}");

    let done = new_issue(&mut store);
    store
        .append(draft(done, set_field("status", json!("done"))))
        .unwrap();
    let finished = store.ask(&jace(), done, "Which?", &[], "kim", None);
    assert!(
        finished.is_err(),
        "a finished Issue waits on no one: {finished:?}"
    );
}

#[test]
fn a_lapsed_claim_of_the_askers_is_kept_for_its_next_session() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    store.take(&claude("s1"), issue, Some(60_000)).unwrap();
    clock.advance(120_000);
    let lapsed = store.claim_state(issue).unwrap().unwrap().claim().clone();
    store
        .ask(&claude("s1"), issue, "Which?", &[], "jace", Some(&lapsed))
        .unwrap();
    let question = store.open_question(issue).unwrap().unwrap().id;
    store.answer(&jace(), issue, question, "this").unwrap();
    let resumed = store.take(&claude("s2"), issue, Some(HOUR_MS)).unwrap();
    assert_eq!(resumed.session.as_deref(), Some("s2"));
}

#[test]
fn a_kept_claim_reads_as_its_questions_answer_and_no_other() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let taken = store.take(&claude("s1"), issue, Some(HOUR_MS)).unwrap();
    store
        .ask(&claude("s1"), issue, "Which?", &[], "jace", Some(&taken))
        .unwrap();
    let kept = held(&store, issue).unwrap();
    assert_eq!(store.answer_for(kept.id).unwrap(), None, "not yet answered");
    let question = store.open_question(issue).unwrap().unwrap().id;
    store.answer(&jace(), issue, question, "SQLite").unwrap();
    let answer = store.answer_for(kept.id).unwrap().unwrap().comment;
    assert_eq!(
        (answer.actor.as_str(), answer.body.as_str(), answer.role),
        ("jace", "SQLite", Some(CommentRole::Answer))
    );
    let resumed = store.take(&claude("s2"), issue, Some(HOUR_MS)).unwrap();
    store
        .ask(
            &claude("s2"),
            issue,
            "And now?",
            &[],
            "jace",
            Some(&resumed),
        )
        .unwrap();
    let kept = held(&store, issue).unwrap();
    assert_eq!(
        store.answer_for(kept.id).unwrap(),
        None,
        "an earlier answer is not this question's"
    );
    let question = store.open_question(issue).unwrap().unwrap().id;
    store.answer(&jace(), issue, question, "Postgres").unwrap();
    let resumed = store.take(&claude("s2"), issue, Some(HOUR_MS)).unwrap();
    let (_, handed) = store
        .hand(&claude("s2"), &resumed, "codex", None, "tests are red")
        .unwrap();
    assert_eq!(
        store.answer_for(handed.id).unwrap(),
        None,
        "a later handoff keeps its note"
    );
}

#[test]
fn only_the_asker_withdraws_its_question() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let taken = store.take(&claude("s1"), issue, Some(HOUR_MS)).unwrap();
    store
        .ask(&claude("s1"), issue, "Which?", &[], "jace", Some(&taken))
        .unwrap();
    let question = store.open_question(issue).unwrap().unwrap().id;
    let codex = Actor::agent("codex", "jace");
    let other = store.withdraw(&codex, issue, question, "settled");
    assert!(other.is_err(), "another Agent: {other:?}");
    let human = store.withdraw(&jace(), issue, question, "settled");
    assert!(human.is_err(), "the person it waits on answers: {human:?}");

    store
        .withdraw(&claude("s2"), issue, question, "the schema settles it")
        .unwrap();
    assert_eq!(field(&store, issue, "waiting_on"), None);
    let kept = held(&store, issue).unwrap();
    let settled = store.answer_for(kept.id).unwrap().unwrap();
    assert_eq!(
        (
            settled.comment.body.as_str(),
            settled.comment.role,
            settled.choice
        ),
        ("the schema settles it", Some(CommentRole::Withdrawn), None)
    );
    let again = store.withdraw(&claude("s2"), issue, question, "twice");
    assert!(again.is_err(), "nothing waits: {again:?}");
}

#[test]
fn a_question_and_how_it_is_settled_are_checked_however_they_are_written() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    for options in [&["one"][..], &["a", "A"], &["a", " "], &["1", "2"]] {
        let options: Vec<String> = options.iter().map(|&option| option.to_owned()).collect();
        let asked = store.ask(&jace(), issue, "Which?", &options, "kim", None);
        assert!(asked.is_err(), "{options:?}: {asked:?}");
    }
    let answer = Change::Answer {
        body: "this".into(),
    };
    let stray = store.append(draft(issue, answer));
    assert!(stray.is_err(), "nothing to answer: {stray:?}");

    store
        .ask(&claude("s1"), issue, "Which?", &[], "jace", None)
        .unwrap();
    let withdraw = Change::Withdraw {
        body: "settled".into(),
    };
    let not_asker = store.append(draft(issue, withdraw));
    assert!(not_asker.is_err(), "jace did not ask: {not_asker:?}");

    let question = store.open_question(issue).unwrap().unwrap().id;
    store.answer(&jace(), issue, question, "this").unwrap();
    let again = Change::Answer {
        body: "that".into(),
    };
    let settled = store.append(draft(issue, again));
    assert!(settled.is_err(), "already answered: {settled:?}");
}

#[test]
fn undoing_a_question_clears_the_wait_and_releases_the_handed_claim() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let taken = store.take(&claude("s1"), issue, Some(HOUR_MS)).unwrap();
    let ops = store
        .ask(&claude("s1"), issue, "Which?", &[], "jace", Some(&taken))
        .unwrap();
    store.undo(&claude("s1"), ops[0].op_id).unwrap();
    assert_eq!(field(&store, issue, "waiting_on"), None);
    let comments = store.comments(issue).unwrap();
    assert!(comments[0].retracted, "{comments:?}");
    assert_eq!(
        held(&store, issue),
        None,
        "as undoing a handoff releases it"
    );
    store.take(&claude("s1"), issue, Some(HOUR_MS)).unwrap();
}
