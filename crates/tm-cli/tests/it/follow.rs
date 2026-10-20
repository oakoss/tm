//! Parents follow their children: starting a child starts its waiting
//! parents in the same batch, and finishing or reopening one hints at the
//! parent's move.

use std::path::Path;

use serde_json::{Value, json};

use crate::support::{Scratch, initialized, text, tm};

fn run(scratch: &Scratch, repo: &Path, args: &[&str]) -> Value {
    let out = tm(scratch, repo, &[&["--json"], args].concat());
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn create(scratch: &Scratch, repo: &Path, issue_type: &str, parent: Option<&str>) {
    let mut args = vec!["create", "--type", issue_type, "--title", "t"];
    if let Some(parent) = parent {
        args.extend(["--parent", parent]);
    }
    run(scratch, repo, &args);
}

fn status(scratch: &Scratch, repo: &Path, key: &str) -> Value {
    run(scratch, repo, &["show", key])["data"]["fields"]["status"].clone()
}

fn keys(entries: &Value) -> Vec<&str> {
    entries
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["key"].as_str().unwrap())
        .collect()
}

#[test]
fn starting_a_child_starts_each_waiting_parent_and_one_undo_reverts_all() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    create(&scratch, &repo, "epic", None);
    create(&scratch, &repo, "story", Some("TM-1"));
    create(&scratch, &repo, "task", Some("TM-2"));

    let doc = run(&scratch, &repo, &["move", "TM-3", "in_review"]);
    let data = &doc["data"];
    assert_eq!(
        data["started"],
        json!([
            { "id": data["started"][0]["id"], "key": "TM-2", "from": "todo", "to": "in_progress" },
            { "id": data["started"][1]["id"], "key": "TM-1", "from": "todo", "to": "in_progress" },
        ]),
        "a parent enters its own first active Status, not the child's"
    );
    assert_eq!(status(&scratch, &repo, "TM-1"), "in_progress");

    let batch = data["batch"].as_str().unwrap();
    let undone = run(&scratch, &repo, &["undo", batch]);
    let mut restored = keys(&undone["data"]["restored"]);
    restored.sort_unstable();
    assert_eq!(restored, ["TM-1", "TM-2", "TM-3"]);
    for key in ["TM-1", "TM-2", "TM-3"] {
        assert_eq!(status(&scratch, &repo, key), "todo", "{key}");
    }
}

#[test]
fn the_text_names_each_parent_started() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    create(&scratch, &repo, "epic", None);
    create(&scratch, &repo, "task", Some("TM-1"));

    let out = tm(&scratch, &repo, &["move", "TM-2", "in_progress"]);
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("also started TM-1 (todo → in_progress)\nundo this with: tm undo "),
        "{stdout}"
    );
}

#[test]
fn a_parent_is_never_moved_backwards_and_an_archived_one_is_skipped_aloud() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    create(&scratch, &repo, "epic", None);
    create(&scratch, &repo, "story", Some("TM-1"));
    create(&scratch, &repo, "task", Some("TM-2"));
    run(&scratch, &repo, &["move", "TM-1", "in_review"]);
    run(&scratch, &repo, &["edit", "TM-2", "--set", "archived=true"]);

    let doc = run(&scratch, &repo, &["move", "TM-3", "in_progress"]);
    assert_eq!(doc["data"]["started"], json!([]));
    assert_eq!(
        doc["warnings"],
        json!(["TM-2 was not started: it is archived"])
    );
    assert_eq!(status(&scratch, &repo, "TM-1"), "in_review");
    assert_eq!(status(&scratch, &repo, "TM-2"), "todo");
}

#[test]
fn only_starting_work_starts_a_parent_and_an_archived_parent_gets_no_hint() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    create(&scratch, &repo, "epic", None);
    create(&scratch, &repo, "task", Some("TM-1"));

    let doc = run(&scratch, &repo, &["move", "TM-2", "triage"]);
    assert_eq!(doc["data"]["started"], json!([]));
    assert_eq!(status(&scratch, &repo, "TM-1"), "todo");

    run(&scratch, &repo, &["move", "TM-1", "done"]);
    run(&scratch, &repo, &["edit", "TM-1", "--set", "archived=true"]);
    let doc = run(&scratch, &repo, &["move", "TM-2", "in_progress"]);
    assert_eq!(doc["data"]["next"], json!([]));
}

