use std::path::Path;
use std::process::{Output, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tm_core::{Actor, Change, ProjectId};
use tm_store::{Draft, Store};
use uuid::Uuid;

use crate::support::{Scratch, initialized, text, tm, tm_command};

/// `tm` writing as the Agent `claude` for `jace`.
fn as_agent(scratch: &Scratch, repo: &Path, args: &[&str]) -> Output {
    as_named(scratch, repo, "claude", args)
}

/// `tm` writing as the Agent `agent` for `jace`.
fn as_named(scratch: &Scratch, repo: &Path, agent: &str, args: &[&str]) -> Output {
    tm_command()
        .args(args)
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", agent)
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn doc(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

/// The Claim `tm show --json` gives the Issue.
fn claim_of(scratch: &Scratch, repo: &Path, key: &str) -> Value {
    doc(&tm(scratch, repo, &["--json", "show", key]))["data"]["claim"].clone()
}

#[test]
fn a_move_to_done_or_cancelled_releases_the_claim_and_review_keeps_it() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["shipped", "dropped"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    for key in ["TM-1", "TM-2"] {
        assert!(as_agent(&scratch, &repo, &["claim", key]).status.success());
    }

    for status in ["in_progress", "in_review"] {
        let out = as_agent(&scratch, &repo, &["move", "TM-1", status]);
        assert!(out.status.success(), "{out:?}");
        assert!(claim_of(&scratch, &repo, "TM-1").is_object(), "{status}");
    }
    let moved = doc(&as_agent(
        &scratch,
        &repo,
        &["--json", "move", "TM-1", "done"],
    ));
    assert_eq!(claim_of(&scratch, &repo, "TM-1"), Value::Null);
    let out = as_agent(&scratch, &repo, &["move", "TM-2", "cancelled"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(claim_of(&scratch, &repo, "TM-2"), Value::Null);

    let batch = moved["data"]["batch"].as_str().unwrap();
    let out = as_agent(&scratch, &repo, &["undo", batch]);
    assert!(out.status.success(), "{out:?}");
    assert!(
        text(&out.stdout)
            .contains("  TM-1's Claim stays released; its Agent takes it again with tm claim\n"),
        "{out:?}"
    );
    let shown = doc(&tm(&scratch, &repo, &["--json", "show", "TM-1"]));
    assert_eq!(shown["data"]["fields"]["status"], "in_review");
    assert_eq!(shown["data"]["claim"], Value::Null);

    // Claimed again before the undo: no note that it stays released.
    assert!(
        as_agent(&scratch, &repo, &["claim", "TM-1"])
            .status
            .success()
    );
    let moved = doc(&as_agent(
        &scratch,
        &repo,
        &["--json", "move", "TM-1", "done"],
    ));
    assert_eq!(claim_of(&scratch, &repo, "TM-1"), Value::Null);
    assert!(
        as_agent(&scratch, &repo, &["claim", "TM-1"])
            .status
            .success()
    );
    let batch = moved["data"]["batch"].as_str().unwrap();
    let out = as_agent(&scratch, &repo, &["--json", "undo", batch]);
    assert_eq!(doc(&out)["data"]["released"], json!([]), "{out:?}");
}

#[test]
fn undoing_a_batch_that_only_released_a_claim_is_refused() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    let claimed = doc(&as_agent(&scratch, &repo, &["--json", "claim", "TM-1"]));
    let token = claimed["data"]["claim"]["id"].as_str().unwrap().to_owned();
    let released = doc(&as_agent(
        &scratch,
        &repo,
        &["--json", "release", "TM-1", &token],
    ));
    let batch = released["data"]["batch"].as_str().unwrap();
    let out = as_agent(&scratch, &repo, &["--json", "undo", batch]);
    assert_eq!(doc(&out)["error"], "cannot_undo", "{out:?}");
    let message = doc(&out)["message"].as_str().unwrap_or("").to_owned();
    assert!(
        message.contains("it released a Claim; take the Issue again with tm claim"),
        "{message}"
    );
}

#[test]
fn an_agent_claims_with_a_token_and_releases_with_it() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();

    let out = as_agent(&scratch, &repo, &["claim", "tm-1"]);
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    let token = shown
        .lines()
        .next()
        .and_then(|line| line.rsplit(' ').next())
        .unwrap()
        .to_owned();
    assert!(
        shown
            .lines()
            .nth(1)
            .unwrap()
            .starts_with("undo this with: tm undo "),
        "{shown}"
    );
    assert!(
        text(&out.stdout)
            .starts_with("TM-1 claimed by claude for jace; release with: tm release TM-1 "),
        "{out:?}"
    );

    let out = as_agent(&scratch, &repo, &["--json", "claim", "TM-1"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(doc(&out)["error"], "already_claimed", "{out:?}");
    assert_eq!(doc(&out)["agent"], "claude");
    assert!(
        !text(&out.stdout).contains(&token),
        "an error never hands out the holder's token: {out:?}"
    );

    let out = as_agent(
        &scratch,
        &repo,
        &["--json", "release", "TM-1", &Uuid::now_v7().to_string()],
    );
    assert_eq!(doc(&out)["error"], "not_claim_holder", "{out:?}");
    assert_eq!(doc(&out)["holder"]["agent"], "claude", "{out:?}");
    assert_eq!(doc(&out)["holder"]["state"], "holds", "{out:?}");
    assert!(
        !text(&out.stdout).contains(&token),
        "an error never hands out the holder's token: {out:?}"
    );
    let out = tm(
        &scratch,
        &repo,
        &["release", "TM-1", &Uuid::now_v7().to_string()],
    );
    assert!(
        !text(&out.stderr).contains(&token),
        "nor does its text: {out:?}"
    );
    let out = as_agent(&scratch, &repo, &["--json", "release", "TM-1", &token]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(doc(&out)["data"]["released"], json!(token));
    let out = tm(&scratch, &repo, &["--json", "show", "TM-1"]);
    assert_eq!(doc(&out)["data"]["claim"], Value::Null);

    let out = as_agent(&scratch, &repo, &["--json", "claim", "TM-1", "--for", "2h"]);
    assert!(out.status.success(), "{out:?}");
    let data = &doc(&out)["data"];
    assert_eq!(data["batch"], data["claim"]["id"], "the take is the batch");
    assert_eq!(data["claim"]["on_behalf_of"], "jace");
    assert!(data["claim"]["expires_ms"].as_i64().unwrap() > 0, "{data}");
}

fn archive(store: &mut Store, project: ProjectId, issue: Uuid) {
    store
        .append(Draft {
            actor: Actor::human("jace"),
            project,
            subject: issue,
            change: Change::SetField {
                field: "archived".into(),
                label: String::new(),
                value: json!(true),
                value_label: None,
            },
        })
        .unwrap();
}

/// Sleeps until the wall clock reaches `ms`, as a `tm` process reads it.
fn wait_until(ms: i64) {
    let now_ms = || {
        i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
    };
    while now_ms() < ms {
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_lapsed_claim_is_shown_as_lapsed_and_another_agent_takes_it() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let mut archived = None;
    for title in ["taken over", "finished", "archived"] {
        let (issue, _) = store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
        archived = Some(issue);
    }
    let mut tokens = Vec::new();
    let mut expires_ms = 0;
    for key in ["TM-1", "TM-2", "TM-3"] {
        let out = as_agent(&scratch, &repo, &["--json", "claim", key, "--for", "1s"]);
        assert!(out.status.success(), "{out:?}");
        let data = &doc(&out)["data"]["claim"];
        tokens.push(data["id"].as_str().unwrap().to_owned());
        expires_ms = expires_ms.max(data["expires_ms"].as_i64().unwrap());
    }
    archive(&mut store, project, archived.unwrap());
    drop(store);
    let prime =
        |agent| doc(&as_named(&scratch, &repo, agent, &["--json", "prime"]))["data"].clone();
    let claims = |agent| prime(agent)["claims"].clone();
    assert_eq!(claims("claude").as_array().unwrap().len(), 3);
    assert_eq!(prime("claude")["lapsed_claims"], json!([]));
    let out = as_agent(&scratch, &repo, &["prime"]);
    assert!(!text(&out.stdout).contains("lapsed"), "{out:?}");
    let out = as_named(&scratch, &repo, "codex", &["claim", "TM-1"]);
    assert!(!out.status.success(), "{out:?}");

    wait_until(expires_ms);
    let lapsed = prime("claude");
    assert_eq!(lapsed["claims"], json!([]));
    assert_eq!(
        lapsed["lapsed_claims"].as_array().unwrap().len(),
        2,
        "an archived Issue's lapsed Claim is not offered"
    );
    let out = tm(&scratch, &repo, &["show", "TM-3"]);
    let shown = text(&out.stdout);
    assert!(shown.contains("Claim by claude for jace lapsed"), "{out:?}");
    assert!(!shown.contains("any Agent may take it"), "{out:?}");
    let out = as_agent(&scratch, &repo, &["prime"]);
    let shown = text(&out.stdout);
    assert!(shown.contains("Your lapsed Claims"), "{out:?}");
    assert!(
        shown.contains(&format!("(Claim {}, lapsed ", tokens[0])),
        "{out:?}"
    );
    assert_eq!(claim_of(&scratch, &repo, "TM-1"), Value::Null);
    let shown = doc(&tm(&scratch, &repo, &["--json", "show", "TM-1"]));
    assert_eq!(shown["data"]["lapsed_claim"]["agent"], "claude");
    let out = tm(&scratch, &repo, &["show", "TM-1"]);
    let shown = text(&out.stdout);
    assert!(
        shown.contains("Claim by claude for jace lapsed 20"),
        "{out:?}"
    );
    assert!(
        shown.contains(&format!("(Claim {}); any Agent may take it", tokens[0])),
        "{out:?}"
    );

    let out = as_named(&scratch, &repo, "codex", &["--json", "claim", "TM-1"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(claim_of(&scratch, &repo, "TM-1")["agent"], "codex");
    let out = as_agent(&scratch, &repo, &["--json", "release", "TM-1", &tokens[0]]);
    assert_eq!(doc(&out)["error"], "not_claim_holder", "{out:?}");
    assert_eq!(doc(&out)["holder"]["state"], "took_over", "{out:?}");
    assert_eq!(doc(&out)["holder"]["agent"], "codex", "{out:?}");

    let out = as_agent(&scratch, &repo, &["move", "TM-2", "done"]);
    assert!(out.status.success(), "{out:?}");
    let shown = doc(&tm(&scratch, &repo, &["--json", "show", "TM-2"]));
    assert_eq!(
        shown["data"]["lapsed_claim"],
        Value::Null,
        "a finishing move releases it"
    );
    assert_eq!(prime("claude")["lapsed_claims"], json!([]));
    let out = as_agent(&scratch, &repo, &["--json", "release", "TM-2", &tokens[1]]);
    assert_eq!(doc(&out)["error"], "not_claim_holder", "{out:?}");
    assert_eq!(doc(&out)["holder"], Value::Null, "{out:?}");
}

#[test]
fn reading_claims_needs_no_time_zone() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    assert!(
        as_agent(&scratch, &repo, &["claim", "TM-1"])
            .status
            .success()
    );
    for args in [&["prime"][..], &["show", "TM-1"], &["ready"]] {
        let out = tm_command()
            .args(args)
            .current_dir(&repo)
            .env("TM_HOME", scratch.home())
            .env("TM_ACTOR", "jace")
            .env("TM_AGENT", "claude")
            .env("TZ", "Bogus/Nowhere")
            .env("GIT_CONFIG_GLOBAL", scratch.git_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
    }
}

#[test]
fn claim_release_and_doctor_answer_in_fixed_shapes() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    let out = as_agent(&scratch, &repo, &["--json", "claim", "TM-1", "--for", "2h"]);
    let claimed = doc(&out);
    insta::assert_json_snapshot!(claimed, {
        ".data.issue.id" => "[id]",
        ".data.claim.id" => "[id]",
        ".data.claim.expires_ms" => "[ms]",
        ".data.batch" => "[id]",
    }, @r#"
    {
      "data": {
        "batch": "[id]",
        "claim": {
          "agent": "claude",
          "expires_ms": "[ms]",
          "id": "[id]",
          "on_behalf_of": "jace"
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
    let token = claimed["data"]["claim"]["id"].as_str().unwrap();
    let out = as_agent(&scratch, &repo, &["--json", "release", "TM-1", token]);
    insta::assert_json_snapshot!(doc(&out), {
        ".data.issue.id" => "[id]",
        ".data.released" => "[id]",
        ".data.batch" => "[id]",
    }, @r#"
    {
      "data": {
        "batch": "[id]",
        "issue": {
          "id": "[id]",
          "key": "TM-1"
        },
        "released": "[id]"
      },
      "schema_version": 1,
      "warnings": []
    }
    "#);
    store
        .connection()
        .execute(
            "INSERT INTO unapplied (op_id, reason) VALUES (?1, 'unknown field')",
            [Uuid::nil().as_bytes()],
        )
        .unwrap();
    let out = tm(&scratch, &repo, &["--json", "doctor"]);
    insta::assert_json_snapshot!(doc(&out), {
        ".data.problems[].seq" => "[seq]",
    }, @r#"
    {
      "data": {
        "lost_claims": [],
        "problems": [
          {
            "op_id": "00000000-0000-0000-0000-000000000000",
            "reason": "unknown field",
            "seq": "[seq]"
          }
        ]
      },
      "schema_version": 1,
      "warnings": []
    }
    "#);
}

#[test]
fn a_human_cannot_claim_and_a_bad_duration_is_a_usage_error() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();

    let out = tm(&scratch, &repo, &["--json", "claim", "TM-1"]);
    assert_eq!(out.status.code(), Some(1));
    let refused = doc(&out);
    assert_eq!(refused["error"], "no_agent", "{refused}");
    assert_eq!(refused["fix"], "TM_AGENT=<name> tm claim TM-1");

    for bad in ["2w", "0m", "soon"] {
        let out = as_agent(&scratch, &repo, &["--json", "claim", "TM-1", "--for", bad]);
        assert_eq!(out.status.code(), Some(2), "{bad}: {out:?}");
        assert_eq!(doc(&out)["error"], "usage");
    }
}

#[test]
fn doctor_lists_lost_claims_apart_from_operations_that_did_not_apply() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    let out = tm(&scratch, &repo, &["doctor"]);
    assert_eq!(text(&out.stdout), "every logged operation applied\n");

    for reason in [
        "lost: a take that saw it unclaimed, but claude holds it",
        "unknown field",
    ] {
        store
            .connection()
            .execute(
                "INSERT INTO unapplied (op_id, reason) VALUES (?1, ?2)",
                (Uuid::now_v7().as_bytes().to_vec(), reason),
            )
            .unwrap();
    }
    let out = tm(&scratch, &repo, &["--json", "doctor"]);
    assert!(out.status.success(), "{out:?}");
    let doc = doc(&out);
    assert_eq!(doc["data"]["lost_claims"].as_array().unwrap().len(), 1);
    assert_eq!(doc["data"]["problems"][0]["reason"], "unknown field");
    assert_eq!(
        doc["warnings"],
        json!([]),
        "doctor reports them, not warns of them"
    );

    let out = tm(&scratch, &repo, &["doctor"]);
    let shown = text(&out.stdout);
    assert!(
        shown.starts_with("operations that did not apply:\n"),
        "{shown}"
    );
    assert!(
        shown.contains("Claims another take or release won:\n"),
        "{shown}"
    );
}
