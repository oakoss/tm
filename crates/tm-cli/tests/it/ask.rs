//! `tm ask` and `tm answer`: a question an Issue waits on, shown to the human
//! and to their Agents, and the work resumed once answered.

use std::path::Path;
use std::process::{Output, Stdio};

use rexpect::process::WaitStatus;
use serde_json::Value;
use tm_core::Actor;

use crate::support::{Scratch, initialized, jace, text, tm, tm_pty, wait_until};

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

/// A repo with `count` Issues, TM-1 on, each asked by claude's session s1.
fn asked(scratch: &Scratch, count: usize, options: &[&str]) -> std::path::PathBuf {
    let (repo, mut store, project) = initialized(scratch);
    for n in 1..=count {
        store
            .create_issue(&Actor::human("jace"), project, "task", &format!("t{n}"))
            .unwrap();
    }
    drop(store);
    for n in 1..=count {
        let key = format!("TM-{n}");
        data(&claude(scratch, &repo, "s1", &["--json", "start", &key]));
        let mut args = vec!["--json", "ask", key.as_str(), "SQLite or Postgres?"];
        for option in options {
            args.extend(["--option", option]);
        }
        data(&claude(scratch, &repo, "s1", &args));
    }
    repo
}

#[test]
fn a_question_offers_choices_picked_by_number_or_words_or_neither() {
    let scratch = Scratch::new();
    let repo = asked(&scratch, 3, &["SQLite", "Postgres"]);
    let shown = text(&tm(&scratch, &repo, &["show", "TM-1"]).stdout);
    assert!(
        shown.contains("  SQLite or Postgres?\n  options: 1. SQLite  2. Postgres\n"),
        "{shown}"
    );
    let shown = data(&tm(&scratch, &repo, &["--json", "show", "TM-1"]));
    assert_eq!(
        shown["comments"][0]["options"],
        serde_json::json!(["SQLite", "Postgres"])
    );
    let prime = text(&tm(&scratch, &repo, &["prime"]).stdout);
    assert!(
        prime.contains("      options: 1. SQLite  2. Postgres\n"),
        "{prime}"
    );
    let prime = data(&tm(&scratch, &repo, &["--json", "prime"]));
    let options = &prime["waiting"]["issues"][0]["question"]["options"];
    assert_eq!(*options, serde_json::json!(["SQLite", "Postgres"]));

    for number in ["3", "0", "-1", "18446744073709551616"] {
        let out_of_range = tm(&scratch, &repo, &["--json", "answer", "TM-1", "--", number]);
        assert_eq!(error(&out_of_range), "invalid_value", "{number}");
    }
    let by_number = data(&tm(&scratch, &repo, &["--json", "answer", "TM-1", "2"]));
    assert_eq!(
        (&by_number["reply"], &by_number["choice"]),
        (&"Postgres".into(), &2.into())
    );
    let by_words = data(&tm(
        &scratch,
        &repo,
        &["--json", "answer", "TM-2", " sqlite "],
    ));
    assert_eq!(
        (&by_words["reply"], &by_words["choice"]),
        (&"SQLite".into(), &1.into())
    );
    let neither = data(&tm(
        &scratch,
        &repo,
        &["--json", "answer", "TM-3", "DuckDB"],
    ));
    assert_eq!(
        (&neither["reply"], &neither["choice"]),
        (&"DuckDB".into(), &Value::Null)
    );
    let prime = text(&claude(&scratch, &repo, "s2", &["prime"]).stdout);
    assert!(prime.contains("jace answered: Postgres"), "{prime}");

    let prime = data(&claude(&scratch, &repo, "s2", &["--json", "prime"]));
    let answers: Vec<(Value, Value)> = prime["handed_claims"]
        .as_array()
        .unwrap()
        .iter()
        .map(|handed| (handed["key"].clone(), handed["answer"]["choice"].clone()))
        .collect();
    assert!(answers.contains(&("TM-1".into(), 2.into())), "{answers:?}");
    assert!(
        answers.contains(&("TM-3".into(), Value::Null)),
        "{answers:?}"
    );
    let shown = data(&tm(&scratch, &repo, &["--json", "show", "TM-2"]));
    assert_eq!(shown["comments"][1]["choice"], 1, "{shown}");
}

