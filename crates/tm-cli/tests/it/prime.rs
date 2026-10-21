//! `tm prime`: the context a `SessionStart` hook gives an Agent session.

use std::fs;
use std::path::Path;
use std::process::Output;
use std::time::Instant;

use serde_json::{Value, json};
use tm_core::{Actor, Change, ProjectId};
use tm_store::workspace::{PathProject, RepoFile};
use tm_store::{Draft, Store};
use uuid::Uuid;

use crate::support::{Scratch, initialized, text, tm, tm_command};

/// tm-u8g.9's budget: prime for a 200-Issue Workspace fits in 4 KB.
const BUDGET_BYTES: usize = 4096;

/// `tm` as the Agent `claude` writing for `jace`.
fn as_claude(scratch: &Scratch, repo: &Path, args: &[&str]) -> Output {
    tm_command()
        .args(args)
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "claude")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

fn set(store: &mut Store, project: ProjectId, issue: Uuid, field: &str, value: Value) {
    store
        .append(Draft {
            actor: Actor::human("jace"),
            project,
            subject: issue,
            change: Change::SetField {
                field: field.into(),
                label: String::new(),
                value,
                value_label: None,
            },
        })
        .unwrap();
}

/// The indented lines under `heading` in prime's text.
fn under<'a>(shown: &'a str, heading: &str) -> Vec<&'a str> {
    shown
        .lines()
        .skip_while(|line| *line != heading)
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .collect()
}

#[test]
fn prime_for_200_issues_fits_the_budget_and_keeps_readys_order() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let claude = Actor::agent("claude", "jace");
    let jace = Actor::human("jace");
    for n in 1..=200 {
        // Longer than any title prime prints whole: ASCII with a character it
        // escapes, or four-byte emoji, fewer characters than the cap's bytes.
        let title = if n % 2 == 0 {
            format!("Issue {n} {}", "😀".repeat(40))
        } else {
            format!("Issue {n} \u{202e}{}", "long title ".repeat(27))
        };
        let (issue, _) = store.create_issue(&jace, project, "task", &title).unwrap();
        if n % 16 == 0 {
            store.take(&claude, issue, None).unwrap();
        }
        if n % 10 == 5 {
            set(&mut store, project, issue, "status", json!("in_progress"));
        }
    }
    drop(store);

    let started = Instant::now();
    let out = as_claude(&scratch, &repo, &["prime"]);
    let took = started.elapsed();
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    assert!(
        shown.len() <= BUDGET_BYTES,
        "{} bytes over the budget:\n{shown}",
        shown.len()
    );
    eprintln!(
        "tm prime over 200 Issues: {} bytes in {took:?}",
        shown.len()
    );

    let ready = text(&as_claude(&scratch, &repo, &["ready", "--limit", "15"]).stdout);
    assert!(
        ready.lines().all(|line| !line.contains('…')),
        "only prime cuts titles: {ready}"
    );
    let key = |line: &str| line.split_whitespace().next().unwrap_or("").to_owned();
    let first: Vec<String> = ready.lines().take(15).map(key).collect();
    let listed: Vec<String> = shown
        .lines()
        .skip_while(|line| !line.starts_with("Ready ("))
        .skip(1)
        .take(15)
        .map(key)
        .collect();
    assert_eq!(listed, first);
    let title = |key: &str| {
        let line = shown.lines().find(|line| line.contains(key)).unwrap_or("");
        line.trim_start()
            .splitn(4, "  ")
            .nth(3)
            .unwrap_or("")
            .to_owned()
    };
    for (key, kept) in [("TM-1 ", r"\u{202e}"), ("TM-2 ", "😀")] {
        let title = title(key);
        assert!(
            title.len() <= 64 && title.ends_with('…') && title.contains(kept),
            "{key}: {title}"
        );
    }
    assert!(shown.contains("Ready (168):\n"), "{shown}");
    assert!(shown.contains("  and 153 more: tm ready\n"), "{shown}");
    let active: Vec<String> = under(&shown, "In progress (20):")
        .into_iter()
        .map(key)
        .collect();
    assert_eq!(
        active,
        [
            "TM-195", "TM-185", "TM-175", "TM-165", "TM-155", "TM-145", "TM-135", "TM-125",
            "TM-115", "TM-105", "and"
        ],
        "most recently started first: {shown}"
    );
    assert!(
        shown.contains("  and 10 more: tm list category:active\n"),
        "{shown}"
    );
    let claims = under(&shown, "Your Claims:");
    assert_eq!(claims.len(), 11, "{shown}");
    assert_eq!(claims[10], "  and 2 more: tm --json prime lists them all");
    assert!(
        shown.starts_with("tm: Project TM, writing as claude for jace\n"),
        "{shown}"
    );
    json_lists_every_claim_and_the_first_active(&scratch, &repo);
}

