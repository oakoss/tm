//! The sessions that worked an Issue, shown with the commands that resume
//! them, and found by `session:`.

use std::path::Path;
use std::process::{Output, Stdio};

use serde_json::Value;
use tm_core::Actor;
use tm_core::actor::SessionId;
use tm_store::Draft;

use crate::support::{Scratch, initialized, jace, text, tm};

const CLAUDE: &str = "50efc9d7-c49a-4377-bb73-793dd6ad2f9e";
const CODEX: &str = "01a190a7-6cab-7f33-ac3c-161870da743b";

/// `tm` in a harness session, named the way the harness names it.
fn in_harness(scratch: &Scratch, repo: &Path, var: &str, session: &str, args: &[&str]) -> Output {
    jace(scratch, repo, args)
        .env(var, session)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn claude(scratch: &Scratch, repo: &Path, args: &[&str]) -> Output {
    in_harness(scratch, repo, "CLAUDE_CODE_SESSION_ID", CLAUDE, args)
}

fn codex(scratch: &Scratch, repo: &Path, args: &[&str]) -> Output {
    in_harness(scratch, repo, "CODEX_SESSION_ID", CODEX, args)
}

fn ok(out: &Output) -> String {
    assert!(out.status.success(), "{out:?}");
    text(&out.stdout)
}

fn data(out: &Output) -> Value {
    serde_json::from_str::<Value>(&ok(out)).unwrap()["data"].clone()
}

#[test]
fn show_lists_each_session_with_what_it_did_and_how_to_resume_it() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let issue = store
        .create_issue(&Actor::human("jace"), project, "task", "Cache the parser")
        .unwrap()
        .0;
    // A session this machine never ran, as a merge would bring one.
    let elsewhere =
        Actor::agent("claude", "jace").in_session(SessionId::parse("far-away").unwrap());
    store
        .append(Draft {
            actor: elsewhere,
            project,
            subject: issue,
            change: tm_core::Change::Comment {
                body: "seen elsewhere".into(),
            },
        })
        .unwrap();
    drop(store);

    ok(&claude(&scratch, &repo, &["start", "TM-1"]));
    ok(&claude(
        &scratch,
        &repo,
        &["comment", "TM-1", "found the cause"],
    ));
    ok(&claude(&scratch, &repo, &["move", "TM-1", "in_review"]));
    ok(&codex(&scratch, &repo, &["comment", "TM-1", "reviewed"]));

    let shown = ok(&tm(&scratch, &repo, &["show", "TM-1"]));
    let Some((_, section)) = shown.split_once("Sessions (3):\n") else {
        panic!("{shown}");
    };
    let lines: Vec<&str> = section.lines().take(7).collect();
    assert!(
        lines[0].starts_with("  codex …70da743b"),
        "the latest first: {shown}"
    );
    assert!(lines[0].ends_with("1 comment"), "{shown}");
    assert!(
        lines[1].ends_with(&format!(" && codex resume {CODEX}")),
        "{shown}"
    );
    assert!(
        lines[2].starts_with("  claude …d6ad2f9e  on "),
        "a take records its branch: {shown}"
    );
    // Starting moved it and made jace its Assignee.
    assert!(
        lines[2].ends_with(
            "claimed, 1 comment, moved to in_progress, in_review, 1 other change  (moved it last)"
        ),
        "{shown}"
    );
    assert!(
        lines[3].starts_with("    cd ") && lines[3].ends_with(&format!(" && claude -r {CLAUDE}")),
        "{shown}"
    );
    assert!(lines[4].starts_with("  claude far-away"), "{shown}");
    assert_eq!(lines[5], "    session far-away, on another machine");
    assert!(
        lines[6].starts_with("  Claude deletes a session unused for 30 days"),
        "{shown}"
    );

    let doc = data(&tm(&scratch, &repo, &["--json", "show", "TM-1"]));
    let sessions = doc["sessions"].as_array().unwrap();
    assert_eq!(sessions[1]["session"], CLAUDE);
    assert_eq!(sessions[1]["claimed"], true);
    assert_eq!(sessions[1]["moved_last"], true);
    assert!(
        sessions[1]["resume"]
            .as_str()
            .unwrap()
            .ends_with(&format!("claude -r {CLAUDE}"))
    );
    assert_eq!(sessions[2]["resume"], Value::Null);
}

