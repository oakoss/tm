use std::path::Path;

use serde_json::{Value, json};
use tm_core::{Actor, Change, ProjectId};
use tm_store::{Draft, Store};
use uuid::Uuid;

use crate::support::{Scratch, fixture, initialized, text, tm};

fn change(store: &mut Store, project: ProjectId, issue: Uuid, change: Change) {
    store
        .append(Draft {
            actor: Actor::human("jace"),
            project,
            subject: issue,
            change,
        })
        .unwrap();
}

fn show(scratch: &Scratch, repo: &Path, reference: &str) -> (bool, Value) {
    let out = tm(scratch, repo, &["show", reference, "--json"]);
    assert_eq!(text(&out.stderr), "", "nothing reaches stderr in JSON mode");
    (
        out.status.success(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

#[test]
fn show_gives_an_issues_fields_parent_labels_and_claim() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let (parent, _) = store
        .create_issue(&Actor::human("jace"), project, "epic", "Ship it")
        .unwrap();
    let (child, _) = store
        .create_issue(&Actor::human("jace"), project, "task", "Write the docs")
        .unwrap();
    change(
        &mut store,
        project,
        child,
        Change::SetField {
            field: "parent".into(),
            label: String::new(),
            value: json!(parent.to_string()),
            value_label: None,
        },
    );
    change(
        &mut store,
        project,
        child,
        Change::Add {
            field: "labels".into(),
            label: String::new(),
            elem: "docs".into(),
            elem_label: None,
        },
    );
    let claim = store
        .take(&Actor::agent("claude", "jace"), child, None)
        .unwrap();

    let (ok, doc) = show(&scratch, &repo, "tm-2");
    assert!(ok, "{doc}");
    assert_eq!(doc["data"]["id"], json!(child.to_string()));
    assert_eq!(doc["data"]["parent"]["id"], json!(parent.to_string()));
    assert_eq!(doc["data"]["claim"]["id"], json!(claim.id.to_string()));
    insta::assert_json_snapshot!(doc, {
        ".data.id" => "[id]",
        ".data.parent.id" => "[id]",
        ".data.fields.parent" => "[id]",
        ".data.claim.id" => "[id]",
        ".data.fields.created" => "[ms]",
        ".data.fields.updated" => "[ms]",
    }, @r#"
    {
      "data": {
        "category": "todo",
        "claim": {
          "agent": "claude",
          "expires_ms": null,
          "id": "[id]",
          "on_behalf_of": "jace"
        },
        "fields": {
          "archived": false,
          "created": "[ms]",
          "parent": "[id]",
          "priority": "none",
          "reporter": "jace",
          "status": "todo",
          "title": "Write the docs",
          "type": "task",
          "updated": "[ms]"
        },
        "id": "[id]",
        "key": "TM-2",
        "parent": {
          "id": "[id]",
          "key": "TM-1"
        },
        "project": "TM",
        "sets": {
          "labels": {
            "docs": "docs"
          }
        }
      },
      "schema_version": 1,
      "warnings": []
    }
    "#);

    let out = tm(&scratch, &repo, &["show", "TM-2"]);
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    assert!(shown.starts_with("TM-2  Write the docs\n"), "{shown}");
    for line in [
        "parent: TM-1\n",
        "labels: docs\n",
        "status: todo\n",
        &format!("claimed by claude for jace (Claim {})\n", claim.id),
    ] {
        assert!(shown.contains(line), "{line:?} in {shown}");
    }
}

#[test]
fn an_issue_is_found_by_its_full_id() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let (issue, _) = store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    let (ok, doc) = show(&scratch, &repo, &issue.to_string());
    assert!(ok, "{doc}");
    assert_eq!(doc["data"]["key"], "TM-1");
    assert_eq!(doc["data"]["claim"], Value::Null);
    assert_eq!(doc["data"]["parent"], Value::Null);
}

#[test]
fn a_reference_to_nothing_is_not_found_and_one_that_is_no_key_is_invalid() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    for reference in ["TM-9", "XX-1", &Uuid::now_v7().to_string()] {
        let (ok, doc) = show(&scratch, &repo, reference);
        assert!(!ok);
        assert_eq!(doc["error"], "not_found", "{doc}");
        assert_eq!(doc["kind"], "issue");
        assert_eq!(doc["reference"], reference);
    }
    let (ok, doc) = show(&scratch, &repo, "first");
    assert!(!ok);
    assert_eq!(doc["error"], "invalid_value", "{doc}");
}

#[test]
fn outside_a_workspace_show_says_to_run_init() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let (ok, doc) = show(&scratch, &repo, "TM-1");
    assert!(!ok);
    assert_eq!(doc["error"], "no_workspace", "{doc}");
    assert_eq!(doc["fix"], "tm init");
}

#[test]
fn operations_that_did_not_apply_are_a_warning_but_a_lost_claim_is_not() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    let unapplied = |reason: &str| {
        store
            .connection()
            .execute(
                "INSERT INTO unapplied (op_id, reason) VALUES (?1, ?2)",
                (Uuid::now_v7().as_bytes().to_vec(), reason),
            )
            .unwrap();
    };
    unapplied("lost: a later take won");
    let (_, doc) = show(&scratch, &repo, "TM-1");
    assert_eq!(doc["warnings"], json!([]));

    unapplied("unknown field");
    let (_, doc) = show(&scratch, &repo, "TM-1");
    assert_eq!(
        doc["warnings"],
        json!(["1 logged operation did not apply in full; tm doctor will list it"])
    );
    let (ok, doc) = show(&scratch, &repo, "TM-9");
    assert!(!ok);
    assert_eq!(doc["error"], "not_found", "{doc}");
    assert_eq!(
        doc["warnings"],
        json!(["1 logged operation did not apply in full; tm doctor will list it"])
    );

    unapplied("unknown field");
    let warning =
        "tm: warning: 2 logged operations did not apply in full; tm doctor will list them\n";
    let out = tm(&scratch, &repo, &["show", "TM-1"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(text(&out.stderr), warning);
    let out = tm(&scratch, &repo, &["show", "TM-9"]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).starts_with(warning), "{out:?}");
}

#[test]
fn a_parent_without_a_key_is_shown_by_its_id() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let (parent, _) = store
        .create_issue(&Actor::human("jace"), project, "epic", "p")
        .unwrap();
    let (child, _) = store
        .create_issue(&Actor::human("jace"), project, "task", "c")
        .unwrap();
    change(
        &mut store,
        project,
        child,
        Change::SetField {
            field: "parent".into(),
            label: String::new(),
            value: json!(parent.to_string()),
            value_label: None,
        },
    );
    store
        .connection()
        .execute("DELETE FROM keys WHERE issue_id = ?1", [parent.as_bytes()])
        .unwrap();
    let out = tm(&scratch, &repo, &["show", "TM-2"]);
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    assert!(shown.contains(&format!("parent: {parent}\n")), "{shown}");
}
