//! Each write and list command's `--json` document, pinned (ADR 0011), with
//! ids and times redacted.

use std::path::Path;

use serde_json::Value;
use tm_core::Actor;

use crate::support::{Scratch, initialized, tm};

fn run(scratch: &Scratch, repo: &Path, args: &[&str]) -> Value {
    let out = tm(scratch, repo, &[&["--json"], args].concat());
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}

/// A repo with TM-1 ("first") blocking TM-2 ("second").
fn two_issues() -> (Scratch, std::path::PathBuf) {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    let (first, _) = store.create_issue(&jace, project, "task", "first").unwrap();
    let (second, _) = store
        .create_issue(&jace, project, "task", "second")
        .unwrap();
    store.link(&jace, "blocks", first, second).unwrap();
    (scratch, repo)
}

#[test]
fn create_answers_in_a_fixed_shape() {
    let (scratch, repo) = two_issues();
    let doc = run(
        &scratch,
        &repo,
        &[
            "create", "--type", "bug", "--title", "Crash", "--label", "ui", "--parent", "TM-1",
        ],
    );
    insta::assert_json_snapshot!(doc, {
        ".data.id" => "[id]",
        ".data.batch" => "[id]",
        ".data.fields.parent" => "[id]",
        ".data.parent.id" => "[id]",
        ".data.fields.created" => "[ms]",
        ".data.fields.updated" => "[ms]",
    }, @r#"
    {
      "data": {
        "batch": "[id]",
        "category": "todo",
        "claim": null,
        "comments": [],
        "fields": {
          "archived": false,
          "created": "[ms]",
          "parent": "[id]",
          "priority": "none",
          "reporter": "jace",
          "status": "todo",
          "title": "Crash",
          "type": "bug",
          "updated": "[ms]"
        },
        "id": "[id]",
        "key": "TM-3",
        "links": [],
        "parent": {
          "id": "[id]",
          "key": "TM-1"
        },
        "project": "TM",
        "sets": {
          "labels": {
            "ui": "ui"
          }
        }
      },
      "schema_version": 1,
      "warnings": []
    }
    "#);
}

#[test]
fn move_answers_in_a_fixed_shape() {
    let (scratch, repo) = two_issues();
    let doc = run(
        &scratch,
        &repo,
        &["move", "TM-1", "in_progress", "--set", "assignee=jace"],
    );
    insta::assert_json_snapshot!(doc, {
        ".data.id" => "[id]",
        ".data.batch" => "[id]",
        ".data.links[].other.id" => "[id]",
        ".data.fields.created" => "[ms]",
        ".data.fields.updated" => "[ms]",
    }, @r#"
    {
      "data": {
        "batch": "[id]",
        "category": "active",
        "claim": null,
        "comments": [],
        "fields": {
          "archived": false,
          "assignee": "jace",
          "created": "[ms]",
          "priority": "none",
          "reporter": "jace",
          "status": "in_progress",
          "title": "first",
          "type": "task",
          "updated": "[ms]"
        },
        "id": "[id]",
        "key": "TM-1",
        "links": [
          {
            "direction": "outward",
            "name": "blocks",
            "other": {
              "id": "[id]",
              "key": "TM-2"
            },
            "type": "blocks"
          }
        ],
        "parent": null,
        "project": "TM",
        "sets": {}
      },
      "schema_version": 1,
      "warnings": []
    }
    "#);
}

#[test]
fn comment_answers_in_a_fixed_shape() {
    let (scratch, repo) = two_issues();
    let doc = run(&scratch, &repo, &["comment", "TM-1", "on it"]);
    insta::assert_json_snapshot!(doc, {
        ".data.issue.id" => "[id]",
        ".data.comment.id" => "[id]",
        ".data.comment.utc_ms" => "[ms]",
        ".data.batch" => "[id]",
    }, @r#"
    {
      "data": {
        "batch": "[id]",
        "comment": {
          "actor": "jace",
          "body": "on it",
          "id": "[id]",
          "on_behalf_of": null,
          "utc_ms": "[ms]"
        },
        "issue": {
          "id": "[id]",
          "key": "TM-1"
        }
      },
      "schema_version": 1,
      "warnings": []
    }
    "#);
}

#[test]
fn link_answers_in_a_fixed_shape() {
    let (scratch, repo) = two_issues();
    let doc = run(&scratch, &repo, &["link", "TM-2", "relates", "to", "TM-1"]);
    insta::assert_json_snapshot!(doc, {
        ".data.link.from.id" => "[id]",
        ".data.link.to.id" => "[id]",
        ".data.batch" => "[id]",
    }, @r#"
    {
      "data": {
        "batch": "[id]",
        "link": {
          "from": {
            "id": "[id]",
            "key": "TM-2"
          },
          "name": "relates to",
          "present": true,
          "to": {
            "id": "[id]",
            "key": "TM-1"
          },
          "type": "relates_to"
        }
      },
      "schema_version": 1,
      "warnings": []
    }
    "#);
}

#[test]
fn list_answers_in_a_fixed_shape() {
    let (scratch, repo) = two_issues();
    let doc = run(&scratch, &repo, &["list", "sort:created-asc"]);
    insta::assert_json_snapshot!(doc, {
        ".data.issues[].id" => "[id]",
        ".data.issues[].fields.created" => "[ms]",
        ".data.issues[].fields.updated" => "[ms]",
    }, @r#"
    {
      "data": {
        "issues": [
          {
            "category": "todo",
            "fields": {
              "archived": false,
              "created": "[ms]",
              "priority": "none",
              "reporter": "jace",
              "status": "todo",
              "title": "first",
              "type": "task",
              "updated": "[ms]"
            },
            "id": "[id]",
            "key": "TM-1",
            "project": "TM",
            "sets": {}
          },
          {
            "category": "todo",
            "fields": {
              "archived": false,
              "created": "[ms]",
              "priority": "none",
              "reporter": "jace",
              "status": "todo",
              "title": "second",
              "type": "task",
              "updated": "[ms]"
            },
            "id": "[id]",
            "key": "TM-2",
            "project": "TM",
            "sets": {}
          }
        ],
        "total": 2
      },
      "schema_version": 1,
      "warnings": []
    }
    "#);
}

#[test]
fn undo_answers_in_a_fixed_shape() {
    let (scratch, repo) = two_issues();
    let moved = run(&scratch, &repo, &["move", "TM-1", "in_progress"]);
    let batch = moved["data"]["batch"].as_str().unwrap().to_owned();
    let doc = run(&scratch, &repo, &["undo", &batch]);
    insta::assert_json_snapshot!(doc, {
        ".data.undone" => "[id]",
        ".data.batch" => "[id]",
        ".data.restored[].issue" => "[id]",
        ".data.issues[].id" => "[id]",
        ".data.issues[].links[].other.id" => "[id]",
        ".data.issues[].fields.created" => "[ms]",
        ".data.issues[].fields.updated" => "[ms]",
    }, @r#"
    {
      "data": {
        "batch": "[id]",
        "dry_run": false,
        "issues": [
          {
            "category": "todo",
            "claim": null,
            "comments": [],
            "fields": {
              "archived": false,
              "created": "[ms]",
              "priority": "none",
              "reporter": "jace",
              "status": "todo",
              "title": "first",
              "type": "task",
              "updated": "[ms]"
            },
            "id": "[id]",
            "key": "TM-1",
            "links": [
              {
                "direction": "outward",
                "name": "blocks",
                "other": {
                  "id": "[id]",
                  "key": "TM-2"
                },
                "type": "blocks"
              }
            ],
            "parent": null,
            "project": "TM",
            "sets": {}
          }
        ],
        "restored": [
          {
            "field": "status",
            "from": "in_progress",
            "issue": "[id]",
            "key": "TM-1",
            "to": "todo"
          }
        ],
        "undone": "[id]"
      },
      "schema_version": 1,
      "warnings": []
    }
    "#);
}

#[test]
fn import_records_answers_in_a_fixed_shape() {
    let (scratch, repo) = two_issues();
    let file = scratch.0.join("records.jsonl");
    let record = serde_json::json!({
        "_type": "issue", "source": "beads:a", "project": "TM", "type": "task",
        "title": "imported", "status": "todo", "labels": ["cli"],
        "comments": [{"id": "c1", "actor": "jace", "body": "hi"}],
    });
    std::fs::write(&file, record.to_string()).unwrap();
    let doc = run(
        &scratch,
        &repo,
        &["import", "--records", file.to_str().unwrap()],
    );
    insta::assert_json_snapshot!(doc, {
        ".data.records[].issue" => "[id]",
        ".data.records[].batches[]" => "[id]",
    }, @r#"
    {
      "data": {
        "created": 1,
        "dry_run": false,
        "projects": [],
        "records": [
          {
            "batches": [
              "[id]"
            ],
            "changed_comments": [],
            "conflicts": [],
            "created": true,
            "issue": "[id]",
            "line": 1,
            "removed": [],
            "source": "beads:a",
            "written": [
              "type",
              "title",
              "status",
              "labels/cli",
              "comments/c1"
            ]
          }
        ],
        "unchanged": 0,
        "updated": 0
      },
      "schema_version": 1,
      "warnings": [
        "a record import keeps each Issue's current state and comments, not its history, operation ids or retracted comments"
      ]
    }
    "#);
}

#[test]
fn import_from_beads_answers_in_a_fixed_shape() {
    let (scratch, repo) = two_issues();
    let file = scratch.0.join("beads.jsonl");
    let bead = serde_json::json!({
        "_type": "issue", "id": "b-1", "title": "imported", "status": "open",
        "issue_type": "chore", "started_at": "2026-01-01T00:00:00Z",
    });
    std::fs::write(&file, bead.to_string()).unwrap();
    let doc = run(
        &scratch,
        &repo,
        &["import", "--from", "beads", file.to_str().unwrap()],
    );
    insta::assert_json_snapshot!(doc, {
        ".data.records[].issue" => "[id]",
        ".data.records[].batches[]" => "[id]",
    }, @r#"
    {
      "data": {
        "created": 1,
        "dropped": {
          "started_at": 1
        },
        "dry_run": false,
        "projects": [],
        "records": [
          {
            "batches": [
              "[id]"
            ],
            "changed_comments": [],
            "conflicts": [],
            "created": true,
            "issue": "[id]",
            "line": 1,
            "removed": [],
            "source": "beads:b-1",
            "written": [
              "type",
              "title",
              "status",
              "labels/beads-type:chore"
            ]
          }
        ],
        "unchanged": 0,
        "updated": 0
      },
      "schema_version": 1,
      "warnings": [
        "a record import keeps each Issue's current state and comments, not its history, operation ids or retracted comments"
      ]
    }
    "#);
}

#[test]
fn ready_answers_in_a_fixed_shape() {
    let (scratch, repo) = two_issues();
    let doc = run(&scratch, &repo, &["ready"]);
    insta::assert_json_snapshot!(doc, {
        ".data.issues[].id" => "[id]",
        ".data.issues[].unblocks[].id" => "[id]",
        ".data.issues[].fields.created" => "[ms]",
        ".data.issues[].fields.updated" => "[ms]",
    }, @r#"
    {
      "data": {
        "issues": [
          {
            "category": "todo",
            "fields": {
              "archived": false,
              "created": "[ms]",
              "priority": "none",
              "reporter": "jace",
              "status": "todo",
              "title": "first",
              "type": "task",
              "updated": "[ms]"
            },
            "id": "[id]",
            "key": "TM-1",
            "project": "TM",
            "sets": {},
            "unblocks": [
              {
                "id": "[id]",
                "key": "TM-2"
              }
            ]
          }
        ],
        "total": 1
      },
      "schema_version": 1,
      "warnings": []
    }
    "#);
}
