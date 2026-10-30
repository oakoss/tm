//! `tm git scan`: Issues a `Closes` line names on the default branch move to
//! done, with a comment naming where they shipped.

use std::path::Path;
use std::process::Stdio;

use serde_json::{Value, json};
use tm_core::{Actor, Change, ProjectId};
use tm_store::{Draft, Store};
use uuid::Uuid;

use crate::support::{Scratch, git, initialized, text, tm, tm_command};

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

/// An empty commit on the current branch, returning its id.
fn commit(scratch: &Scratch, repo: &Path, subject: &str, body: &str) -> String {
    let message = format!("{subject}\n\n{body}");
    let args = ["commit", "-q", "--allow-empty", "-m", &message];
    git(scratch, repo, &args);
    text(&git(scratch, repo, &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_owned()
}

fn scan(scratch: &Scratch, repo: &Path) -> Value {
    let out = tm(scratch, repo, &["--json", "git", "scan"]);
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}

fn show(scratch: &Scratch, repo: &Path, key: &str) -> Value {
    let out = tm(scratch, repo, &["--json", "show", key]);
    serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone()
}

fn status(scratch: &Scratch, repo: &Path, key: &str) -> Value {
    show(scratch, repo, key)["fields"]["status"].clone()
}

#[test]
fn closes_lines_on_the_default_branch_move_their_issues_to_done() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    let mut new = |title: &str| store.create_issue(&jace, project, "task", title).unwrap().0;
    let titles = ["one", "two", "x", "side", "colon", "low", "shelf", "hand"];
    let [_, two, cancelled, _, _, low, shelved, by_hand] = titles.map(&mut new);
    store.ask(&jace, low, "Which?", "kim", None).unwrap();
    set(&mut store, project, two, "status", json!("in_review"));
    set(&mut store, project, cancelled, "status", json!("cancelled"));
    set(&mut store, project, shelved, "archived", json!(true));
    set(&mut store, project, by_hand, "status", json!("done"));
    let claude = Actor::agent("claude", "jace");
    store.take(&claude, two, None).unwrap();

    let first = commit(&scratch, &repo, "feat: one (#7)", "Closes TM-1, TM-2.");
    let body = "Closes TM-1\nCloses TM-3, TM-8";
    let again = commit(&scratch, &repo, "fix: again", body);
    let others = "Closes TM-99\nCloses ENG-1";
    commit(&scratch, &repo, "chore: others", others);
    commit(&scratch, &repo, "fix: colon", "  Closes: TM-5");
    commit(&scratch, &repo, "fix: lower", "closes tm-6\ncloses TM-7");
    git(&scratch, &repo, &["switch", "--quiet", "-c", "side"]);
    commit(&scratch, &repo, "feat: unmerged", "Closes TM-4");
    git(&scratch, &repo, &["switch", "--quiet", "main"]);

    let dry = tm(&scratch, &repo, &["--json", "git", "scan", "--dry-run"]);
    let dry: Value = serde_json::from_slice(&dry.stdout).unwrap();
    assert_eq!(dry["data"]["dry_run"], true);
    assert_eq!(dry["data"]["moved"][0]["batch"], Value::Null);
    assert_eq!(status(&scratch, &repo, "TM-1"), "todo", "nothing written");
    let dry = text(&tm(&scratch, &repo, &["git", "scan", "--dry-run"]).stdout);
    assert!(dry.ends_with("dry run: nothing was written\n"), "{dry}");

    let out = tm(&scratch, &repo, &["git", "scan"]);
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    let short = &first[..7];
    let one = format!("TM-1  status: todo → done  ({short}, #7)\nTM-1: undo with tm undo ");
    assert!(shown.starts_with(&one), "{shown}");
    let two_line = format!("TM-2  status: in_review → done  ({short}, #7)\n");
    assert!(shown.contains(&two_line), "{shown}");
    let warned = text(&out.stderr);
    assert!(warned.contains("TM-3 is cancelled"), "{warned}");
    let unknown = "closes TM-99, which is no Issue";
    assert!(warned.contains(unknown), "{warned}");
    assert!(!warned.contains("ENG-1"), "{warned}");
    let local = "scanned local main, since it tracks no remote branch";
    assert!(warned.contains(local), "{warned}");

    let one = show(&scratch, &repo, "TM-1");
    assert_eq!(one["fields"]["status"], "done");
    let body = &one["comments"][0]["body"];
    let both = format!("shipped in {first}, #7; also closed by {again}");
    assert_eq!(*body, both);
    let hand = show(&scratch, &repo, "TM-8");
    assert_eq!(hand["comments"], json!([]), "done by hand is left alone");
    let two = show(&scratch, &repo, "TM-2");
    assert_eq!(two["fields"]["status"], "done");
    assert_eq!(two["claim"], Value::Null, "finishing releases it");
    assert_eq!(status(&scratch, &repo, "TM-3"), "cancelled");
    assert_eq!(status(&scratch, &repo, "TM-4"), "todo", "not on main");
    assert_eq!(status(&scratch, &repo, "TM-5"), "done", "with a colon");
    assert_eq!(status(&scratch, &repo, "TM-6"), "done", "in lowercase");
    let six = show(&scratch, &repo, "TM-6");
    assert_eq!(
        six["fields"].get("waiting_on"),
        None,
        "finished waits on no one"
    );
    assert_eq!(status(&scratch, &repo, "TM-7"), "todo", "archived");

    let reopen = tm(&scratch, &repo, &["move", "TM-1", "todo"]);
    assert!(reopen.status.success(), "{reopen:?}");
    let out = tm(&scratch, &repo, &["git", "scan"]);
    let nothing = "nothing to move: no new Closes line on main names an open Issue\n";
    assert_eq!(text(&out.stdout), nothing);
    assert_eq!(status(&scratch, &repo, "TM-1"), "todo", "a reopen stays");
    let dry = tm(&scratch, &repo, &["git", "scan", "--dry-run"]);
    let marked = format!("{nothing}dry run: nothing was written\n");
    assert_eq!(text(&dry.stdout), marked, "a dry run always says so");

    commit(&scratch, &repo, "fix: really (#11)", "Closes TM-1");
    let doc = scan(&scratch, &repo);
    assert_eq!(doc["data"]["moved"][0]["pr"], 11, "a new Closes line");
}

#[test]
fn an_agents_scan_finishes_an_issue_another_agent_holds() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    let issue = store
        .create_issue(&jace, project, "task", "held")
        .unwrap()
        .0;
    store
        .take(&Actor::agent("claude", "jace"), issue, None)
        .unwrap();
    commit(&scratch, &repo, "feat: held (#3)", "Closes TM-1");

    let out = tm_command()
        .args(["--json", "git", "scan"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "codex")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let held = show(&scratch, &repo, "TM-1");
    assert_eq!(held["fields"]["status"], "done");
    assert_eq!(held["claim"], Value::Null, "a merged Closes line ends it");
}

#[test]
fn a_renamed_projects_old_and_new_keys_close_one_issue_once() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    store.create_issue(&jace, project, "task", "t").unwrap();
    let a = commit(&scratch, &repo, "fix: a (#1)", "Closes TM-1");
    let renamed = store.rename_project(&jace, project, Some("NEW"), None);
    renamed.unwrap();
    let b = commit(&scratch, &repo, "fix: b (#2)", "Closes TM-1, NEW-1");
    // A comment naming a commit is not a scan's record.
    let note = tm(&scratch, &repo, &["comment", "NEW-1", &format!("see {a}")]);
    assert!(note.status.success(), "{note:?}");
    // tm reads the log as UTF-8 whatever git would print it in.
    let utf16 = ["config", "i18n.logOutputEncoding", "UTF-16"];
    git(&scratch, &repo, &utf16);

    let doc = scan(&scratch, &repo);
    assert_eq!(doc["data"]["moved"].as_array().unwrap().len(), 1);
    let comments = show(&scratch, &repo, "NEW-1")["comments"].clone();
    let record = format!("shipped in {a}, #1; also closed by {b}, #2");
    assert_eq!(comments[1]["body"], record, "each commit once");
    let reopen = tm(&scratch, &repo, &["move", "NEW-1", "todo"]);
    assert!(reopen.status.success(), "{reopen:?}");
    let doc = scan(&scratch, &repo);
    assert_eq!(doc["data"]["moved"], json!([]), "both Keys were recorded");
}