/// `--json` prime over the 200-Issue Workspace: every Claim, and the first
/// ten in-progress Issues with their total.
fn json_lists_every_claim_and_the_first_active(scratch: &Scratch, repo: &Path) {
    let out = as_claude(scratch, repo, &["--json", "prime"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["claims"].as_array().map(Vec::len), Some(12));
    let active = &doc["data"]["in_progress"];
    let keys: Vec<&Value> = active["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| &issue["key"])
        .collect();
    assert_eq!(
        (&active["total"], keys.len(), keys[0]),
        (&json!(20), 10, &json!("TM-195"))
    );
}

#[test]
fn in_progress_lists_every_active_status_by_last_update() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    let [started, reviewed, shelved, finished] = ["started", "reviewed", "shelved", "finished"]
        .map(|title| store.create_issue(&jace, project, "task", title).unwrap().0);
    for (issue, field, value) in [
        (started, "status", json!("in_progress")),
        (reviewed, "status", json!("in_review")),
        (shelved, "status", json!("in_progress")),
        (shelved, "archived", json!(true)),
        (finished, "status", json!("done")),
        (started, "priority", json!("high")),
    ] {
        set(&mut store, project, issue, field, value);
    }
    drop(store);

    let shown = text(&as_claude(&scratch, &repo, &["prime"]).stdout);
    assert!(
        shown.contains(
            "In progress (2):\n  TM-1  in_progress  high  started\n  TM-2  in_review  none  reviewed\n\
             Ready (0):\n  nothing is ready\n"
        ),
        "the last updated first, not the last created: {shown}"
    );
}

#[test]
fn a_session_writing_as_a_human_is_warned_in_the_text_an_agent_reads() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let out = tm(&scratch, &repo, &["prime"]);
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    assert!(
        shown.starts_with(
            "warning: this session writes as jace; an Agent sets TM_AGENT to its name\n"
        ),
        "{shown}"
    );
    assert!(shown.contains("Your Claims:\n  none\n"), "{shown}");

    let shown = text(&as_claude(&scratch, &repo, &["prime"]).stdout);
    assert!(!shown.contains("warning:"), "{shown}");
}

#[test]
fn prime_without_an_identity_still_gives_the_scheme_and_ready_queue() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "first")
        .unwrap();
    drop(store);
    let out = tm_command()
        .args(["prime"])
        .current_dir(&repo)
        .env_remove("TM_ACTOR")
        .env_remove("TM_AGENT")
        .env("TM_HOME", scratch.home())
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    assert!(shown.starts_with("warning: no identity: "), "{shown}");
    assert!(shown.contains("writing as nobody\n"), "{shown}");
    assert!(
        shown.contains("In progress (0):\n  none\nReady (1):\n  TM-1  todo  none  first\n"),
        "{shown}"
    );
}

#[test]
fn a_claims_expiry_prints_in_local_time() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let (issue, _) = store
        .create_issue(&Actor::human("jace"), project, "task", "held")
        .unwrap();
    let claim = store
        .take(&Actor::agent("claude", "jace"), issue, Some(3_600_000))
        .unwrap();
    drop(store);
    let out = tm_command()
        .args(["prime"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "claude")
        .env("TZ", "America/Chicago")
        .output()
        .unwrap();
    let expires = jiff::Timestamp::from_millisecond(claim.expires_ms.unwrap())
        .unwrap()
        .in_tz("America/Chicago")
        .unwrap()
        .strftime("%Y-%m-%d %H:%M %Z")
        .to_string();
    let shown = text(&out.stdout);
    assert!(
        shown.contains(&format!(
            "  TM-1  todo  none  held  (Claim {}, expires {expires})\n",
            claim.id
        )),
        "{shown}"
    );
}

#[test]
fn a_directory_mapped_to_a_missing_project_is_a_warning() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let path = repo.join(".config/tm.toml");
    let mut file = RepoFile::parse(&fs::read_to_string(&path).unwrap()).unwrap();
    file.paths.push(PathProject {
        prefix: "gone".into(),
        project: "GONE".into(),
    });
    fs::write(&path, file.render()).unwrap();
    fs::create_dir_all(repo.join("gone")).unwrap();

    let out = as_claude(&scratch, &repo.join("gone"), &["prime"]);
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    assert!(
        shown.starts_with("warning: ") && shown.contains("Project GONE, which this Workspace"),
        "{shown}"
    );
    assert!(
        shown.contains("tm: Project -, writing as claude for jace\n"),
        "{shown}"
    );
}

#[test]
fn json_prime_keeps_warnings_out_of_the_data() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let out = tm(&scratch, &repo, &["--json", "prime"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        doc["warnings"][0],
        "this session writes as jace; an Agent sets TM_AGENT to its name"
    );
    assert_eq!(doc["data"]["project"], "TM");
    assert_eq!(doc["data"]["ready"]["total"], 0);
}
