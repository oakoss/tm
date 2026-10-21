use std::path::Path;

use serde_json::{Value, json};
use tm_core::{Actor, Change, ProjectId};
use tm_store::{Draft, Store};
use uuid::Uuid;

use crate::support::{Scratch, fixture, initialized, text, tm, tm_command};

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

/// Text output with each comment heading's time cut off.
fn untimed(shown: &str) -> String {
    shown
        .lines()
        .map(|line| match line.rsplit_once(", ") {
            Some((by, rest)) if !line.starts_with(' ') && rest.contains(':') => {
                format!("{by}{}", &rest[rest.rfind(':').unwrap_or(0)..])
            }
            _ => line.to_owned(),
        })
        .map(|line| line + "\n")
        .collect()
}

/// An instant as `show` prints it in `zone`.
fn in_zone(utc_ms: i64, zone: &str) -> String {
    jiff::Timestamp::from_millisecond(utc_ms)
        .unwrap()
        .in_tz(zone)
        .unwrap()
        .strftime("%Y-%m-%d %H:%M %Z")
        .to_string()
}

#[test]
fn show_prints_times_in_local_time_and_text_across_lines() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let (issue, _) = store
        .create_issue(&Actor::human("jace"), project, "task", "Readable")
        .unwrap();
    change(
        &mut store,
        project,
        issue,
        Change::SetField {
            field: "description".into(),
            label: String::new(),
            value: json!("First line\n\n## Notes\n\tindented\u{1b}[2J"),
            value_label: None,
        },
    );
    change(
        &mut store,
        project,
        issue,
        Change::Comment { body: "hi".into() },
    );
    let (_, doc) = show(&scratch, &repo, "TM-1");
    let created = doc["data"]["fields"]["created"].as_i64().unwrap();
    let said = doc["data"]["comments"][0]["utc_ms"].as_i64().unwrap();

    let out = tm_command()
        .args(["show", "TM-1"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TZ", "America/Chicago")
        .output()
        .unwrap();
    let shown = text(&out.stdout);
    let zone = "America/Chicago";
    assert!(
        shown.contains(&format!("\ncreated: {}\n", in_zone(created, zone))),
        "{shown}"
    );
    assert!(
        shown.contains("\ndescription:\n  First line\n  \n  ## Notes\n  \tindented\\u{1b}[2J\n"),
        "{shown}"
    );
    assert!(
        shown.contains(&format!("\njace, {}:\n  hi\n", in_zone(said, zone))),
        "{shown}"
    );
    assert_eq!(
        doc["data"]["fields"]["created"],
        json!(created),
        "JSON keeps milliseconds"
    );
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
        "comments": [],
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
        "links": [],
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
fn show_names_links_from_this_end_and_gives_comments_in_order() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    let (blocker, _) = store.create_issue(&jace, project, "task", "a").unwrap();
    let (held, _) = store.create_issue(&jace, project, "task", "b").unwrap();
    store.link(&jace, "blocks", blocker, held).unwrap();
    change(
        &mut store,
        project,
        held,
        Change::Comment {
            body: "first\nsecond line".into(),
        },
    );
    store
        .append(Draft {
            actor: Actor::agent("claude", "jace"),
            project,
            subject: held,
            change: Change::Comment {
                body: "on it".into(),
            },
        })
        .unwrap();

    let (ok, doc) = show(&scratch, &repo, "TM-2");
    assert!(ok, "{doc}");
    assert_eq!(
        doc["data"]["links"],
        json!([{
            "type": "blocks",
            "direction": "inward",
            "name": "is blocked by",
            "other": {
                "id": blocker.to_string(),
                "key": "TM-1",
                "status": "todo",
                "category": "todo",
                "archived": false,
            },
        }])
    );
    let comments = doc["data"]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0]["body"], "first\nsecond line");
    assert_eq!(comments[1]["actor"], "claude");
    assert_eq!(comments[1]["on_behalf_of"], "jace");

    let (_, doc) = show(&scratch, &repo, "TM-1");
    assert_eq!(doc["data"]["links"][0]["name"], "blocks");
    assert_eq!(doc["data"]["links"][0]["other"]["key"], "TM-2");

    change(
        &mut store,
        project,
        held,
        Change::Comment {
            body: "\u{1b}[2Jcleared".into(),
        },
    );
    let out = tm(&scratch, &repo, &["show", "TM-2"]);
    let shown = text(&out.stdout);
    assert!(
        !shown.contains('\u{1b}'),
        "a control sequence reached the terminal"
    );
    assert!(shown.contains(r"  \u{1b}[2Jcleared"), "{shown}");
    assert!(shown.contains("is blocked by TM-1\n"), "{shown}");
    assert!(
        untimed(&shown).contains("\njace:\n  first\n  second line\n\nclaude for jace:\n  on it\n"),
        "{shown}"
    );

    let on_it: uuid::Uuid = comments[1]["id"].as_str().unwrap().parse().unwrap();
    change(
        &mut store,
        project,
        held,
        Change::Retract {
            comment: on_it,
            retracted: true,
        },
    );
    let (_, doc) = show(&scratch, &repo, "TM-2");
    let retracted = &doc["data"]["comments"][1];
    assert_eq!(retracted["retracted"], true, "{doc}");
    assert_eq!(
        retracted["body"],
        Value::Null,
        "a retracted comment loses its words"
    );
    assert_eq!(
        retracted["actor"], "claude",
        "but keeps its place and author"
    );
    let out = tm(&scratch, &repo, &["show", "TM-2"]);
    assert!(
        untimed(&text(&out.stdout)).contains("\nclaude for jace: (retracted)\n\njace:"),
        "{out:?}"
    );
}

