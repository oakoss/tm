use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Stdio;

use serde_json::{Value, json};
use tm_core::{Actor, Change, ProjectId};
use tm_store::{Draft, Store};
use uuid::Uuid;

use crate::support::{Scratch, initialized, text, tm, tm_command};

fn set(store: &mut Store, project: ProjectId, issue: Uuid, field: &str, value: &str) {
    store
        .append(Draft {
            actor: Actor::human("jace"),
            project,
            subject: issue,
            change: Change::SetField {
                field: field.into(),
                label: String::new(),
                value: json!(value),
                value_label: None,
            },
        })
        .unwrap();
}

fn new(store: &mut Store, project: ProjectId, title: &str) -> Uuid {
    store
        .create_issue(&Actor::human("jace"), project, "task", title)
        .unwrap()
        .0
}

fn list(scratch: &Scratch, repo: &Path, args: &[&str]) -> (bool, Value) {
    let out = tm(scratch, repo, &[&["list", "--json"], args].concat());
    assert_eq!(text(&out.stderr), "", "nothing reaches stderr in JSON mode");
    (
        out.status.success(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

fn keys(doc: &Value) -> Vec<String> {
    let mut keys: Vec<String> = doc["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["key"].as_str().unwrap().to_owned())
        .collect();
    keys.sort();
    keys
}

#[test]
fn json_list_and_ready_carry_the_stores_warnings() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    new(&mut store, project, "t");
    store
        .connection()
        .execute(
            "INSERT INTO unapplied (op_id, reason) VALUES (?1, ?2)",
            (Uuid::now_v7().as_bytes().to_vec(), "unknown field"),
        )
        .unwrap();
    for command in ["list", "ready"] {
        let out = tm(&scratch, &repo, &["--json", command]);
        assert!(out.status.success(), "{command}: {out:?}");
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            doc["warnings"],
            json!(["1 logged operation did not apply in full; tm doctor will list it"]),
            "{command}"
        );
    }
}

#[test]
fn with_no_filter_list_shows_the_open_work() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    new(&mut store, project, "todo");
    for (title, status) in [
        ("triage", "triage"),
        ("active", "in_progress"),
        ("done", "done"),
        ("cancelled", "cancelled"),
    ] {
        let issue = new(&mut store, project, title);
        set(&mut store, project, issue, "status", status);
    }
    let archived = new(&mut store, project, "archived");
    store
        .append(Draft {
            actor: Actor::human("jace"),
            project,
            subject: archived,
            change: Change::SetField {
                field: "archived".into(),
                label: String::new(),
                value: json!(true),
                value_label: None,
            },
        })
        .unwrap();

    let (ok, doc) = list(&scratch, &repo, &[]);
    assert!(ok, "{doc}");
    assert_eq!(keys(&doc), ["TM-1", "TM-2", "TM-3"]);
    assert_eq!(doc["data"]["total"], 3);
    let first = &doc["data"]["issues"][0];
    assert_eq!(first["project"], "TM");
    assert!(first["fields"]["title"].is_string(), "{first}");

    let (_, doc) = list(&scratch, &repo, &["category:done,cancelled"]);
    assert_eq!(keys(&doc), ["TM-4", "TM-5"]);

    for args in [&["--all"][..], &["--all", "sort:priority"]] {
        let (ok, doc) = list(&scratch, &repo, args);
        assert!(ok, "{doc}");
        assert_eq!(
            keys(&doc),
            ["TM-1", "TM-2", "TM-3", "TM-4", "TM-5"],
            "every Status: {args:?}"
        );
    }
}

#[test]
fn filter_words_join_into_one_filter_and_limit_cuts_the_list() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["a", "b", "c"] {
        let issue = new(&mut store, project, title);
        set(&mut store, project, issue, "priority", "high");
    }
    new(&mut store, project, "low");

    let (_, doc) = list(&scratch, &repo, &["priority:high", "status:todo"]);
    assert_eq!(keys(&doc), ["TM-1", "TM-2", "TM-3"]);
    let (_, doc) = list(&scratch, &repo, &["priority:high", "--limit", "2"]);
    assert_eq!(doc["data"]["issues"].as_array().unwrap().len(), 2);
    assert_eq!(doc["data"]["total"], 3);

    let out = tm(&scratch, &repo, &["list", "priority:high", "--limit", "1"]);
    let shown = text(&out.stdout);
    assert!(shown.contains("  todo  high  "), "{shown}");
    assert!(shown.ends_with("(1 of 3 shown)\n"), "{shown}");
    let out = tm(&scratch, &repo, &["list", "priority:urgent"]);
    assert_eq!(text(&out.stdout), "no Issues match\n");

    let (ok, doc) = list(&scratch, &repo, &["stauts:todo"]);
    assert!(!ok);
    assert_eq!(doc["error"], "unknown_field", "{doc}");
    let (_, doc) = list(&scratch, &repo, &["sort:bogus"]);
    assert_eq!(doc["error"], "unknown_field", "{doc}");
    assert_eq!(doc["span"], json!([5, 10]), "the span is in what was typed");
    let (_, doc) = list(&scratch, &repo, &["parent:TM-9"]);
    assert_eq!(doc["error"], "not_found", "{doc}");
    assert_eq!(doc["message"], "no Issue is TM-9", "{doc}");
    assert_eq!(doc["kind"], "issue", "{doc}");
    assert_eq!(doc["span"], json!([7, 11]), "{doc}");
}