#[test]
fn a_scan_reads_the_remotes_default_branch_as_last_fetched() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    let (child, _) = store.create_issue(&jace, project, "task", "c").unwrap();
    let (epic, _) = store.create_issue(&jace, project, "epic", "e").unwrap();
    let parent = json!(epic.to_string());
    set(&mut store, project, child, "parent", parent);
    let remote = scratch.0.join("remote.git");
    let remote = remote.to_str().unwrap();
    let clone = ["clone", "--quiet", "--bare", ".", remote];
    git(&scratch, &repo, &clone);
    git(&scratch, &repo, &["remote", "add", "origin", remote]);
    git(&scratch, &repo, &["fetch", "--quiet", "origin"]);
    // A fetch may set origin/HEAD; without it, main's upstream is read.
    git(&scratch, &repo, &["remote", "set-head", "origin", "-d"]);
    let track = ["branch", "--set-upstream-to", "origin/main"];
    git(&scratch, &repo, &track);
    let doc = scan(&scratch, &repo);
    assert_eq!(doc["data"]["branch"], "origin/main", "main's upstream");
    assert_eq!(doc["warnings"], json!([]));
    git(&scratch, &repo, &["remote", "set-head", "origin", "main"]);
    // A tag of the branch's name must not shadow it.
    git(&scratch, &repo, &["tag", "origin/main"]);

    // No blank line: git's %s would join both lines into the subject.
    let shipped = commit(&scratch, &repo, "feat: c (#9)\nCloses TM-1", "");
    let doc = scan(&scratch, &repo);
    assert_eq!(doc["data"]["branch"], "origin/main");
    assert_eq!(doc["data"]["moved"], json!([]), "not fetched yet");

    git(&scratch, &repo, &["push", "--quiet", "origin", "main"]);
    git(&scratch, &repo, &["fetch", "--quiet", "origin"]);
    let doc = scan(&scratch, &repo);
    assert_eq!(doc["warnings"], json!([]));
    let moved = &doc["data"]["moved"][0];
    let batch = moved["batch"].as_str().unwrap();
    let expected = json!({
        "id": child.to_string(),
        "key": "TM-1",
        "from": "todo",
        "to": "done",
        "commit": shipped,
        "pr": 9,
        "batch": batch,
    });
    assert_eq!(*moved, expected);
    assert_eq!(doc["data"]["next"], json!(["tm move TM-2 done"]));

    let undone = tm(&scratch, &repo, &["undo", batch]);
    assert!(undone.status.success(), "{undone:?}");
    let child = show(&scratch, &repo, "TM-1");
    assert_eq!(child["fields"]["status"], "todo");
    assert_eq!(child["comments"][0]["retracted"], true, "one batch");
    let doc = scan(&scratch, &repo);
    assert_eq!(doc["data"]["moved"], json!([]), "an undo stays");

    git(&scratch, &repo, &["remote", "set-head", "origin", "-d"]);
    let tracked = "refs/remotes/origin/main";
    git(&scratch, &repo, &["update-ref", "-d", tracked]);
    let doc = scan(&scratch, &repo);
    assert_eq!(doc["data"]["branch"], "main");
    let gone = "since origin/main, the branch it tracks, is gone";
    let warned = doc["warnings"].to_string();
    assert!(warned.contains(gone), "{warned}");
}