#[test]
fn a_finished_linked_issue_is_named_with_its_status() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    let [done, dropped, open, held] = ["a", "b", "c", "d"]
        .map(|title| store.create_issue(&jace, project, "task", title).unwrap().0);
    for blocker in [done, dropped, open] {
        store.link(&jace, "blocks", blocker, held).unwrap();
    }
    store.link(&jace, "relates_to", held, done).unwrap();
    for (issue, status) in [(done, "done"), (dropped, "cancelled")] {
        change(
            &mut store,
            project,
            issue,
            Change::SetField {
                field: "status".into(),
                label: String::new(),
                value: json!(status),
                value_label: None,
            },
        );
    }

    let out = tm(&scratch, &repo, &["show", "TM-4"]);
    let shown = text(&out.stdout);
    for line in [
        "is blocked by TM-1 (done)\n",
        "is blocked by TM-2 (cancelled)\n",
        "is blocked by TM-3\n",
        "relates to TM-1 (done)\n",
    ] {
        assert!(shown.contains(line), "{line:?} in {shown}");
    }
    let shown = text(&tm(&scratch, &repo, &["show", "TM-1"]).stdout);
    assert!(
        shown.contains("blocks TM-4\n"),
        "an open Issue is unmarked: {shown}"
    );

    let (_, doc) = show(&scratch, &repo, "TM-4");
    let mut others: Vec<(&Value, &Value)> = doc["data"]["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|link| (&link["other"]["status"], &link["other"]["category"]))
        .collect();
    others.sort_by_key(|(status, _)| status.to_string());
    assert_eq!(
        others,
        [
            (&json!("cancelled"), &json!("cancelled")),
            (&json!("done"), &json!("done")),
            (&json!("done"), &json!("done")),
            (&json!("todo"), &json!("todo")),
        ]
    );
}

#[test]
fn an_archived_linked_issue_is_marked_archived() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    let [shelved, finished, held] =
        ["a", "b", "c"].map(|title| store.create_issue(&jace, project, "task", title).unwrap().0);
    for blocker in [shelved, finished] {
        store.link(&jace, "blocks", blocker, held).unwrap();
    }
    for (issue, field, value) in [
        (finished, "status", json!("done")),
        (shelved, "archived", json!(true)),
        (finished, "archived", json!(true)),
    ] {
        change(
            &mut store,
            project,
            issue,
            Change::SetField {
                field: field.into(),
                label: String::new(),
                value,
                value_label: None,
            },
        );
    }

    let shown = text(&tm(&scratch, &repo, &["show", "TM-3"]).stdout);
    for line in [
        "is blocked by TM-1 (archived)\n",
        "is blocked by TM-2 (done, archived)\n",
    ] {
        assert!(shown.contains(line), "{line:?} in {shown}");
    }

    let (_, doc) = show(&scratch, &repo, "TM-3");
    let archived: Vec<&Value> = doc["data"]["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|link| &link["other"]["archived"])
        .collect();
    assert_eq!(archived, [&json!(true), &json!(true)]);
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