#[test]
fn a_number_stands_as_written_where_no_option_is_offered() {
    let scratch = Scratch::new();
    let repo = asked(&scratch, 1, &[]);
    let answered = data(&tm(&scratch, &repo, &["--json", "answer", "TM-1", "3"]));
    assert_eq!(
        (&answered["reply"], &answered["choice"]),
        (&"3".into(), &Value::Null)
    );
}

#[test]
fn an_option_in_any_script_is_picked_whatever_its_case() {
    let scratch = Scratch::new();
    let repo = asked(&scratch, 1, &["Äpfel", "Birnen"]);
    let answered = data(&tm(&scratch, &repo, &["--json", "answer", "TM-1", "äPFEL"]));
    assert_eq!(
        (&answered["reply"], &answered["choice"]),
        (&"Äpfel".into(), &1.into())
    );
}

#[test]
fn options_are_two_or_more_and_distinct() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    let ask = |options: &[&str]| {
        let mut args = vec!["--json", "ask", "TM-1", "Which?", "--of", "kim"];
        for option in options {
            args.extend(["--option", option]);
        }
        tm(&scratch, &repo, &args)
    };
    assert_eq!(error(&ask(&["SQLite"])), "invalid_value");
    assert_eq!(error(&ask(&["SQLite", "sqlite"])), "invalid_value");
    assert_eq!(
        error(&ask(&["Äpfel", "äPFEL"])),
        "invalid_value",
        "any script"
    );
    assert_eq!(error(&ask(&["SQLite", " "])), "invalid_value");
    let numbered = ask(&["2", "1"]);
    assert_eq!(error(&numbered), "invalid_value");
    assert!(
        text(&numbered.stdout).contains("reads as a number"),
        "{numbered:?}"
    );
    let shown = data(&tm(&scratch, &repo, &["--json", "show", "TM-1"]));
    assert_eq!(shown["comments"], serde_json::json!([]), "nothing asked");
}

#[test]
fn only_the_asking_agent_withdraws_its_question() {
    let scratch = Scratch::new();
    let repo = asked(&scratch, 1, &[]);
    let codex = jace(&scratch, &repo, &["--json", "withdraw", "TM-1", "settled"])
        .env("TM_AGENT", "codex")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(error(&codex), "not_asker");
    let human = tm(&scratch, &repo, &["--json", "withdraw", "TM-1", "settled"]);
    assert_eq!(error(&human), "not_asker");

    let out = claude(
        &scratch,
        &repo,
        "s2",
        &["withdraw", "TM-1", "the schema settles it"],
    );
    let withdrew = text(&out.stdout);
    assert!(
        withdrew.starts_with(
            "TM-1 no longer waits on jace: the schema settles it\n\
             claude resumes it with tm start TM-1\n"
        ),
        "{withdrew}"
    );
    let prime = text(&tm(&scratch, &repo, &["prime"]).stdout);
    assert!(!prime.contains("Waiting on you"), "{prime}");
    let prime = text(&claude(&scratch, &repo, "s3", &["prime"]).stdout);
    assert!(
        prime.contains("claude withdrew the question: the schema settles it"),
        "{prime}"
    );
    let shown = text(&tm(&scratch, &repo, &["show", "TM-1"]).stdout);
    assert!(
        shown.contains("claude for jace withdrew the question, "),
        "{shown}"
    );
    let history = text(&tm(&scratch, &repo, &["history", "TM-1"]).stdout);
    assert!(
        history.contains("  withdrew the question: the schema settles it\n"),
        "{history}"
    );
    data(&claude(&scratch, &repo, "s3", &["--json", "start", "TM-1"]));
}