#[test]
fn history_prime_and_the_filter_name_the_session() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["one", "two"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    ok(&claude(&scratch, &repo, &["comment", "TM-1", "a"]));
    ok(&codex(&scratch, &repo, &["comment", "TM-2", "b"]));

    let history = ok(&tm(&scratch, &repo, &["history", "TM-1"]));
    assert!(
        history.contains("claude for jace in session …d6ad2f9e"),
        "{history}"
    );

    let prime = ok(&claude(&scratch, &repo, &["prime"]));
    assert!(
        prime.starts_with(&format!(
            "tm: Project TM, writing as claude for jace in session {CLAUDE}\n"
        )),
        "{prime}"
    );
    let prime = data(&claude(&scratch, &repo, &["--json", "prime"]));
    assert_eq!(prime["actor"]["session"], CLAUDE);

    let found = |filter: &str| -> Vec<String> {
        let doc = data(&tm(&scratch, &repo, &["--json", "list", filter]));
        doc["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|issue| issue["key"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(found(&format!("session:{CLAUDE}")), ["TM-1"]);
    assert_eq!(found("session:…70da743b"), ["TM-2"]);
}

#[test]
fn a_question_waiting_on_you_names_the_session_that_asked() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["Pick a cache", "Pick a queue"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    ok(&claude(&scratch, &repo, &["start", "TM-1"]));
    ok(&claude(
        &scratch,
        &repo,
        &["ask", "TM-1", "SQLite or Postgres?"],
    ));
    ok(&claude(&scratch, &repo, &["ask", "TM-2", "NATS or Kafka?"]));
    let prime = ok(&tm(&scratch, &repo, &["prime"]));
    assert!(
        prime
            .matches("      the asking session resumes with: cd ")
            .count()
            == 2
            && prime.contains(&format!(" && claude -r {CLAUDE}\n"))
            && prime
                .matches("Claude deletes a session unused for 30 days")
                .count()
                == 1,
        "the expiry note follows the section once: {prime}"
    );
    let shown = data(&tm(&scratch, &repo, &["--json", "show", "TM-1"]));
    let asker = &shown["sessions"][0];
    assert_eq!(
        (&asker["asked"], &asker["handed"]),
        (&1.into(), &false.into()),
        "a question keeps the Claim for its asker; no handoff: {asker}"
    );
}

#[test]
fn a_claim_or_next_records_the_branch_and_a_sub_agent_resumes_its_parent() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["Claimed", "Taken next"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    let head = crate::support::git(&scratch, &repo, &["symbolic-ref", "--short", "HEAD"]);
    let branch = text(&head.stdout).trim().to_owned();
    ok(&claude(&scratch, &repo, &["claim", "TM-1"]));
    ok(&codex(&scratch, &repo, &["next"]));
    let reviewer = jace(&scratch, &repo, &["comment", "TM-1", "looks right"])
        .env("CLAUDE_CODE_SESSION_ID", CLAUDE)
        .env("TM_AGENT", "reviewer-1234")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    ok(&reviewer);

    let claimed = data(&tm(&scratch, &repo, &["--json", "show", "TM-1"]));
    let sessions = claimed["sessions"].as_array().unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "the sub-agent writes in its parent's session"
    );
    assert_eq!(
        sessions[0]["branch"],
        branch.as_str(),
        "tm claim records it"
    );
    let resume = sessions[0]["resume"].as_str().unwrap();
    assert!(resume.ends_with(&format!("claude -r {CLAUDE}")), "{resume}");
    let next = data(&tm(&scratch, &repo, &["--json", "show", "TM-2"]));
    assert_eq!(
        next["sessions"][0]["branch"],
        branch.as_str(),
        "tm next records it"
    );

    let named = jace(&scratch, &repo, &["comment", "TM-1", "named only"])
        .env("TM_AGENT", "claude")
        .env("TM_SESSION", CLAUDE)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    ok(&named);
    let claimed = data(&tm(&scratch, &repo, &["--json", "show", "TM-1"]));
    let resume = claimed["sessions"][0]["resume"].as_str().unwrap();
    assert!(
        resume.ends_with(&format!("claude -r {CLAUDE}")),
        "a write naming no harness keeps the one known: {resume}"
    );
}

#[test]
fn a_claim_on_a_detached_head_records_no_branch_and_no_warning() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "Detached")
        .unwrap();
    drop(store);
    for args in [
        &["commit", "--quiet", "--allow-empty", "-m", "one"][..],
        &["checkout", "--quiet", "--detach"],
    ] {
        assert!(crate::support::git(&scratch, &repo, args).status.success());
    }
    let out = claude(&scratch, &repo, &["claim", "TM-1"]);
    ok(&out);
    assert_eq!(text(&out.stderr), "", "a detached head is no failure");
    let shown = data(&tm(&scratch, &repo, &["--json", "show", "TM-1"]));
    assert_eq!(shown["sessions"][0]["branch"], Value::Null);
}

#[test]
fn the_expiry_note_follows_the_harness_not_the_agent_name() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["Reviewed", "Codex named claude"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    let named = |var: &str, session: &str, key: &str| {
        let out = jace(&scratch, &repo, &["comment", key, "noted"])
            .env(var, session)
            .env(
                "TM_AGENT",
                if key == "TM-1" { "reviewer" } else { "claude" },
            )
            .stdin(Stdio::null())
            .output()
            .unwrap();
        ok(&out);
    };
    named("CLAUDE_CODE_SESSION_ID", CLAUDE, "TM-1");
    named("CODEX_SESSION_ID", CODEX, "TM-2");
    let note = "Claude deletes a session unused for 30 days";
    let reviewed = ok(&tm(&scratch, &repo, &["show", "TM-1"]));
    assert!(
        reviewed.contains(note),
        "a sub-agent's Claude session: {reviewed}"
    );
    let codex = ok(&tm(&scratch, &repo, &["show", "TM-2"]));
    assert!(
        codex.contains(&format!("codex resume {CODEX}")) && !codex.contains(note),
        "a Codex session named claude: {codex}"
    );
}
