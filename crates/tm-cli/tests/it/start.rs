//! `tm start`: a Claim, the move to an active Status and the Assignee, as one
//! undo.

use std::path::Path;
use std::process::Output;

use serde_json::{Value, json};
use tm_core::Actor;

use crate::support::{Scratch, initialized, text, tm, tm_command};

/// `tm` as the Agent `claude` for `jace`, in `session`.
fn as_agent(scratch: &Scratch, repo: &Path, session: &str, args: &[&str]) -> Output {
    tm_command()
        .args(args)
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "claude")
        .env("TM_SESSION", session)
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

fn doc(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

fn show(scratch: &Scratch, repo: &Path, key: &str) -> Value {
    doc(&tm(scratch, repo, &["--json", "show", key]))["data"].clone()
}

#[test]
fn an_agent_claims_moves_and_assigns_in_one_undo() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    let (epic, _) = store
        .create_issue(&jace, project, "epic", "The epic")
        .unwrap();
    let (task, _) = store
        .create_issue(&jace, project, "task", "Claim and start work, in one step")
        .unwrap();
    store
        .append(tm_store::Draft {
            actor: jace.clone(),
            project,
            subject: task,
            change: tm_core::Change::SetField {
                field: "parent".into(),
                label: String::new(),
                value: json!(epic.to_string()),
                value_label: None,
            },
        })
        .unwrap();
    drop(store);

    let out = as_agent(&scratch, &repo, "s1", &["start", "TM-2"]);
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    let lines: Vec<&str> = shown.lines().collect();
    assert!(
        lines[0].starts_with("TM-2 claimed by claude (session s1) for jace until "),
        "{shown}"
    );
    assert_eq!(lines[1], "TM-2  status: todo → in_progress", "{shown}");
    assert_eq!(lines[2], "TM-2  assignee: (none) → jace", "{shown}");
    assert!(lines[3].starts_with("Claim "), "{shown}");
    assert_eq!(
        lines[4], "a branch for it: git switch -c tm-2-claim-and-start-work-in",
        "{shown}"
    );
    assert_eq!(
        lines[5], "also started TM-1 (todo → in_progress)",
        "{shown}"
    );
    let batch = lines[6].strip_prefix("undo this with: tm undo ").unwrap();

    let started = show(&scratch, &repo, "TM-2");
    assert_eq!(started["fields"]["status"], "in_progress");
    assert_eq!(started["fields"]["assignee"], "jace");
    assert_eq!(started["claim"]["session"], "s1");
    assert_eq!(
        lines[3],
        format!(
            "Claim {}; release with: tm release TM-2",
            started["claim"]["id"].as_str().unwrap()
        ),
        "the token printed is the Claim's"
    );
    assert_eq!(
        show(&scratch, &repo, "TM-1")["fields"]["status"],
        "in_progress"
    );

    let out = tm(&scratch, &repo, &["undo", batch]);
    assert!(out.status.success(), "{out:?}");
    let undone = show(&scratch, &repo, "TM-2");
    assert_eq!(undone["fields"]["status"], "todo");
    assert_eq!(undone["fields"]["assignee"], Value::Null);
    assert_eq!(undone["claim"], Value::Null, "one undo releases it too");
    assert_eq!(show(&scratch, &repo, "TM-1")["fields"]["status"], "todo");

    let out = tm(&scratch, &repo, &["move", "TM-2", "in_progress"]);
    assert!(out.status.success(), "{out:?}");
    let out = tm(&scratch, &repo, &["move", "TM-1", "todo"]);
    assert!(out.status.success(), "{out:?}");
    assert!(
        as_agent(&scratch, &repo, "s1", &["start", "TM-2"])
            .status
            .success()
    );
    assert_eq!(
        show(&scratch, &repo, "TM-1")["fields"]["status"],
        "todo",
        "an Issue already active starts no parent"
    );
}

