//! `tm ask` and `tm answer`: a question an Issue waits on, shown to the human
//! and to their Agents, and the work resumed once answered.

use std::path::Path;
use std::process::{Output, Stdio};

use serde_json::Value;
use tm_core::Actor;

use crate::support::{Scratch, initialized, jace, text, tm, wait_until};

fn claude(scratch: &Scratch, repo: &Path, session: &str, args: &[&str]) -> Output {
    jace(scratch, repo, args)
        .env("TM_AGENT", "claude")
        .env("TM_SESSION", session)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn data(out: &Output) -> Value {
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone()
}

fn error(out: &Output) -> String {
    assert!(!out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    doc["error"].as_str().unwrap().to_owned()
}

#[test]
fn an_agent_parks_its_work_on_a_question_and_resumes_it_once_answered() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "Pick the cache")
        .unwrap();
    drop(store);
    data(&claude(&scratch, &repo, "s1", &["--json", "start", "TM-1"]));

    let out = claude(
        &scratch,
        &repo,
        "s1",
        &["ask", "TM-1", "SQLite or Postgres?"],
    );
    let asked = text(&out.stdout);
    assert!(
        asked.starts_with(
            "TM-1 waits on jace: SQLite or Postgres?\n\
             the Claim is kept for claude, which resumes it with tm start TM-1 once answered\n\
             tm next takes other work\n\
             undo this with: tm undo "
        ),
        "{asked}"
    );
    let refused = claude(&scratch, &repo, "s2", &["--json", "start", "TM-1"]);
    assert_eq!(error(&refused), "waiting");
    let prime = text(&claude(&scratch, &repo, "s2", &["prime"]).stdout);
    assert!(
        prime.contains(
            "Waiting on jace (1): ask them, then tm answer KEY \"...\" records each reply:\n  TM-1  in_progress  none  Pick the cache\n    claude asked "
        ),
        "{prime}"
    );
    assert!(!prime.contains("Handed to you"), "{prime}");
    let prime = text(&tm(&scratch, &repo, &["prime"]).stdout);
    assert!(prime.contains("Waiting on you (1): "), "{prime}");
    let shown = text(&tm(&scratch, &repo, &["show", "TM-1"]).stdout);
    assert!(shown.contains("waiting_on: jace\n"), "{shown}");
    assert!(shown.contains("claude for jace asked, "), "{shown}");

    let answered = data(&tm(
        &scratch,
        &repo,
        &["--json", "answer", "TM-1", "SQLite"],
    ));
    assert_eq!(answered["kept_for"], "claude");
    let prime = text(&claude(&scratch, &repo, "s2", &["prime"]).stdout);
    assert!(prime.contains("    jace answered: SQLite\n"), "{prime}");
    let resumed = data(&claude(&scratch, &repo, "s2", &["--json", "start", "TM-1"]));
    assert_eq!(resumed["claim"]["session"], "s2", "{resumed}");
    let history = text(&tm(&scratch, &repo, &["history", "TM-1"]).stdout);
    assert!(
        history.contains("  asked: SQLite or Postgres?\n"),
        "{history}"
    );
    assert!(history.contains("  answered: SQLite\n"), "{history}");
}

#[test]
fn a_question_is_refused_where_it_would_not_be_seen_or_resumed() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["held", "done"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    data(&claude(&scratch, &repo, "s1", &["--json", "start", "TM-1"]));
    let held = claude(&scratch, &repo, "s2", &["--json", "ask", "TM-1", "Which?"]);
    assert_eq!(error(&held), "already_claimed", "another session holds it");
    let named = claude(
        &scratch,
        &repo,
        "s1",
        &["--json", "ask", "TM-1", "Which?", "--of", "kim"],
    );
    assert_eq!(error(&named), "usage", "an Agent asks its own human");
    data(&tm(&scratch, &repo, &["--json", "move", "TM-2", "done"]));
    let finished = tm(
        &scratch,
        &repo,
        &["--json", "ask", "TM-2", "Which?", "--of", "kim"],
    );
    assert_eq!(error(&finished), "finished");
    for args in [
        &[
            "--json",
            "create",
            "--type",
            "task",
            "--title",
            "x",
            "--set",
            "waiting_on=kim",
        ][..],
        &["--json", "move", "TM-1", "todo", "--set", "waiting_on=kim"],
    ] {
        let out = tm(&scratch, &repo, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {out:?}");
    }
}

#[test]
fn a_question_is_asked_once_and_answered_only_while_it_waits() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    let unnamed = tm(&scratch, &repo, &["--json", "ask", "TM-1", "Which?"]);
    assert_eq!(error(&unnamed), "usage", "a person names who answers");
    let not_waiting = tm(&scratch, &repo, &["--json", "answer", "TM-1", "this"]);
    assert_eq!(error(&not_waiting), "not_waiting");
    let asked = data(&tm(
        &scratch,
        &repo,
        &["--json", "ask", "TM-1", "Which?", "--of", "kim"],
    ));
    assert_eq!(asked["waiting_on"], "kim");
    assert_eq!(asked["kept_for"], Value::Null, "no Claim to keep");
    let again = tm(
        &scratch,
        &repo,
        &["--json", "ask", "TM-1", "And?", "--of", "kim"],
    );
    assert_eq!(error(&again), "already_waiting");
    let edited = tm(
        &scratch,
        &repo,
        &["--json", "edit", "TM-1", "--set", "waiting_on=x"],
    );
    assert_eq!(edited.status.code(), Some(2), "{edited:?}");
    let ready = text(&tm(&scratch, &repo, &["ready"]).stdout);
    assert_eq!(ready, "nothing is ready\n");
    let moved = text(&tm(&scratch, &repo, &["move", "TM-1", "cancelled"]).stdout);
    assert!(
        moved.contains("TM-1  waiting_on: kim → (none)\n"),
        "{moved}"
    );
}

#[test]
fn a_question_on_a_lapsed_claim_is_resumed_by_the_next_session() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    let start = ["--json", "start", "TM-1", "--for", "1s"];
    let started = data(&claude(&scratch, &repo, "s1", &start));
    let expires = started["claim"]["expires_ms"].as_i64().unwrap();
    wait_until(expires + 1);
    let asked = data(&claude(
        &scratch,
        &repo,
        "s1",
        &["--json", "ask", "TM-1", "Which?"],
    ));
    assert_eq!(asked["kept_for"], "claude", "{asked}");
    data(&tm(&scratch, &repo, &["--json", "answer", "TM-1", "this"]));
    let resumed = data(&claude(&scratch, &repo, "s2", &["--json", "start", "TM-1"]));
    assert_eq!(resumed["claim"]["session"], "s2", "{resumed}");
}
