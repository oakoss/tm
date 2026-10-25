use std::path::Path;

use serde_json::{Value, json};
use tm_core::Actor;
use tm_store::Store;

use crate::support::{Scratch, initialized, text, tm, tm_piped};

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

/// A move writes only what changes: the current Status alone writes no
/// batch, and with a field set writes only that field. A finished Issue a
/// Claim still holds is let go by moving it again.
#[test]
fn a_move_writes_only_what_changes() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let (issue, _) = store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    let before = ops(&store);

    let (code, doc) = run(&scratch, &repo, &["TM-1", "todo"]);
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(doc["data"]["batch"], Value::Null, "{doc}");
    assert_eq!(doc["data"]["fields"]["status"], "todo");
    assert_eq!(ops(&store), before, "nothing written");

    let (_, doc) = run(&scratch, &repo, &["TM-1", "todo", "--set=priority=high"]);
    assert_eq!(doc["data"]["fields"]["priority"], "high", "{doc}");
    assert_eq!(ops(&store), before + 1, "the priority alone");

    run(&scratch, &repo, &["TM-1", "done"]);
    store
        .take(&Actor::agent("claude", "jace"), issue, None)
        .unwrap();
    let out = tm(&scratch, &repo, &["move", "TM-1", "done"]);
    let stdout = text(&out.stdout);
    assert!(stdout.starts_with("TM-1  Claim released\n"), "{stdout}");
    assert_eq!(store.claim_state(issue).unwrap(), None);
}

/// `FIELD=-` reads the value from stdin as it is, not a literal `-`.
#[test]
fn a_move_reads_a_field_from_stdin() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    let args = ["--json", "move", "TM-1", "done", "--set", "description=-"];
    let body = "shipped `x` for $5\nsee 'notes'\n";
    let out = tm_piped(&scratch, &repo, &args, body);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{doc}");
    let description = &doc["data"]["fields"]["description"];
    assert_eq!(description, "shipped `x` for $5\nsee 'notes'");

    let before = ops(&store);
    let out = tm_piped(&scratch, &repo, &args, " \n");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["reason"], "is blank; leave the field out", "{doc}");
    assert_eq!(ops(&store), before, "a blank pipe writes nothing");
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
