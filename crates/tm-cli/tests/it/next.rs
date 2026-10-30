//! `tm next`: taking the next Issue from the session's Short-lists, or the
//! ready queue (ADR 0016).

use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};
use tm_core::Actor;

use crate::support::{Scratch, initialized, text, tm, tm_command};

/// `tm` as the Agent `claude` writing for `jace`, with `env` set.
fn claude(scratch: &Scratch, repo: &Path, env: &[(&str, &str)], words: &str) -> Output {
    let mut command = tm_command();
    command
        .args(words.split(' '))
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "claude")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn json_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

/// A repo with four todo Issues, TM-1 to TM-4, and in another Project an
/// Issue more urgent than any, which the ready fallback never takes.
fn repo(scratch: &Scratch) -> PathBuf {
    let (repo, mut store, project) = initialized(scratch);
    let jace = Actor::human("jace");
    let (web, _) = store.create_project(&jace, "WEB", "Web").unwrap();
    let issues = [
        (project, "a", "low"),
        (project, "b", "high"),
        (project, "c", "low"),
        (project, "d", "low"),
        (web, "elsewhere", "urgent"),
    ];
    for (project, title, priority) in issues {
        let (issue, _) = store.create_issue(&jace, project, "task", title).unwrap();
        let change = tm_core::Change::SetField {
            field: "priority".into(),
            label: String::new(),
            value: json!(priority),
            value_label: None,
        };
        let actor = jace.clone();
        store
            .append(tm_store::Draft {
                actor,
                project,
                subject: issue,
                change,
            })
            .unwrap();
    }
    repo
}

fn shortlist(scratch: &Scratch, repo: &Path, words: &str) {
    let args: Vec<&str> = words.split(' ').collect();
    let out = tm(scratch, repo, &args);
    assert!(out.status.success(), "{words}: {out:?}");
}