#[test]
fn a_held_claim_an_active_status_and_an_assignee_are_kept() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    let out = tm(&scratch, &repo, &["edit", "TM-1", "--set", "assignee=sam"]);
    assert!(out.status.success(), "{out:?}");

    let first = as_agent(&scratch, &repo, "s1", &["--json", "start", "TM-1"]);
    assert!(first.status.success(), "{first:?}");
    let claim = show(&scratch, &repo, "TM-1")["claim"]["id"].clone();
    let out = as_agent(&scratch, &repo, "s1", &["start", "TM-1"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        text(&out.stdout),
        "TM-1 is already claimed by claude (session s1) for jace\n\
         a branch for it: git switch -c tm-1-t\n",
        "nothing else changed"
    );
    let again = show(&scratch, &repo, "TM-1");
    assert_eq!(again["claim"]["id"], claim, "the same Claim");
    assert_eq!(again["fields"]["assignee"], "sam", "never replaced");

    let out = as_agent(&scratch, &repo, "s2", &["--json", "start", "TM-1"]);
    assert_eq!(doc(&out)["error"], "already_claimed", "{out:?}");

    let out = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "u"],
    );
    assert!(out.status.success(), "{out:?}");
    assert!(
        as_agent(&scratch, &repo, "s1", &["claim", "TM-2"])
            .status
            .success()
    );
    let out = as_agent(&scratch, &repo, "s2", &["--json", "start", "TM-2"]);
    assert_eq!(doc(&out)["error"], "already_claimed", "{out:?}");
    let refused = show(&scratch, &repo, "TM-2");
    assert_eq!(
        (&refused["fields"]["status"], &refused["fields"]["assignee"]),
        (&json!("todo"), &Value::Null),
        "a refused take writes nothing"
    );
}

#[test]
fn start_takes_the_claim_for_as_long_as_asked() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["for", "forever"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    let out = as_agent(
        &scratch,
        &repo,
        "s1",
        &["--json", "start", "TM-1", "--for", "30m"],
    );
    let data = &doc(&out)["data"];
    assert_eq!(data["claim"]["session"], "s1", "{out:?}");
    let left = data["claim"]["expires_in_ms"].as_i64().unwrap();
    assert!((1_700_000..=1_800_000).contains(&left), "30m: {left}");
    assert_eq!(data["branch"], "tm-1-for");
    let out = as_agent(&scratch, &repo, "s1", &["start", "TM-2", "--no-expiry"]);
    assert!(
        text(&out.stdout)
            .starts_with("TM-2 claimed by claude (session s1) for jace with no expiry\n"),
        "{out:?}"
    );
}

#[test]
fn a_human_starts_without_a_claim() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    let out = tm(&scratch, &repo, &["start", "TM-1"]);
    assert!(out.status.success(), "{out:?}");
    assert!(
        text(&out.stdout)
            .starts_with("TM-1  status: todo → in_progress\nTM-1  assignee: (none) → jace\n"),
        "{out:?}"
    );
    let started = show(&scratch, &repo, "TM-1");
    assert_eq!(started["claim"], Value::Null);
    let out = tm(&scratch, &repo, &["--json", "start", "TM-1"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(doc(&out)["data"]["branch"], json!("tm-1-t"));
    let out = tm(&scratch, &repo, &["start", "TM-1"]);
    assert!(
        text(&out.stdout).starts_with("TM-1: nothing changed\n"),
        "{out:?}"
    );

    let out = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "u"],
    );
    assert!(out.status.success(), "{out:?}");
    assert!(
        as_agent(&scratch, &repo, "s1", &["claim", "TM-2"])
            .status
            .success()
    );
    let out = tm(&scratch, &repo, &["--json", "start", "TM-2"]);
    assert_eq!(
        doc(&out)["warnings"],
        json!(["TM-2 is claimed by claude (session s1) for jace, who may be working on it"]),
        "{out:?}"
    );
    let out = tm(&scratch, &repo, &["move", "TM-2", "done"]);
    assert!(out.status.success(), "{out:?}");
    let out = as_agent(&scratch, &repo, "s1", &["--json", "start", "TM-2"]);
    assert_eq!(
        doc(&out)["warnings"],
        json!(["TM-2 was not moved: it is finished; tm move reopens it"]),
        "{out:?}"
    );
    assert_eq!(show(&scratch, &repo, "TM-2")["fields"]["status"], "done");
}
