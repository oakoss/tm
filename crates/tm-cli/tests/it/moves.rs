use std::path::Path;

use serde_json::{Value, json};
use tm_core::Actor;
use tm_store::Store;

use crate::support::{Scratch, initialized, text, tm};

fn run(scratch: &Scratch, repo: &Path, args: &[&str]) -> (Option<i32>, Value) {
    let out = tm(scratch, repo, &[&["--json", "move"], args].concat());
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

#[test]
fn move_sets_the_status_and_its_fields_in_one_batch() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();

    let (code, doc) = run(
        &scratch,
        &repo,
        &[
            "tm-1",
            "In Progress",
            "--set",
            "assignee=jace",
            "--set",
            "priority=high",
        ],
    );
    assert_eq!(code, Some(0), "{doc}");
    let data = &doc["data"];
    assert_eq!(data["fields"]["status"], "in_progress");
    assert_eq!(data["category"], "active");
    assert_eq!(data["fields"]["assignee"], "jace");
    assert_eq!(data["fields"]["priority"], "high");
    let batch = data["batch"].as_str().unwrap();
    let in_batch: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM op_batches WHERE batch = unhex(replace(?1, '-', ''))",
            [batch],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(in_batch, 3, "status and both fields share one batch");
}

#[test]
fn the_text_names_each_change_and_not_the_whole_issue() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let (issue, _) = store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    store
        .take(&Actor::agent("claude", "jace"), issue, None)
        .unwrap();

    let out = tm(
        &scratch,
        &repo,
        &["move", "TM-1", "done", "--set", "assignee=jace"],
    );
    let stdout = text(&out.stdout);
    let (changes, undo) = stdout.split_once("undo this with: ").unwrap();
    assert_eq!(
        changes,
        "TM-1  status: todo → done\nTM-1  assignee: (none) → jace\nTM-1  Claim released\n"
    );
    assert_eq!(undo.lines().count(), 1, "{stdout}");
}

#[test]
fn the_text_leaves_out_what_did_not_change() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let (issue, _) = store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    store
        .take(&Actor::agent("claude", "jace"), issue, None)
        .unwrap();

    let out = tm(
        &scratch,
        &repo,
        &["move", "TM-1", "in_review", "--set", "priority=none"],
    );
    let stdout = text(&out.stdout);
    assert!(
        stdout.starts_with("TM-1  status: todo → in_review\nundo this with: "),
        "a review Status keeps the Claim, and priority was already none: {stdout}"
    );

    let out = tm(&scratch, &repo, &["move", "TM-1", "in_review"]);
    assert!(
        text(&out.stdout).starts_with("TM-1: nothing changed\n"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn a_category_or_unknown_target_is_refused_with_the_statuses_and_writes_nothing() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    let before = ops(&store);

    let (code, doc) = run(&scratch, &repo, &["TM-1", "active"]);
    assert_eq!(code, Some(1));
    assert_eq!(doc["error"], "invalid_value", "{doc}");
    assert_eq!(doc["field"], "status");
    assert_eq!(doc["allowed"], json!(["in_progress", "in_review"]));

    let (code, doc) = run(&scratch, &repo, &["TM-1", "shipped"]);
    assert_eq!(code, Some(1));
    assert_eq!(doc["allowed"].as_array().unwrap().len(), 6, "{doc}");
    assert_eq!(ops(&store), before);

    let out = tm(&scratch, &repo, &["move", "TM-1", "shipped"]);
    assert!(
        text(&out.stderr).contains("one of: triage, todo, in_progress"),
        "{out:?}"
    );
    for set in ["status=done", "labels=x", "parent=TM-1", "assignee"] {
        let (code, doc) = run(&scratch, &repo, &["TM-1", "done", "--set", set]);
        assert_eq!(
            (code, doc["error"].as_str()),
            (Some(2), Some("usage")),
            "{set}"
        );
    }
    let (code, doc) = run(
        &scratch,
        &repo,
        &[
            "TM-1",
            "done",
            "--set",
            "priority=high",
            "--set",
            "priority=none",
        ],
    );
    assert_eq!(
        (code, doc["error"].as_str()),
        (Some(1), Some("invalid_value"))
    );
    assert_eq!(doc["field"], "priority");
    let (code, doc) = run(&scratch, &repo, &["TM-9", "done"]);
    assert_eq!((code, doc["error"].as_str()), (Some(1), Some("not_found")));
    assert_eq!(ops(&store), before);
}
