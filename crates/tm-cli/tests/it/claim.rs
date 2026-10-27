use std::path::Path;
use std::process::{Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tm_core::{Actor, Change, ProjectId};
use tm_store::{Draft, Store};
use uuid::Uuid;

use crate::support::{Scratch, initialized, text, tm, tm_command, wait_until};

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
fn an_agent_finishes_an_issue_another_session_holds_only_with_force() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["held", "for jace", "lapsing"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    let in_session = |session: &str, args: &[&str]| {
        tm_command()
            .args(args)
            .current_dir(&repo)
            .env("TM_HOME", scratch.home())
            .env("TM_ACTOR", "jace")
            .env("TM_AGENT", "claude")
            .env("TM_SESSION", session)
            .env("GIT_CONFIG_GLOBAL", scratch.git_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    for key in ["TM-1", "TM-2"] {
        let out = in_session("session-one-id", &["claim", key]);
        assert!(out.status.success(), "{out:?}");
    }

    // The same Agent's other session is refused, and nothing is written.
    let out = in_session("session-two-id", &["--json", "move", "TM-1", "done"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let refused = doc(&out);
    assert_eq!(refused["error"], "already_claimed", "{refused}");
    assert_eq!(refused["key"], "TM-1");
    assert_eq!(refused["session"], "session-one-id");
    assert_eq!(refused["fix"], Value::Null, "an override is never a fix");
    let message = refused["message"].as_str().unwrap();
    assert!(
        message.starts_with("Issue TM-1 is held by claude ("),
        "{message}"
    );
    assert!(
        message.ends_with("; finishing it would end that Claim, which tm move --force does"),
        "{message}"
    );
    let shown = doc(&tm(&scratch, &repo, &["--json", "show", "TM-1"]));
    assert_eq!(shown["data"]["fields"]["status"], "todo");
    assert!(shown["data"]["claim"].is_object());

    let out = in_session(
        "session-two-id",
        &["--json", "move", "TM-1", "done", "--force"],
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(claim_of(&scratch, &repo, "TM-1"), Value::Null);

    // Undoing the override reopens the Issue but leaves the Claim released.
    let batch = doc(&out)["data"]["batch"].as_str().unwrap().to_owned();
    let out = in_session("session-two-id", &["undo", &batch]);
    assert!(out.status.success(), "{out:?}");
    let note = "  TM-1's Claim stays released; its Agent takes it again with tm claim\n";
    assert!(text(&out.stdout).contains(note), "{out:?}");
    assert_eq!(claim_of(&scratch, &repo, "TM-1"), Value::Null);
    let out = in_session("session-two-id", &["move", "TM-1", "done"]);
    assert!(out.status.success(), "{out:?}");

    // Neither the human it is held for nor a lapsed Claim needs --force.
    let out = tm(&scratch, &repo, &["move", "TM-2", "cancelled"]);
    assert!(out.status.success(), "{out:?}");
    let out = in_session("session-one-id", &["claim", "TM-3", "--for", "1s"]);
    assert!(out.status.success(), "{out:?}");
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    let out = in_session("session-two-id", &["move", "TM-3", "done"]);
    assert!(
        out.status.success(),
        "a lapsed Claim holds nothing: {out:?}"
    );
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
    let lines: Vec<&str> = shown.lines().collect();
    assert!(
        lines[0].starts_with("TM-1 claimed by claude for jace until ")
            && lines[0].ends_with(
                " (2h); your writes renew it, and tm heartbeat keeps it through quiet work"
            ),
        "{shown}"
    );
    let token = lines[1]
        .strip_prefix("Claim ")
        .and_then(|rest| rest.strip_suffix("; release with: tm release TM-1"))
        .unwrap_or_else(|| panic!("{shown}"))
        .to_owned();
    assert!(lines[2].starts_with("undo this with: tm undo "), "{shown}");

    let out = as_agent(&scratch, &repo, &["--json", "claim", "TM-1"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(doc(&out)["error"], "already_claimed", "{out:?}");
    assert_eq!(doc(&out)["agent"], "claude");
    assert_eq!(doc(&out)["key"], "TM-1");
    let message = doc(&out)["message"].as_str().unwrap().to_owned();
    assert!(message.starts_with("Issue TM-1 is held by "), "{message}");
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
    assert_eq!(doc(&out)["key"], "TM-1");
    let message = doc(&out)["message"].as_str().unwrap().to_owned();
    assert!(message.contains(" does not hold Issue TM-1; "), "{message}");
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
    let stranger = Uuid::now_v7().to_string();
    let out = as_agent(&scratch, &repo, &["--json", "release", "TM-3", &stranger]);
    assert_eq!(doc(&out)["holder"]["state"], "lapsed", "{out:?}");
    assert_eq!(doc(&out)["holder"]["agent"], "claude", "{out:?}");
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
fn each_session_of_an_agent_sees_its_own_claims_and_the_others_apart() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["one's", "two's", "no session's", "two's lapsing"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    let out = as_agent(&scratch, &repo, &["claim", "TM-3"]);
    assert!(out.status.success(), "{out:?}");
    let in_session = |session: &str, args: &[&str]| {
        tm_command()
            .args(args)
            .current_dir(&repo)
            .env("TM_HOME", scratch.home())
            .env("TM_ACTOR", "jace")
            .env("TM_AGENT", "claude")
            .env("TM_SESSION", session)
            .env("GIT_CONFIG_GLOBAL", scratch.git_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let out = in_session("session-one-id", &["claim", "TM-1"]);
    assert!(
        text(&out.stdout).contains("claimed by claude (session session-one-id) for jace"),
        "{out:?}"
    );
    let out = in_session("session-two-id", &["--json", "claim", "TM-2"]);
    assert_eq!(doc(&out)["data"]["claim"]["session"], "session-two-id");

    let out = in_session("session-two-id", &["--json", "claim", "TM-1"]);
    assert_eq!(doc(&out)["error"], "already_claimed", "{out:?}");
    assert_eq!(doc(&out)["session"], "session-one-id", "{out:?}");
    assert!(
        doc(&out)["message"]
            .as_str()
            .unwrap()
            .contains("held by claude (session session-one-id) for jace"),
        "{out:?}"
    );
    let out = in_session(
        "session-two-id",
        &["--json", "release", "TM-1", &Uuid::now_v7().to_string()],
    );
    assert_eq!(doc(&out)["holder"]["session"], "session-one-id", "{out:?}");
    assert!(
        doc(&out)["message"]
            .as_str()
            .unwrap()
            .ends_with("; claude (session session-one-id) holds it for jace"),
        "{out:?}"
    );

    let prime = doc(&in_session("session-one-id", &["--json", "prime"]));
    let keys = |items: &Value| -> Vec<String> {
        items
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["key"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        keys(&prime["data"]["claims"]),
        ["TM-1", "TM-3"],
        "a Claim with no session reads as every session's own"
    );
    assert_eq!(keys(&prime["data"]["other_session_claims"]), ["TM-2"]);
    let out = in_session("session-one-id", &["prime"]);
    let shown = text(&out.stdout);
    assert!(shown.contains("Held by your other sessions:"), "{out:?}");
    assert!(shown.contains("(session session-two-id"), "{out:?}");
    let out = tm(&scratch, &repo, &["show", "TM-1"]);
    assert!(
        text(&out.stdout)
            .contains("claimed by claude (session session-one-id) for jace, expires in 2h (Claim "),
        "{out:?}"
    );

    let out = in_session(
        "session-two-id",
        &["--json", "claim", "TM-4", "--for", "1s"],
    );
    wait_until(doc(&out)["data"]["claim"]["expires_ms"].as_i64().unwrap());
    let lapsed = |session| {
        let prime = doc(&in_session(session, &["--json", "prime"]));
        keys(&prime["data"]["lapsed_claims"])
    };
    assert_eq!(
        lapsed("session-one-id"),
        Vec::<String>::new(),
        "another session's"
    );
    assert_eq!(lapsed("session-two-id"), ["TM-4"]);
    let out = tm(&scratch, &repo, &["show", "TM-4"]);
    assert!(
        text(&out.stdout).contains("Claim by claude (session session-two-id) for jace lapsed"),
        "{out:?}"
    );
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
        ".data.claim.expires_in_ms" => "[ms]",
        ".data.batch" => "[id]",
    }, @r#"
    {
      "data": {
        "batch": "[id]",
        "claim": {
          "agent": "claude",
          "expires_in_ms": "[ms]",
          "expires_ms": "[ms]",
          "handed": false,
          "id": "[id]",
          "on_behalf_of": "jace",
          "session": null
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
        shown.contains("Claims another take, renewal or release won:\n"),
        "{shown}"
    );
}

fn wall_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

#[test]
fn a_claim_lasts_the_repo_files_expiry_unless_told_otherwise() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["one", "two", "three"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    let before = wall_ms();
    let out = as_agent(&scratch, &repo, &["--json", "claim", "TM-1"]);
    let expires = doc(&out)["data"]["claim"]["expires_ms"].as_i64().unwrap();
    assert!(
        (before + 7_200_000..=wall_ms() + 7_200_000).contains(&expires),
        "2h by default: {out:?}"
    );
    let out = as_agent(&scratch, &repo, &["claim", "TM-2", "--no-expiry"]);
    assert!(
        text(&out.stdout).starts_with("TM-2 claimed by claude for jace with no expiry\nClaim "),
        "{out:?}"
    );
    assert_eq!(claim_of(&scratch, &repo, "TM-2")["expires_ms"], Value::Null);

    let file = repo.join(".config/tm.toml");
    let mut repo_file = std::fs::read_to_string(&file).unwrap();
    repo_file.push_str("claim_expiry = \"off\"\n");
    std::fs::write(&file, repo_file).unwrap();
    let out = as_agent(&scratch, &repo, &["--json", "claim", "TM-3"]);
    assert_eq!(
        doc(&out)["data"]["claim"]["expires_ms"],
        Value::Null,
        "{out:?}"
    );

    let out = as_agent(
        &scratch,
        &repo,
        &["claim", "TM-3", "--for", "1h", "--no-expiry"],
    );
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

#[test]
fn a_heartbeat_renews_the_sessions_due_claims() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    let out = tm(&scratch, &repo, &["--json", "heartbeat"]);
    assert_eq!(doc(&out)["error"], "no_agent", "{out:?}");
    let out = as_agent(&scratch, &repo, &["heartbeat"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(text(&out.stdout), "no Claim is due for renewal\n");

    let out = as_agent(&scratch, &repo, &["--json", "claim", "TM-1", "--for", "3s"]);
    let taken = doc(&out)["data"]["claim"]["expires_ms"].as_i64().unwrap();
    wait_until(taken - 2_000 + 50);
    let out = as_agent(&scratch, &repo, &["heartbeat"]);
    assert_eq!(text(&out.stdout), "renewed TM-1\n", "{out:?}");
    let renewed = claim_of(&scratch, &repo, "TM-1")["expires_ms"]
        .as_i64()
        .unwrap();
    assert!(renewed > taken, "{renewed} > {taken}");

    let out = as_agent(&scratch, &repo, &["--json", "heartbeat"]);
    assert_eq!(doc(&out)["data"]["renewed"], json!([]), "{out:?}");
    let out = as_agent(&scratch, &repo, &["--json", "heartbeat", "TM-1"]);
    let id = doc(&tm(&scratch, &repo, &["--json", "show", "TM-1"]))["data"]["id"].clone();
    assert_eq!(
        doc(&out)["data"]["renewed"],
        json!([{ "id": id, "key": "TM-1" }]),
        "named, it is renewed before it is due: {out:?}"
    );

    wait_until(
        claim_of(&scratch, &repo, "TM-1")["expires_ms"]
            .as_i64()
            .unwrap()
            - 2_000
            + 50,
    );
    let out = as_agent(&scratch, &repo, &["move", "TM-1", "in_progress"]);
    assert!(
        !text(&out.stdout).contains("Claim released"),
        "a renewal riding on the move is not a release: {out:?}"
    );
}

#[test]
fn the_holder_or_the_human_it_is_held_for_releases_without_the_token() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    let out = as_agent(&scratch, &repo, &["--json", "release", "TM-1"]);
    let refused = doc(&out);
    assert_eq!(refused["error"], "not_claim_holder", "{out:?}");
    assert_eq!(refused["message"], "TM-1 is unclaimed");
    assert!(refused["issue"].is_string(), "{out:?}");
    assert_eq!(refused["key"], "TM-1", "{out:?}");

    assert!(
        as_agent(&scratch, &repo, &["claim", "TM-1"])
            .status
            .success()
    );
    let out = as_named(&scratch, &repo, "codex", &["--json", "release", "TM-1"]);
    let refused = doc(&out);
    assert_eq!(refused["error"], "not_claim_holder", "{out:?}");
    assert_eq!(
        refused["message"],
        "TM-1 is held by claude for jace, so name its Claim to release it"
    );
    assert_eq!(refused["fix"], "tm release TM-1 <claim>");
    assert_eq!(refused["key"], "TM-1", "{out:?}");
    let out = as_agent(&scratch, &repo, &["release", "TM-1"]);
    assert_eq!(text(&out.stdout), "released TM-1\n", "{out:?}");

    assert!(
        as_agent(&scratch, &repo, &["claim", "TM-1"])
            .status
            .success()
    );
    let out = tm(&scratch, &repo, &["release", "TM-1"]);
    assert_eq!(text(&out.stdout), "released TM-1\n", "{out:?}");
    assert_eq!(claim_of(&scratch, &repo, "TM-1"), Value::Null);
}

#[test]
fn a_session_releases_all_it_holds_and_nothing_else() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["one", "two", "other session's", "codex's", "lapsed"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    let in_session = |session: &str, args: &[&str]| {
        tm_command()
            .args(args)
            .current_dir(&repo)
            .env("TM_HOME", scratch.home())
            .env("TM_ACTOR", "jace")
            .env("TM_AGENT", "claude")
            .env("TM_SESSION", session)
            .env("GIT_CONFIG_GLOBAL", scratch.git_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let out = in_session("s1", &["release", "--mine"]);
    assert_eq!(
        text(&out.stdout),
        "this session holds no Claims\n",
        "{out:?}"
    );
    for key in ["TM-1", "TM-2"] {
        assert!(in_session("s1", &["claim", key]).status.success());
    }
    assert!(in_session("s2", &["claim", "TM-3"]).status.success());
    assert!(
        as_named(&scratch, &repo, "codex", &["claim", "TM-4"])
            .status
            .success()
    );
    let out = in_session("s1", &["--json", "claim", "TM-5", "--for", "1s"]);
    wait_until(doc(&out)["data"]["claim"]["expires_ms"].as_i64().unwrap());

    let out = in_session("s1", &["--json", "release", "--mine"]);
    let released = doc(&out);
    let released: Vec<&str> = released["data"]["released"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        released,
        ["TM-1", "TM-2", "TM-5"],
        "lapsed ones too: {out:?}"
    );
    for (key, held) in [
        ("TM-1", false),
        ("TM-2", false),
        ("TM-3", true),
        ("TM-4", true),
    ] {
        assert_eq!(claim_of(&scratch, &repo, key).is_object(), held, "{key}");
    }
    let shown = doc(&tm(&scratch, &repo, &["--json", "show", "TM-5"]));
    assert_eq!(shown["data"]["lapsed_claim"], Value::Null);
    let out = tm(&scratch, &repo, &["--json", "release", "--mine"]);
    assert_eq!(doc(&out)["error"], "no_agent", "{out:?}");

    for key in ["TM-1", "TM-2"] {
        assert!(in_session("s1", &["claim", key]).status.success());
    }
    let refused = claim_of(&scratch, &repo, "TM-2")["id"].clone();
    let refused = Uuid::parse_str(refused.as_str().unwrap()).unwrap();
    store
        .connection()
        .execute(
            &format!(
                "CREATE TRIGGER refuse BEFORE INSERT ON ops WHEN NEW.payload LIKE '%{refused}%'
                 BEGIN SELECT RAISE(ABORT, 'refused'); END"
            ),
            [],
        )
        .unwrap();
    let out = in_session("s1", &["--json", "release", "--mine"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let failed = doc(&out);
    assert_eq!(failed["error"], "release_failed");
    assert!(
        failed["message"]
            .as_str()
            .unwrap()
            .starts_with("released TM-1; could not release TM-2: "),
        "{failed}"
    );
    assert_eq!(failed["released"][0]["key"], "TM-1");
    assert_eq!(claim_of(&scratch, &repo, "TM-1"), Value::Null);
    assert!(claim_of(&scratch, &repo, "TM-2").is_object());

    for args in [&["release", "TM-3", "--mine"][..], &["release"]] {
        let out = tm(&scratch, &repo, args);
        assert_eq!(out.status.code(), Some(2), "a usage error: {out:?}");
    }
}

#[test]
fn the_hook_heartbeat_tells_the_agent_each_warning_once() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    let run = |dir: &Path, args: &[&str], env: &[(&str, &str)]| {
        let mut command = tm_command();
        command
            .args(args)
            .current_dir(dir)
            .env("TM_HOME", scratch.home())
            .env("TM_AGENT", "claude")
            .env("GIT_CONFIG_GLOBAL", scratch.git_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("TM_ACTOR")
            .envs(env.iter().copied());
        command.output().unwrap()
    };
    let s1 = [("TM_ACTOR", "jace"), ("TM_SESSION", "s1")];
    let hook = |dir: &Path, env: &[(&str, &str)]| {
        let out = run(dir, &["hook", "heartbeat", "claude"], env);
        assert!(out.status.success(), "{out:?}");
        text(&out.stdout)
    };
    let told = |context: &str| {
        format!(
            "{}\n",
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PostToolUse",
                    "additionalContext": context,
                },
            })
        )
    };
    let take_over = || {
        let out = run(&repo, &["--json", "claim", "TM-1", "--for", "1s"], &s1);
        wait_until(doc(&out)["data"]["claim"]["expires_ms"].as_i64().unwrap());
        let out = as_named(
            &scratch,
            &repo,
            "codex",
            &["--json", "claim", "TM-1", "--for", "1s"],
        );
        assert!(out.status.success(), "{out:?}");
        wait_until(doc(&out)["data"]["claim"]["expires_ms"].as_i64().unwrap());
    };
    assert_eq!(hook(&repo, &s1), "", "nothing to say");

    let lapsed = "tm: TM-1 is now held by codex for jace; your Claim lapsed";
    take_over();
    assert_eq!(hook(&repo, &s1), told(lapsed));
    assert_eq!(hook(&repo, &s1), "", "told once");
    assert!(scratch.home().join("hooks/s1.reported").is_file());
    take_over();
    assert_eq!(hook(&repo, &s1), told(lapsed), "a second takeover is news");

    let elsewhere = scratch.home().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    assert_eq!(hook(&elsewhere, &s1), "", "tm tracks nothing there");
    let broken = [("TM_SESSION", "s1"), ("TM_JSON", "1")];
    let cannot = "tm: cannot tell who you are; set TM_ACTOR or git's user.email";
    assert_eq!(
        hook(&repo, &broken),
        told(cannot),
        "TM_JSON keeps the format"
    );
    assert_eq!(hook(&repo, &broken), "", "once");
    assert_eq!(hook(&repo, &s1), "");
    assert_eq!(hook(&repo, &broken), told(cannot), "again after it worked");

    let harness = [("CLAUDE_CODE_SESSION_ID", "s3")];
    assert_eq!(hook(&repo, &harness), told(cannot));
    assert_eq!(hook(&repo, &harness), "", "the harness's session keys it");
    assert!(scratch.home().join("hooks/s3.reported").is_file());
}

#[test]
fn claims_lists_who_holds_what_and_what_has_lapsed() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["held", "lapsed", "archived"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    let out = tm(&scratch, &repo, &["claims"]);
    assert_eq!(
        text(&out.stdout),
        "Held (0):\n  none\nLapsed, any Agent may take (0):\n  none\n"
    );

    assert!(
        as_agent(&scratch, &repo, &["claim", "TM-1"])
            .status
            .success()
    );
    let mut expires_ms = 0;
    for key in ["TM-2", "TM-3"] {
        let out = as_named(
            &scratch,
            &repo,
            "codex",
            &["--json", "claim", key, "--for", "1s"],
        );
        assert!(out.status.success(), "{out:?}");
        expires_ms = doc(&out)["data"]["claim"]["expires_ms"].as_i64().unwrap();
    }
    let out = tm(&scratch, &repo, &["edit", "TM-3", "--set", "archived=true"]);
    assert!(out.status.success(), "{out:?}");
    wait_until(expires_ms);

    let out = tm(&scratch, &repo, &["claims"]);
    let shown = text(&out.stdout);
    let lines: Vec<&str> = shown.lines().collect();
    assert_eq!(lines[0], "Held (1):", "{shown}");
    assert!(
        lines[1].starts_with("  TM-1  todo  none  held  (claude for jace, expires in 2h)"),
        "{shown}"
    );
    assert_eq!(lines[2], "Lapsed, any Agent may take (1):", "{shown}");
    assert!(
        lines[3].starts_with("  TM-2  todo  none  lapsed  (codex for jace, lapsed "),
        "{shown}"
    );
    assert_eq!(lines.len(), 4, "the archived Issue is left out: {shown}");

    let out = tm(&scratch, &repo, &["--json", "claims"]);
    let data = &doc(&out)["data"];
    assert_eq!(data["held"][0]["issue"]["key"], "TM-1");
    assert!(data["held"][0]["claim"]["expires_in_ms"].as_i64().unwrap() > 7_000_000);
    assert_eq!(data["lapsed"][0]["issue"]["key"], "TM-2");
    assert_eq!(data["lapsed"][0]["claim"]["expires_in_ms"], 0);
}

#[test]
fn a_failed_takeover_check_after_a_write_is_only_a_warning() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    let out = as_agent(&scratch, &repo, &["--json", "claim", "TM-1", "--for", "1s"]);
    let taken = &doc(&out)["data"]["claim"];
    wait_until(taken["expires_ms"].as_i64().unwrap());
    assert!(
        as_named(&scratch, &repo, "codex", &["claim", "TM-1"])
            .status
            .success()
    );
    // Unreadable, the old take fails the check that follows the write.
    let mine = Uuid::parse_str(taken["id"].as_str().unwrap()).unwrap();
    let conn = store.connection();
    let trigger: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'ops_immutable'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute_batch("DROP TRIGGER ops_immutable").unwrap();
    conn.execute(
        "UPDATE ops SET payload = '{\"kind\":\"bogus\"}' WHERE op_id = ?1",
        [mine.as_bytes()],
    )
    .unwrap();
    conn.execute_batch(&trigger).unwrap();

    let out = as_agent(&scratch, &repo, &["--json", "comment", "TM-1", "landed"]);
    assert!(out.status.success(), "{out:?}");
    let warnings = doc(&out)["warnings"].clone();
    assert!(
        warnings[0].as_str().unwrap().starts_with(
            "the write landed, but whether another Agent took over your Claims is unknown: "
        ),
        "{warnings}"
    );
    let shown = doc(&tm(&scratch, &repo, &["--json", "show", "TM-1"]));
    assert!(shown.to_string().contains("landed"), "{shown}");

    let out = as_agent(&scratch, &repo, &["--json", "heartbeat"]);
    assert!(out.status.success(), "{out:?}");
    let warnings = doc(&out)["warnings"].clone();
    assert!(
        warnings[0]
            .as_str()
            .unwrap()
            .starts_with("renewed, but whether another Agent took over your Claims is unknown: "),
        "{warnings}"
    );
}

#[test]
fn writing_to_an_issue_taken_over_warns_its_old_holder() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["taken over", "another"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    let out = as_agent(&scratch, &repo, &["--json", "claim", "TM-1", "--for", "1s"]);
    wait_until(doc(&out)["data"]["claim"]["expires_ms"].as_i64().unwrap());
    assert!(
        as_named(&scratch, &repo, "codex", &["claim", "TM-1"])
            .status
            .success()
    );

    let out = as_agent(
        &scratch,
        &repo,
        &["--json", "comment", "TM-1", "still here"],
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        doc(&out)["warnings"],
        json!(["TM-1 is now held by codex for jace; your Claim lapsed"])
    );
    for args in [
        &["--json", "heartbeat"][..],
        &["--json", "heartbeat", "TM-1"],
        &["--json", "heartbeat", "TM-2"],
    ] {
        let out = as_agent(&scratch, &repo, args);
        assert_eq!(doc(&out)["data"]["renewed"], json!([]), "{out:?}");
        assert_eq!(
            doc(&out)["warnings"],
            json!(["TM-1 is now held by codex for jace; your Claim lapsed"]),
            "{args:?}"
        );
    }
    let out = as_named(
        &scratch,
        &repo,
        "codex",
        &["--json", "comment", "TM-1", "mine"],
    );
    assert_eq!(doc(&out)["warnings"], json!([]), "{out:?}");
}
