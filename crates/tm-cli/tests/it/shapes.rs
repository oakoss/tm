//! Each write and list command's `--json` document, pinned (ADR 0011), with
//! ids and times redacted.

use std::path::Path;

use serde_json::Value;
use tm_core::Actor;

use crate::support::{Scratch, initialized, tm, tm_command};

fn run(scratch: &Scratch, repo: &Path, args: &[&str]) -> Value {
    let out = tm(scratch, repo, &[&["--json"], args].concat());
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    // A parsed document rewrites its keys sorted, so this pins the bytes
    // too, including a list's, which is written without a parse.
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{doc}\n"));
    doc
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
        "children": {
          "archived": 0,
          "by_status": []
        },
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
        "lapsed_claim": null,
        "links": [],
        "next": [],
        "parent": {
          "id": "[id]",
          "key": "TM-1"
        },
        "project": "TM",
        "sets": {
          "labels": {
            "ui": "ui"
          }
        },
        "started": []
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
        "children": {
          "archived": 0,
          "by_status": []
        },
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
        "lapsed_claim": null,
        "links": [
          {
            "direction": "outward",
            "name": "blocks",
            "other": {
              "archived": false,
              "category": "todo",
              "id": "[id]",
              "key": "TM-2",
              "status": "todo"
            },
            "type": "blocks"
          }
        ],
        "next": [],
        "parent": null,
        "project": "TM",
        "sets": {},
        "started": []
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
            "children": {
              "archived": 0,
              "by_status": []
            },
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
            "lapsed_claim": null,
            "links": [
              {
                "direction": "outward",
                "name": "blocks",
                "other": {
                  "archived": false,
                  "category": "todo",
                  "id": "[id]",
                  "key": "TM-2",
                  "status": "todo"
                },
                "type": "blocks"
              }
            ],
            "parent": null,
            "project": "TM",
            "sets": {}
          }
        ],
        "released": [],
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
#[expect(
    clippy::too_many_lines,
    reason = "the inline snapshot is the whole document"
)]
fn prime_answers_in_a_fixed_shape() {
    let (scratch, repo) = two_issues();
    let doc = run(&scratch, &repo, &["prime"]);
    insta::assert_json_snapshot!(doc, {
        ".data.habits" => "[habits]",
        ".data.ready.issues[].id" => "[id]",
        ".data.ready.issues[].fields.created" => "[ms]",
        ".data.ready.issues[].fields.updated" => "[ms]",
    }, @r#"
    {
      "data": {
        "actor": {
          "agent": false,
          "name": "jace",
          "on_behalf_of": null
        },
        "claims": [],
        "habits": "[habits]",
        "in_progress": {
          "issues": [],
          "total": 0
        },
        "lapsed_claims": [],
        "other_session_claims": [],
        "project": "TM",
        "ready": {
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
            }
          ],
          "total": 1
        },
        "scheme": {
          "name": "default",
          "required": [
            "type",
            "title",
            "status"
          ],
          "types": [
            "epic",
            "story",
            "task",
            "bug",
            "sub_task",
            "spike",
            "design"
          ],
          "workflows": [
            {
              "name": "default",
              "statuses": [
                {
                  "category": "triage",
                  "id": "triage"
                },
                {
                  "category": "todo",
                  "id": "todo"
                },
                {
                  "category": "active",
                  "id": "in_progress"
                },
                {
                  "category": "active",
                  "id": "in_review"
                },
                {
                  "category": "done",
                  "id": "done"
                },
                {
                  "category": "cancelled",
                  "id": "cancelled"
                }
              ]
            }
          ]
        }
      },
      "schema_version": 1,
      "warnings": [
        "this session writes as jace; an Agent sets TM_AGENT to its name"
      ]
    }
    "#);
}

#[test]
fn primes_claims_answer_in_a_fixed_shape() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let claude = Actor::agent("claude", "jace");
    let (issue, _) = store
        .create_issue(&Actor::human("jace"), project, "task", "held")
        .unwrap();
    store.take(&claude, issue, Some(3_600_000)).unwrap();
    drop(store);
    let out = tm_command()
        .args(["--json", "prime"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "claude")
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    insta::assert_json_snapshot!(doc["data"]["claims"], {
        "[].id" => "[id]",
        "[].claim.id" => "[id]",
        "[].claim.expires_ms" => "[ms]",
        "[].claim.expires_in_ms" => "[ms]",
        "[].fields.created" => "[ms]",
        "[].fields.updated" => "[ms]",
    }, @r#"
    [
      {
        "category": "todo",
        "claim": {
          "agent": "claude",
          "expires_in_ms": "[ms]",
          "expires_ms": "[ms]",
          "id": "[id]",
          "on_behalf_of": "jace",
          "session": null
        },
        "fields": {
          "archived": false,
          "created": "[ms]",
          "priority": "none",
          "reporter": "jace",
          "status": "todo",
          "title": "held",
          "type": "task",
          "updated": "[ms]"
        },
        "id": "[id]",
        "key": "TM-1",
        "project": "TM",
        "sets": {}
      }
    ]
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
