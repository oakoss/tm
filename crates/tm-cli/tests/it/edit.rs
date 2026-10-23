use std::path::Path;
use std::process::Output;

use serde_json::{Value, json};
use tm_core::scheme::{Field, FieldType, Scheme};
use tm_core::{Actor, Change};
use tm_store::{Draft, Store};

use crate::support::{Scratch, initialized, text, tm, tm_piped};

fn run(scratch: &Scratch, repo: &Path, args: &[&str]) -> (Option<i32>, Value) {
    let out = tm(scratch, repo, &[&["--json", "edit"], args].concat());
    assert_eq!(text(&out.stderr), "", "nothing reaches stderr in JSON mode");
    (
        out.status.code(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

/// `tm edit` with `input` on stdin.
fn piped(scratch: &Scratch, repo: &Path, args: &[&str], input: &str) -> Output {
    tm_piped(scratch, repo, &[&["--json", "edit"], args].concat(), input)
}

fn ops(store: &Store) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM ops", [], |row| row.get(0))
        .unwrap()
}

fn two_issues(scratch: &Scratch) -> (std::path::PathBuf, Store) {
    let (repo, mut store, project) = initialized(scratch);
    for title in ["parent", "child"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    (repo, store)
}

#[test]
fn an_edit_writes_fields_labels_and_parent_in_one_undoable_batch() {
    let scratch = Scratch::new();
    let (repo, store) = two_issues(&scratch);

    let (code, doc) = run(
        &scratch,
        &repo,
        &[
            "TM-2",
            "--set",
            "priority=high",
            "--label",
            "cli",
            "--parent",
            "TM-1",
        ],
    );
    assert_eq!(code, Some(0), "{doc}");
    let data = &doc["data"];
    assert_eq!(data["key"], "TM-2");
    let parent_id = store.issue_by_key("TM-1").unwrap().unwrap().to_string();
    assert_eq!(
        data["changed"],
        json!([
            { "field": "priority", "before": "none", "after": "high" },
            { "field": "labels", "before": [], "after": ["cli"] },
            { "field": "parent", "before": null, "after": { "id": parent_id, "key": "TM-1" } },
        ])
    );
    let batch = data["batch"].as_str().unwrap().to_owned();

    let out = tm(&scratch, &repo, &["undo", &batch]);
    assert!(out.status.success(), "{out:?}");
    let shown: Value =
        serde_json::from_slice(&tm(&scratch, &repo, &["--json", "show", "TM-2"]).stdout).unwrap();
    assert_eq!(
        shown["data"]["fields"]["priority"], "none",
        "one undo reverts all"
    );
    assert_eq!(shown["data"]["parent"], Value::Null);
    assert_eq!(shown["data"]["sets"], json!({}));
}

#[test]
fn an_edit_that_changes_nothing_writes_nothing() {
    let scratch = Scratch::new();
    let (repo, store) = two_issues(&scratch);
    let (code, _) = run(&scratch, &repo, &["TM-2", "--label", "cli"]);
    assert_eq!(code, Some(0));
    let before = ops(&store);

    let (code, doc) = run(
        &scratch,
        &repo,
        &[
            "TM-2",
            "--set",
            "priority=none",
            "--label",
            "CLI",
            "--unset",
            "assignee",
            "--no-parent",
        ],
    );
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(doc["data"]["changed"], json!([]));
    assert_eq!(doc["data"]["batch"], Value::Null);
    assert_eq!(ops(&store), before);
    let out = tm(&scratch, &repo, &["edit", "TM-2", "--remove-label", "nope"]);
    assert_eq!(text(&out.stdout), "TM-2: nothing changed\n");
}

#[test]
fn a_field_reads_stdin_without_its_trailing_newline() {
    let scratch = Scratch::new();
    let (repo, _store) = two_issues(&scratch);
    let out = piped(
        &scratch,
        &repo,
        &["TM-2", "--set", "description=-"],
        "line one\nline two\n",
    );
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["changed"][0]["after"], "line one\nline two");

    let out = piped(&scratch, &repo, &["TM-2", "--set", "description=-"], " \n");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"], "invalid_value", "{doc}");
    assert_eq!(doc["field"], "description");
    assert!(
        doc["reason"].as_str().unwrap().contains("--unset"),
        "a blank value points to clearing the field: {doc}"
    );

    let out = piped(
        &scratch,
        &repo,
        &["TM-2", "--set", "title=-", "--set", "description=-"],
        "x",
    );
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"], "invalid_value", "{doc}");
}

#[test]
fn an_edit_refuses_what_another_command_or_the_store_owns() {
    let scratch = Scratch::new();
    let (repo, store) = two_issues(&scratch);
    let before = ops(&store);

    let (code, doc) = run(&scratch, &repo, &["TM-2", "--set", "status=done"]);
    assert_eq!(code, Some(1));
    assert_eq!(doc["error"], "use_move", "{doc}");
    assert_eq!(doc["fix"], "tm move TM-2 done");

    let (_, doc) = run(&scratch, &repo, &["TM-2", "--unset", "title"]);
    assert_eq!(doc["error"], "missing_fields", "{doc}");
    assert_eq!(
        doc["message"],
        "Issue TM-2 is missing required fields: title"
    );
    assert_eq!(doc["key"], "TM-2");
    assert_eq!(doc["source"], Value::Null);
    assert!(
        doc["issue"].as_str().unwrap().parse::<uuid::Uuid>().is_ok(),
        "{doc}"
    );

    let (code, _) = run(&scratch, &repo, &["TM-2", "--parent", "TM-1"]);
    assert_eq!(code, Some(0));
    let (_, doc) = run(&scratch, &repo, &["TM-1", "--parent", "TM-2"]);
    assert_eq!(doc["error"], "cycle", "{doc}");
    assert_eq!(
        doc["message"],
        "parent would close a cycle: TM-1 -> TM-2 -> TM-1"
    );
    assert_eq!(doc["keys"], json!(["TM-1", "TM-2", "TM-1"]));
    assert_eq!(doc["path"].as_array().unwrap().len(), 3, "{doc}");

    for twice in [
        &["TM-2", "--set", "assignee=a", "--unset", "assignee"][..],
        &["TM-2", "--set", "assignee=a", "--set", "assignee=b"],
    ] {
        let (_, doc) = run(&scratch, &repo, twice);
        assert_eq!(doc["error"], "invalid_value", "{doc}");
        assert_eq!(doc["message"], "assignee is given more than once");
    }

    for usage in [
        &["TM-2"][..],
        &["TM-2", "--set", "labels=x"],
        &["TM-2", "--unset", "parent"],
        &["TM-2", "--parent", "TM-1", "--no-parent"],
    ] {
        let out = tm(&scratch, &repo, &[&["--json", "edit"], usage].concat());
        assert_eq!(out.status.code(), Some(2), "{usage:?}: {out:?}");
    }
    assert_eq!(ops(&store), before + 1, "only the one parent edit wrote");
}

#[test]
fn labels_match_as_the_store_keys_them() {
    let scratch = Scratch::new();
    let (repo, _store) = two_issues(&scratch);
    let (_, doc) = run(&scratch, &repo, &["TM-2", "--label", " Équipe "]);
    assert_eq!(doc["data"]["changed"][0]["after"], json!(["Équipe"]));

    let (_, doc) = run(&scratch, &repo, &["TM-2", "--remove-label", "équipe"]);
    assert_eq!(
        doc["data"]["changed"],
        json!([{ "field": "labels", "before": ["Équipe"], "after": [] }])
    );

    let (_, doc) = run(
        &scratch,
        &repo,
        &["TM-2", "--label", "x", "--remove-label", " X"],
    );
    assert_eq!(doc["error"], "invalid_value", "{doc}");
}

#[test]
fn unset_refuses_a_field_it_cannot_clear() {
    let scratch = Scratch::new();
    let (repo, _store) = two_issues(&scratch);
    let (_, doc) = run(&scratch, &repo, &["TM-2", "--unset", "priority"]);
    assert_eq!(doc["error"], "invalid_value", "{doc}");
    assert_eq!(doc["field"], "priority");

    let (_, doc) = run(&scratch, &repo, &["TM-2", "--unset", "asignee"]);
    assert_eq!(doc["error"], "unknown_field", "{doc}");

    let (_, doc) = run(&scratch, &repo, &["TM-2", "--set", "status=In Progress"]);
    assert_eq!(doc["fix"], "tm move TM-2 'In Progress'");
}

#[test]
fn unset_clears_a_field_the_scheme_no_longer_has() {
    let scratch = Scratch::new();
    let (repo, store) = two_issues(&scratch);
    let mut scheme = Scheme::built_in();
    scheme
        .fields
        .push(Field::custom("area", "Area", FieldType::Number, Vec::new()));
    let mut store = store.with_scheme(scheme);
    let issue = store.issue_by_key("TM-2").unwrap().unwrap();
    store
        .append(Draft {
            actor: Actor::human("jace"),
            project: store.issue(issue).unwrap().unwrap().project,
            subject: issue,
            change: Change::SetField {
                field: "area".into(),
                label: String::new(),
                value: json!(4),
                value_label: None,
            },
        })
        .unwrap();

    let (code, doc) = run(&scratch, &repo, &["TM-2", "--unset", "area"]);
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(
        doc["data"]["changed"],
        json!([{ "field": "area", "before": 4, "after": null }])
    );
}

#[test]
fn an_edit_prints_one_line_per_change() {
    let scratch = Scratch::new();
    let (repo, _store) = two_issues(&scratch);
    let out = tm(
        &scratch,
        &repo,
        &["edit", "TM-2", "--set", "priority=high", "--label", "cli"],
    );
    let printed = text(&out.stdout);
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(
        lines[..2],
        ["TM-2  priority: none → high", "TM-2  labels: (none) → cli"]
    );
    assert!(
        lines[2].starts_with("undo this with: tm undo "),
        "{printed}"
    );

    let long = "a description longer than one line of a terminal, so it is summarized";
    let out = tm(
        &scratch,
        &repo,
        &["edit", "TM-2", "--set", &format!("description={long}")],
    );
    assert!(
        text(&out.stdout).starts_with("TM-2  description: changed (0 → 1 lines)\n"),
        "{out:?}"
    );
}