#[test]
fn an_agent_takes_the_lists_first_eligible_issue_and_starts_it() {
    let scratch = Scratch::new();
    let repo = repo(&scratch);
    shortlist(&scratch, &repo, "shortlist create docs --actor claude");
    shortlist(&scratch, &repo, "shortlist add docs TM-3 TM-1 TM-4");
    let other = tm_command()
        .args(["start", "TM-3"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(other.status.success(), "{other:?}");

    let peek = claude(&scratch, &repo, &[], "next --peek");
    let said = text(&peek.stdout);
    let line = "TM-1 is next on docs; tm next takes it\n";
    assert!(said.starts_with(line), "{said}");

    let out = claude(&scratch, &repo, &[], "--json next");
    let doc = json_of(&out);
    assert!(out.status.success(), "{doc}");
    assert_eq!(doc["data"]["key"], "TM-1");
    assert_eq!(doc["data"]["taken"], true);
    assert_eq!(doc["data"]["from"], "shortlist");
    assert_eq!(doc["data"]["claim"]["agent"], "claude");
    assert_eq!(doc["data"]["fields"]["status"], "in_progress");
    assert!(doc["data"]["batch"].is_string(), "{doc}");

    let out = claude(&scratch, &repo, &[], "next");
    assert!(text(&out.stdout).starts_with("TM-4 is next on docs\n"));
}

#[test]
fn with_nothing_eligible_it_says_why_and_takes_nothing() {
    let scratch = Scratch::new();
    let repo = repo(&scratch);
    shortlist(&scratch, &repo, "shortlist create docs --actor claude");
    shortlist(&scratch, &repo, "shortlist add docs TM-1 TM-2");
    shortlist(&scratch, &repo, "link TM-1 blocks TM-2");
    let codex = tm_command()
        .args(["claim", "TM-1"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "codex")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(codex.status.success(), "{codex:?}");

    let out = claude(&scratch, &repo, &[], "next");
    assert!(out.status.success(), "{out:?}");
    let said = text(&out.stdout);
    let why = "nothing on docs can be taken next; of its 2 Issues: 1 blocked, 1 claimed\n";
    assert_eq!(said, format!("{why}tm ready lists other work\n"));
    let doc = json_of(&claude(&scratch, &repo, &[], "--json next"));
    assert_eq!(doc["data"]["taken"], false);
    assert_eq!(doc["data"]["key"], Value::Null);
    let peeked = json_of(&claude(&scratch, &repo, &[], "--json next --peek"));
    assert_eq!(peeked["data"]["key"], Value::Null);
    let second = &doc["data"]["passed_over"][1]["passed_over"];
    assert_eq!(second, &json!(["blocked"]));
    assert_eq!(doc["data"].get("batch"), None);
}

#[test]
fn a_question_waiting_on_an_answer_is_passed_over() {
    let scratch = Scratch::new();
    let repo = repo(&scratch);
    shortlist(&scratch, &repo, "shortlist create docs --actor claude");
    shortlist(&scratch, &repo, "shortlist add docs TM-1 TM-2");
    shortlist(&scratch, &repo, "ask TM-1 --of jace Which?");
    let doc = json_of(&claude(&scratch, &repo, &[], "--json next --peek"));
    assert_eq!(doc["data"]["key"], "TM-2", "{doc}");
    shortlist(&scratch, &repo, "ask TM-2 --of jace And?");
    let doc = json_of(&claude(&scratch, &repo, &[], "--json next"));
    assert_eq!(doc["data"]["taken"], false, "{doc}");
    let first = &doc["data"]["passed_over"][0]["passed_over"];
    assert_eq!(first, &json!(["waiting"]));
}

#[test]
fn a_session_holding_its_lists_last_issue_is_told_so() {
    let scratch = Scratch::new();
    let repo = repo(&scratch);
    shortlist(&scratch, &repo, "shortlist create docs --actor claude");
    shortlist(&scratch, &repo, "shortlist add docs TM-1");
    let taken = claude(&scratch, &repo, &[], "next");
    assert!(taken.status.success(), "{taken:?}");
    let said = text(&claude(&scratch, &repo, &[], "next").stdout);
    let hint = "you already hold TM-1; tm show TM-1\n";
    assert!(said.contains(hint), "{said}");
}

#[test]
fn a_session_with_no_lists_takes_the_most_urgent_ready_issue() {
    let scratch = Scratch::new();
    let repo = repo(&scratch);
    let doc = json_of(&claude(&scratch, &repo, &[], "--json next"));
    assert_eq!(doc["data"]["key"], "TM-2", "{doc}");
    assert_eq!(doc["data"]["from"], "ready");
    let said = text(&claude(&scratch, &repo, &[], "next --peek").stdout);
    let line = "TM-1 is next in TM's ready queue";
    assert!(said.starts_with(line), "{said}");
}

/// The fallback leaves what another Actor's list holds, and epics, which a
/// take passes over, so a peek names what a take would take.
#[test]
fn the_ready_fallback_skips_epics_and_other_actors_lists() {
    let scratch = Scratch::new();
    let repo = repo(&scratch);
    shortlist(&scratch, &repo, "edit TM-2 --set type=epic");
    shortlist(&scratch, &repo, "shortlist create codex --actor codex");
    shortlist(&scratch, &repo, "shortlist add codex TM-1");
    let said = text(&claude(&scratch, &repo, &[], "next --peek").stdout);
    assert!(said.starts_with("TM-3 is next"), "{said}");
    let doc = json_of(&claude(&scratch, &repo, &[], "--json next"));
    assert_eq!(doc["data"]["key"], "TM-3", "{doc}");
    shortlist(&scratch, &repo, "shortlist add codex TM-4");
    let said = text(&claude(&scratch, &repo, &[], "next").stdout);
    let why = "nothing in TM's ready queue can be taken next; of its 3 Issues: 1 epic, 2 on a Short-list\n";
    let hint = "tm shortlist --all shows the lists holding the rest\n";
    assert_eq!(said, format!("{why}{hint}"));
    let doc = json_of(&claude(&scratch, &repo, &[], "--json next"));
    let listed: Vec<&Value> = doc["data"]["passed_over"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["passed_over"] == json!(["on_shortlist"]))
        .collect();
    assert_eq!(listed.len(), 2, "{doc}");
    assert_eq!(listed[0]["shortlist"], "codex");
}

#[test]
fn tm_shortlist_binds_a_session_to_one_list_and_never_falls_back() {
    let scratch = Scratch::new();
    let repo = repo(&scratch);
    let bound = [("TM_SHORTLIST", "ops")];
    let out = claude(&scratch, &repo, &bound, "--json next");
    assert_eq!(out.status.code(), Some(1));
    let doc = json_of(&out);
    assert_eq!(doc["error"], "not_found");
    assert_eq!(doc["name"], "ops");

    shortlist(&scratch, &repo, "shortlist create docs --actor claude");
    shortlist(&scratch, &repo, "shortlist create ops --actor claude");
    shortlist(&scratch, &repo, "shortlist add docs TM-1");
    shortlist(&scratch, &repo, "shortlist add ops TM-3");
    let doc = json_of(&claude(&scratch, &repo, &bound, "--json next"));
    assert_eq!(doc["data"]["key"], "TM-3", "{doc}");

    shortlist(&scratch, &repo, "shortlist create web --actor codex");
    let out = claude(&scratch, &repo, &[("TM_SHORTLIST", "web")], "--json next");
    let doc = json_of(&out);
    assert_eq!(doc["valid"], json!(["docs", "ops"]), "{doc}");
    let out = claude(&scratch, &repo, &[("TM_SHORTLIST", "nope")], "--json next");
    let doc = json_of(&out);
    assert_eq!(doc["error"], "not_found");
    assert_eq!(doc["valid"], json!(["docs", "ops"]), "{doc}");
    let message = doc["message"].as_str().unwrap();
    assert!(message.starts_with("TM_SHORTLIST names nope"), "{message}");

    let out = claude(&scratch, &repo, &[("TM_SHORTLIST", " ")], "--json next");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json_of(&out)["error"], "invalid_value");
}

#[test]
fn a_human_starts_the_next_issue_without_a_claim() {
    let scratch = Scratch::new();
    let repo = repo(&scratch);
    let out = tm(&scratch, &repo, &["--json", "next"]);
    let doc = json_of(&out);
    assert!(out.status.success(), "{doc}");
    assert_eq!(doc["data"]["key"], "TM-2");
    assert_eq!(doc["data"]["claim"], Value::Null);
    assert_eq!(doc["data"]["fields"]["status"], "in_progress");
}
