use std::path::Path;
use std::process::Stdio;

use serde_json::{Value, json};
use tm_core::Actor;
use tm_store::Store;

use crate::support::{Scratch, initialized, text, tm, tm_command};

fn run(scratch: &Scratch, repo: &Path, args: &[&str]) -> (Option<i32>, Value) {
    let out = tm(scratch, repo, &[&["--json"], args].concat());
    assert_eq!(text(&out.stderr), "", "nothing reaches stderr in JSON mode");
    (
        out.status.code(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

fn ops(store: &Store) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM ops", [], |row| row.get(0))
        .unwrap()
}

/// A repo with TM-1, moved to `in_progress`; the move's batch.
fn moved(scratch: &Scratch) -> (std::path::PathBuf, Store, String) {
    let (repo, mut store, project) = initialized(scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    let (code, doc) = run(scratch, &repo, &["move", "TM-1", "in_progress"]);
    assert_eq!(code, Some(0), "{doc}");
    let batch = doc["data"]["batch"].as_str().unwrap().to_owned();
    (repo, store, batch)
}

#[test]
fn undo_reverts_a_move_and_its_own_batch_redoes_it() {
    let scratch = Scratch::new();
    let (repo, store, batch) = moved(&scratch);

    let before = ops(&store);
    let (code, doc) = run(&scratch, &repo, &["undo", &batch, "--dry-run"]);
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(doc["data"]["dry_run"], true);
    assert_eq!(doc["data"]["batch"], Value::Null);
    assert_eq!(
        doc["data"]["restored"],
        json!([{
            "issue": doc["data"]["issues"][0]["id"],
            "key": "TM-1",
            "field": "status",
            "from": "in_progress",
            "to": "todo",
        }])
    );
    assert_eq!(ops(&store), before, "a dry run writes nothing");

    let out = tm(&scratch, &repo, &["undo", &batch]);
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    assert!(
        shown.contains("  TM-1 status: \"in_progress\" -> \"todo\"\n"),
        "{shown}"
    );
    let the_undo = shown
        .lines()
        .last()
        .unwrap()
        .strip_prefix("undo this with: tm undo ")
        .unwrap()
        .to_owned();
    let (_, doc) = run(&scratch, &repo, &["show", "TM-1"]);
    assert_eq!(doc["data"]["fields"]["status"], "todo");

    let (code, doc) = run(&scratch, &repo, &["undo", &the_undo]);
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(doc["data"]["issues"][0]["fields"]["status"], "in_progress");

    run(&scratch, &repo, &["move", "TM-1", "in_review"]);
    let (code, doc) = run(&scratch, &repo, &["undo", &batch]);
    assert_eq!(code, Some(1));
    assert_eq!(doc["error"], "undo_conflict", "{doc}");
    assert_eq!(doc["conflicts"][0]["from"], "in_review");
}

#[test]
fn a_batch_is_named_by_its_end_and_never_guessed() {
    let scratch = Scratch::new();
    let (repo, _store, mut batch) = moved(&scratch);
    let mut later = String::new();
    // An id's first 8 digits change every 65 seconds, so of two writes in a
    // row that straddle the change, the next pair cannot.
    for status in ["in_review", "in_progress"] {
        let (_, doc) = run(&scratch, &repo, &["move", "TM-1", status]);
        if !later.is_empty() {
            batch = later;
        }
        later = doc["data"]["batch"].as_str().unwrap().to_owned();
        if batch[..8] == later[..8] {
            break;
        }
    }
    assert_eq!(
        batch[..8],
        later[..8],
        "ids written together share their start"
    );

    let (code, doc) = run(&scratch, &repo, &["undo", &batch[..8], "--dry-run"]);
    assert_eq!(
        (code, doc["error"].as_str()),
        (Some(1), Some("invalid_value")),
        "{doc}"
    );
    assert!(doc["matches"].as_array().unwrap().len() >= 2, "{doc}");

    let (code, doc) = run(
        &scratch,
        &repo,
        &["undo", &later[later.len() - 12..], "--dry-run"],
    );
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(doc["data"]["undone"], json!(later));

    let (code, doc) = run(&scratch, &repo, &["undo", "abc"]);
    assert_eq!(
        (code, doc["error"].as_str()),
        (Some(1), Some("invalid_value"))
    );
    let (code, doc) = run(&scratch, &repo, &["undo", "0123456789ab"]);
    assert_eq!((code, doc["error"].as_str()), (Some(1), Some("not_found")));
    assert_eq!(doc["kind"], "batch");
}

#[test]
fn a_batch_reference_is_hex_so_wildcards_name_nothing() {
    let scratch = Scratch::new();
    let (repo, _store, _) = moved(&scratch);
    for wildcard in ["%%%%%%%%", "________", "01a151d5%"] {
        let (code, doc) = run(&scratch, &repo, &["undo", wildcard, "--dry-run"]);
        assert_eq!(code, Some(1), "{wildcard}: {doc}");
        assert_eq!(doc["error"], "invalid_value", "{wildcard}: {doc}");
    }
}

#[test]
fn undoing_a_link_shows_both_ends_by_key() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["a", "b"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    let (_, doc) = run(
        &scratch,
        &repo,
        &["link", "TM-2", "is", "blocked", "by", "TM-1"],
    );
    let linked = doc["data"]["batch"].as_str().unwrap().to_owned();
    let (_, doc) = run(&scratch, &repo, &["undo", &linked, "--dry-run"]);
    let keys: Vec<&str> = doc["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["TM-1", "TM-2"], "{doc}");
    let out = tm(&scratch, &repo, &["undo", &linked]);
    assert!(
        text(&out.stdout).contains("  TM-1 blocks TM-2: removed\n"),
        "{out:?}"
    );

    let (_, doc) = run(
        &scratch,
        &repo,
        &[
            "create", "--type", "task", "--title", "c", "--label", "Backend",
        ],
    );
    let created = doc["data"]["batch"].as_str().unwrap().to_owned();
    let out = tm(&scratch, &repo, &["undo", &created]);
    assert!(
        text(&out.stdout).contains("  TM-3 archived: false -> true\n"),
        "{out:?}"
    );
}

#[test]
fn undoing_a_create_archives_and_a_comment_retracts() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let (_, doc) = run(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "t"],
    );
    let created = doc["data"]["batch"].as_str().unwrap().to_owned();
    let (_, doc) = run(&scratch, &repo, &["comment", "TM-1", "oops"]);
    let comment = doc["data"]["batch"].as_str().unwrap().to_owned();

    let (code, doc) = run(&scratch, &repo, &["undo", &comment]);
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(doc["data"]["issues"][0]["comments"][0]["retracted"], true);
    let (code, doc) = run(&scratch, &repo, &["undo", &created]);
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(doc["data"]["issues"][0]["fields"]["archived"], true);
    assert_eq!(doc["data"]["issues"][0]["key"], "TM-1");
}

fn as_agent(scratch: &Scratch, repo: &Path, args: &[&str]) -> (Option<i32>, Value) {
    let out = tm_command()
        .args([&["--json"], args].concat())
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "claude")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.code(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

#[test]
fn a_bare_undo_steps_back_through_the_humans_commands() {
    let scratch = Scratch::new();
    let (repo, _store, _) = moved(&scratch);
    let out = tm(&scratch, &repo, &["move", "TM-1", "in_review"]);
    let hint = text(&out.stdout);
    let hint = hint.lines().last().unwrap();
    assert!(hint.starts_with("undo this with: tm undo "), "{hint}");
    assert_eq!(hint.len(), "undo this with: tm undo ".len() + 12);
    as_agent(&scratch, &repo, &["comment", "TM-1", "by claude"]);

    let (code, doc) = run(&scratch, &repo, &["undo"]);
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(doc["data"]["issues"][0]["fields"]["status"], "in_progress");
    let (code, doc) = run(&scratch, &repo, &["undo"]);
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(doc["data"]["issues"][0]["fields"]["status"], "todo");
    assert_eq!(
        doc["data"]["issues"][0]["comments"][0]["retracted"], false,
        "the Agent's comment is not the human's to undo"
    );
}

#[test]
fn a_write_that_changed_nothing_offers_no_undo() {
    let scratch = Scratch::new();
    let (repo, _store, _) = moved(&scratch);
    let out = tm(&scratch, &repo, &["move", "TM-1", "in_progress"]);
    assert!(out.status.success(), "{out:?}");
    assert!(!text(&out.stdout).contains("undo this with"), "{out:?}");
    let (_, doc) = run(&scratch, &repo, &["move", "TM-1", "in_progress"]);
    assert!(doc["data"]["batch"].is_null(), "nothing was written: {doc}");
}

#[test]
fn a_refused_bare_undo_offers_the_command_before_it() {
    let scratch = Scratch::new();
    let (repo, _store, _) = moved(&scratch);
    as_agent(&scratch, &repo, &["move", "TM-1", "in_review"]);

    let (code, doc) = run(&scratch, &repo, &["undo"]);
    assert_eq!(
        (code, doc["error"].as_str()),
        (Some(1), Some("undo_conflict")),
        "{doc}"
    );
    let fix = doc["fix"].as_str().unwrap();
    let (code, doc) = run(
        &scratch,
        &repo,
        &[
            fix.strip_prefix("tm ")
                .unwrap()
                .split(' ')
                .collect::<Vec<_>>()
                .as_slice(),
            &["--dry-run"],
        ]
        .concat(),
    );
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(
        doc["data"]["restored"][0]["field"], "archived",
        "the create, before the move"
    );
}

#[test]
fn a_bare_undo_refused_for_another_reason_offers_nothing_older() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["a", "b"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    run(&scratch, &repo, &["link", "TM-1", "blocks", "TM-2"]);
    run(&scratch, &repo, &["unlink", "TM-1", "blocks", "TM-2"]);
    as_agent(&scratch, &repo, &["link", "TM-2", "blocks", "TM-1"]);

    let (code, doc) = run(&scratch, &repo, &["undo"]);
    assert_eq!(
        (code, doc["error"].as_str()),
        (Some(1), Some("cycle")),
        "{doc}"
    );
    assert_eq!(doc["fix"], Value::Null, "{doc}");
}

#[test]
fn an_agent_names_the_batch_and_a_human_with_nothing_is_told() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let (code, doc) = run(&scratch, &repo, &["undo"]);
    assert_eq!(
        (code, doc["error"].as_str()),
        (Some(1), Some("not_found")),
        "{doc}"
    );
    assert_eq!(doc["kind"], "batch");

    let (code, doc) = as_agent(&scratch, &repo, &["undo"]);
    assert_eq!(
        (code, doc["error"].as_str()),
        (Some(1), Some("batch_required")),
        "{doc}"
    );
    assert_eq!(doc["fix"], "tm undo <batch>");
}