#[test]
fn a_scan_names_the_branch_it_falls_back_to_or_fails() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    git(&scratch, &repo, &["branch", "master"]);
    let doc = scan(&scratch, &repo);
    assert_eq!(doc["data"]["branch"], "main", "main before master");
    git(&scratch, &repo, &["branch", "--set-upstream-to", "master"]);
    let warned = scan(&scratch, &repo)["warnings"].to_string();
    let local = "scanned local main, since it tracks no remote branch";
    assert!(warned.contains(local), "a local upstream: {warned}");

    // refs/heads/main/wip matches a for-each-ref pattern for main.
    git(&scratch, &repo, &["branch", "-m", "main", "main/wip"]);
    let doc = scan(&scratch, &repo);
    assert_eq!(doc["data"]["branch"], "master");

    git(&scratch, &repo, &["branch", "-D", "master"]);
    let out = tm(&scratch, &repo, &["--json", "git", "scan"]);
    assert!(!out.status.success());
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"], "no_branch", "{doc}");
    assert_eq!(doc["fix"], Value::Null, "no origin to set a head on");

    let elsewhere = scratch.0.join("elsewhere.git");
    let elsewhere = elsewhere.to_str().unwrap();
    git(&scratch, &repo, &["remote", "add", "origin", elsewhere]);
    let out = tm(&scratch, &repo, &["--json", "git", "scan"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["fix"], "git remote set-head origin --auto", "{doc}");
}
