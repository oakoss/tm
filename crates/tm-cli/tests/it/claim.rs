use std::path::Path;
use std::process::{Output, Stdio};

use serde_json::{Value, json};
use tm_core::Actor;
use uuid::Uuid;

use crate::support::{Scratch, initialized, text, tm, tm_command};

/// `tm` writing as the Agent `claude` for `jace`.
fn as_agent(scratch: &Scratch, repo: &Path, args: &[&str]) -> Output {
    tm_command()
        .args(args)
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "claude")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn doc(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
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