#[test]
fn a_reply_is_asked_for_only_at_a_persons_terminal() {
    let scratch = Scratch::new();
    let repo = asked(&scratch, 1, &[]);
    for args in [&["answer", "TM-1"][..], &["answer"]] {
        let piped = tm(&scratch, &repo, args);
        assert_eq!(piped.status.code(), Some(2), "{piped:?}");
        assert!(
            text(&piped.stderr).contains("give the reply: tm answer "),
            "{piped:?}"
        );
    }
    let mut agent = jace(&scratch, &repo, &["answer", "TM-1"]);
    agent.env("TM_AGENT", "claude");
    let mut at_terminal = rexpect::session::spawn_command(agent, Some(10_000)).unwrap();
    at_terminal
        .exp_string("give the reply: tm answer TM-1")
        .unwrap();
    let status = at_terminal.process().wait().unwrap();
    assert!(
        matches!(status, WaitStatus::Exited(_, 2)),
        "an Agent at a terminal is refused, never left waiting: {status:?}"
    );
    let mut json = tm_pty(&scratch, &repo, &["--json", "answer"]);
    let refused = json.exp_eof().unwrap();
    assert!(refused.contains("\"error\":\"usage\""), "{refused}");
}

#[test]
fn a_walk_whose_every_reply_fails_says_so() {
    let scratch = Scratch::new();
    let repo = asked(&scratch, 1, &[]);
    let mut pty = tm_pty(&scratch, &repo, &["answer"]);
    pty.exp_string("reply (a number or words, blank skips): ")
        .unwrap();
    data(&tm(
        &scratch,
        &repo,
        &["--json", "answer", "TM-1", "elsewhere"],
    ));
    pty.send_line("here").unwrap();
    pty.exp_string("TM-1 was not answered: ").unwrap();
    let rest = pty.exp_eof().unwrap();
    assert!(rest.contains("no reply was written: 1 failed"), "{rest}");
    let status = pty.process().wait().unwrap();
    assert!(matches!(status, WaitStatus::Exited(_, 1)), "{status:?}");
}

#[test]
fn a_person_answers_at_the_terminal_when_no_reply_is_given() {
    let scratch = Scratch::new();
    let repo = asked(&scratch, 1, &["SQLite", "Postgres"]);
    let mut pty = tm_pty(&scratch, &repo, &["answer", "TM-1"]);
    pty.exp_string("options: 1. SQLite  2. Postgres").unwrap();
    pty.exp_string("reply (a number or words): ").unwrap();
    pty.send_line("5").unwrap();
    pty.exp_string("choose 1 to 2").unwrap();
    pty.exp_string("reply (a number or words): ").unwrap();
    pty.send_line("1").unwrap();
    let rest = pty.exp_eof().unwrap();
    assert!(rest.contains("TM-1 answered: SQLite\r\n"), "{rest}");
    let shown = data(&tm(&scratch, &repo, &["--json", "show", "TM-1"]));
    assert_eq!(shown["comments"][1]["body"], "SQLite");
}

#[test]
fn a_bare_answer_walks_each_question_waiting_on_you() {
    let scratch = Scratch::new();
    let repo = asked(&scratch, 3, &[]);
    let mut pty = tm_pty(&scratch, &repo, &["answer"]);
    pty.exp_string("(1 of 3) TM-1  t1").unwrap();
    pty.exp_string("reply (a number or words, blank skips): ")
        .unwrap();
    pty.send_line("").unwrap();
    pty.exp_string("(2 of 3) TM-2  t2").unwrap();
    pty.exp_string("reply (a number or words, blank skips): ")
        .unwrap();
    pty.send_line("Postgres").unwrap();
    pty.exp_string("(3 of 3) TM-3  t3").unwrap();
    pty.exp_string("reply (a number or words, blank skips): ")
        .unwrap();
    pty.send_control('d').unwrap();
    let rest = pty.exp_eof().unwrap();
    assert!(rest.contains("TM-2 answered: Postgres\r\n"), "{rest}");
    assert!(rest.contains("answered 1 of 3\r\n"), "{rest}");
    let prime = data(&tm(&scratch, &repo, &["--json", "prime"]));
    let keys: Vec<&str> = prime["waiting"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        ["TM-1", "TM-3"],
        "a skip and a stop leave theirs waiting"
    );
}