#[test]
fn at_me_is_the_human_an_agent_works_for() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let mine = new(&mut store, project, "mine");
    set(&mut store, project, mine, "assignee", "jace");
    let theirs = new(&mut store, project, "theirs");
    set(&mut store, project, theirs, "assignee", "sam");

    let run = |vars: &[(&str, &str)]| {
        let mut command = tm_command();
        command
            .args(["list", "--json", "assignee:@me"])
            .current_dir(&repo)
            .env("TM_HOME", scratch.home())
            .env_remove("TM_ACTOR")
            .env("GIT_CONFIG_GLOBAL", scratch.git_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null());
        for (key, value) in vars {
            command.env(key, value);
        }
        let out = command.output().unwrap();
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        doc
    };
    let doc = run(&[("TM_ACTOR", "jace"), ("CLAUDE_CODE_SESSION_ID", "s")]);
    assert_eq!(keys(&doc), ["TM-1"], "{doc}");

    let doc = run(&[]);
    assert_eq!(doc["error"], "no_identity", "{doc}");
    assert!(
        doc["message"].as_str().unwrap().contains("TM_ACTOR"),
        "the reason no one was found: {doc}"
    );
}

#[test]
fn list_keeps_the_filters_order_negation_and_one_line_per_issue() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for (title, priority) in [("low", "low"), ("urgent", "urgent"), ("high", "high")] {
        let issue = new(&mut store, project, title);
        set(&mut store, project, issue, "priority", priority);
    }
    let done = new(&mut store, project, "done");
    set(&mut store, project, done, "status", "done");
    set(&mut store, project, done, "priority", "urgent");
    new(&mut store, project, "two\nlines");

    let ordered = |doc: &Value| -> Vec<String> {
        doc["data"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|issue| issue["key"].as_str().unwrap().to_owned())
            .collect()
    };
    let (_, doc) = list(&scratch, &repo, &["sort:priority", "--limit", "2"]);
    assert_eq!(
        ordered(&doc),
        ["TM-2", "TM-3"],
        "open work only, sorted: {doc}"
    );
    assert_eq!(doc["data"]["total"], 4);
    let (ok, doc) = list(
        &scratch,
        &repo,
        &["--limit", "5", "--", "-priority:low,none"],
    );
    assert!(ok, "{doc}");
    assert_eq!(keys(&doc), ["TM-2", "TM-3", "TM-4"]);
    let (ok, doc) = list(&scratch, &repo, &["-priority:low,none", "--limit", "2"]);
    assert!(ok, "a negated term needs no --: {doc}");
    assert_eq!(doc["data"]["issues"].as_array().unwrap().len(), 2, "{doc}");
    assert_eq!(doc["data"]["total"], 3);
    let (ok, doc) = list(&scratch, &repo, &["-priority:low", "--limt", "2"]);
    assert!(!ok);
    assert_eq!(
        doc["error"], "usage",
        "a mistyped flag is still clap's: {doc}"
    );

    let out = tm(&scratch, &repo, &["list", "priority:none"]);
    assert_eq!(text(&out.stdout).lines().count(), 1, "{out:?}");
    assert!(text(&out.stdout).ends_with("two\\nlines\n"), "{out:?}");
}

#[test]
fn a_filter_without_at_me_runs_no_git() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    new(&mut store, project, "t");
    let bin = scratch.0.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let marker = scratch.0.join("git-ran");
    let git = bin.join("git");
    std::fs::write(
        &git,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = tm_command()
        .args(["list", "--json"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env_remove("TM_ACTOR")
        .env("PATH", &bin)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(!marker.exists(), "git ran for a filter without @me");
}

#[test]
fn a_filter_without_at_me_needs_no_identity() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    new(&mut store, project, "t");
    let out = tm_command()
        .args(["list", "--json"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env_remove("TM_ACTOR")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(keys(&doc), ["TM-1"]);
}