#[test]
fn finishing_the_last_open_child_suggests_closing_the_parent() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    create(&scratch, &repo, "epic", None);
    for _ in 0..3 {
        create(&scratch, &repo, "task", Some("TM-1"));
    }
    run(&scratch, &repo, &["edit", "TM-4", "--set", "archived=true"]);
    run(&scratch, &repo, &["move", "TM-2", "in_progress"]);

    let doc = run(&scratch, &repo, &["move", "TM-2", "done"]);
    assert_eq!(doc["data"]["next"], json!([]), "TM-3 is still open");

    let out = tm(&scratch, &repo, &["move", "TM-3", "cancelled"]);
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("TM-1 has no open children left; close it with: tm move TM-1 done\n"),
        "{stdout}"
    );
    let doc = run(&scratch, &repo, &["move", "TM-3", "done"]);
    assert_eq!(doc["data"]["next"], json!(["tm move TM-1 done"]));
    assert_eq!(
        status(&scratch, &repo, "TM-1"),
        "in_progress",
        "only suggested"
    );
}

#[test]
fn an_open_child_under_a_finished_parent_suggests_reopening_it() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    create(&scratch, &repo, "epic", None);
    create(&scratch, &repo, "task", Some("TM-1"));
    run(&scratch, &repo, &["move", "TM-2", "done"]);
    run(&scratch, &repo, &["move", "TM-1", "done"]);

    let out = tm(&scratch, &repo, &["move", "TM-2", "in_progress"]);
    let stdout = text(&out.stdout);
    assert!(
        stdout
            .contains("TM-1 is done but TM-2 is open; reopen it with: tm move TM-1 in_progress\n"),
        "{stdout}"
    );
    assert_eq!(status(&scratch, &repo, "TM-1"), "done");

    let doc = run(
        &scratch,
        &repo,
        &[
            "create", "--type", "task", "--title", "t", "--parent", "TM-1",
        ],
    );
    assert_eq!(doc["data"]["next"], json!(["tm move TM-1 in_progress"]));
}

#[test]
fn creating_a_started_child_starts_its_parents() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    create(&scratch, &repo, "epic", None);
    create(&scratch, &repo, "story", Some("TM-1"));

    let doc = run(
        &scratch,
        &repo,
        &[
            "create", "--type", "task", "--title", "t", "--parent", "TM-2",
        ],
    );
    assert_eq!(
        doc["data"]["started"],
        json!([]),
        "a todo child starts nothing"
    );

    let doc = run(
        &scratch,
        &repo,
        &[
            "create",
            "--type",
            "task",
            "--title",
            "t",
            "--parent",
            "TM-2",
            "--set",
            "status=in_progress",
        ],
    );
    assert_eq!(keys(&doc["data"]["started"]), ["TM-2", "TM-1"]);
    assert_eq!(status(&scratch, &repo, "TM-1"), "in_progress");
}

#[test]
fn a_parent_a_merge_left_missing_starts_nothing_and_blocks_nothing() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    create(&scratch, &repo, "task", None);
    store
        .connection()
        .execute(
            "UPDATE issues SET parent_id = ?1",
            [uuid::Uuid::now_v7().as_bytes()],
        )
        .unwrap();

    let doc = run(&scratch, &repo, &["move", "TM-1", "in_progress"]);
    assert_eq!(doc["data"]["started"], json!([]));
    assert_eq!(status(&scratch, &repo, "TM-1"), "in_progress");
}

#[test]
fn undo_refuses_once_another_write_changed_a_started_parent() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    create(&scratch, &repo, "epic", None);
    create(&scratch, &repo, "task", Some("TM-1"));
    let doc = run(&scratch, &repo, &["move", "TM-2", "in_progress"]);
    let batch = doc["data"]["batch"].as_str().unwrap().to_owned();
    run(&scratch, &repo, &["move", "TM-1", "in_review"]);

    let out = tm(&scratch, &repo, &["--json", "undo", &batch]);
    assert_eq!(out.status.code(), Some(1));
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"], "undo_conflict", "{doc}");
    assert_eq!(status(&scratch, &repo, "TM-2"), "in_progress");
}
