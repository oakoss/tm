//! `tm notifications` lists the mentions of you and marks them read
//! (ADR 0019).

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

use crate::support::{Scratch, initialized, jace, text, tm};

/// `tm` writing as `actor`, an Agent for jace when `agent` is given.
fn as_actor(
    scratch: &Scratch,
    repo: &Path,
    actor: &str,
    agent: Option<&str>,
    args: &[&str],
) -> Output {
    let mut command: Command = jace(scratch, repo, args);
    command.env("TM_ACTOR", actor).stdin(Stdio::null());
    if let Some(agent) = agent {
        command.env("TM_AGENT", agent);
    }
    command.output().unwrap()
}

fn kim(scratch: &Scratch, repo: &Path, args: &[&str]) -> Output {
    as_actor(scratch, repo, "kim@example.com", None, args)
}

fn doc(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|err| panic!("{err}: {out:?}"))
}

/// TM-1, written by kim so a mention can name her, then mentioning her in a
/// comment by jace.
fn mentioned(scratch: &Scratch) -> PathBuf {
    let (repo, store, _) = initialized(scratch);
    drop(store);
    let made = kim(
        scratch,
        &repo,
        &["create", "--type", "task", "--title", "t"],
    );
    assert!(made.status.success(), "{made:?}");
    let said = tm(
        scratch,
        &repo,
        &["comment", "TM-1", "over to you\n@kim@example.com, see this"],
    );
    assert!(said.status.success(), "{said:?}");
    repo
}

fn listed(scratch: &Scratch, repo: &Path, args: &[&str]) -> Vec<Value> {
    let mut all = vec!["--json", "notifications"];
    all.extend_from_slice(args);
    doc(&kim(scratch, repo, &all))["data"]["notifications"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn a_mention_is_listed_for_whom_it_names_on_the_line_it_sits() {
    let scratch = Scratch::new();
    let repo = mentioned(&scratch);
    let out = kim(&scratch, &repo, &["notifications"]);
    let shown = text(&out.stdout);
    assert!(out.status.success(), "{out:?}");
    assert!(shown.contains("TM-1"), "{shown}");
    assert!(shown.contains("jace in a comment"), "{shown}");
    assert!(shown.contains(": @kim@example.com, see this"), "{shown}");
    assert!(
        !shown.contains("over to you"),
        "only the line it sits on: {shown}"
    );
    let entries = listed(&scratch, &repo, &[]);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["issue"], "TM-1");
    assert_eq!(entries[0]["kind"], "comment");
    assert_eq!(entries[0]["read"], false);
    assert_eq!(entries[0]["mentions"][0]["token"], "<@kim@example.com>");
    let mine = text(&tm(&scratch, &repo, &["notifications"]).stdout);
    assert_eq!(
        mine, "no unread Notifications\n",
        "the writer is not notified"
    );
}

#[test]
fn reading_one_hides_it_until_undone() {
    let scratch = Scratch::new();
    let repo = mentioned(&scratch);
    let id = listed(&scratch, &repo, &[])[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let end = &id[id.len() - 12..];
    let refused = tm(&scratch, &repo, &["--json", "notifications", "read", end]);
    assert!(!refused.status.success(), "jace cannot read kim's");
    assert_eq!(doc(&refused)["error"], "not_found");
    let read = kim(&scratch, &repo, &["notifications", "read", end]);
    assert!(read.status.success(), "{read:?}");
    assert!(
        text(&read.stdout).contains("marked 1 Notification read"),
        "{read:?}"
    );
    assert_eq!(listed(&scratch, &repo, &[]).len(), 0);
    assert_eq!(listed(&scratch, &repo, &["--all"])[0]["read"], true);
    let again = kim(&scratch, &repo, &["notifications", "read", "all"]);
    assert_eq!(text(&again.stdout), "no unread Notifications\n");
    let undone = kim(&scratch, &repo, &["undo"]);
    assert!(undone.status.success(), "{undone:?}");
    assert_eq!(listed(&scratch, &repo, &[]).len(), 1, "unread again");
}

#[test]
fn an_agent_reads_the_mentions_of_it() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    let made = as_actor(
        &scratch,
        &repo,
        "jace",
        Some("claude"),
        &["create", "--type", "task", "--title", "t"],
    );
    assert!(made.status.success(), "{made:?}");
    let said = tm(&scratch, &repo, &["comment", "TM-1", "@claude, take this"]);
    assert!(said.status.success(), "{said:?}");
    let out = as_actor(
        &scratch,
        &repo,
        "jace",
        Some("claude"),
        &["--json", "notifications"],
    );
    let entries = doc(&out)["data"]["notifications"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(entries.len(), 1, "{out:?}");
    assert_eq!(entries[0]["text"], "<@claude for jace>, take this");
    let read = as_actor(
        &scratch,
        &repo,
        "jace",
        Some("claude"),
        &["notifications", "read", "all"],
    );
    assert!(
        text(&read.stdout).contains("marked 1 Notification read"),
        "{read:?}"
    );
}

#[test]
fn a_write_past_an_agents_claim_tells_the_agent_and_shows_in_history() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    let claude = |args: &[&str]| as_actor(&scratch, &repo, "jace", Some("claude"), args);
    let made = claude(&["create", "--type", "task", "--title", "t"]);
    assert!(made.status.success(), "{made:?}");
    let started = claude(&["start", "TM-1"]);
    assert!(started.status.success(), "{started:?}");
    let moved = tm(&scratch, &repo, &["move", "TM-1", "in_review"]);
    assert!(moved.status.success(), "{moved:?}");
    let listed = text(&claude(&["notifications"]).stdout);
    assert!(
        listed.contains("TM-1  jace in your Claim")
            && listed.contains(": wrote status while you held it"),
        "{listed}"
    );
    let history = text(&tm(&scratch, &repo, &["history", "TM-1"]).stdout);
    assert!(
        history.contains("wrote status past claude's Claim"),
        "{history}"
    );
}

#[test]
fn each_line_says_where_the_mention_was_written() {
    let scratch = Scratch::new();
    let repo = mentioned(&scratch);
    for args in [
        &["ask", "TM-1", "@kim@example.com which?", "--of", "kim"][..],
        &["answer", "TM-1", "this, @kim@example.com"],
        &["comment", "TM-1", "a note"],
        &[
            "comment",
            "TM-1",
            "--edit",
            "last",
            "a note for @kim@example.com",
        ],
        &[
            "edit",
            "TM-1",
            "--set",
            "description=over to @kim@example.com",
        ],
    ] {
        let out = tm(&scratch, &repo, args);
        assert!(out.status.success(), "{args:?}: {out:?}");
    }
    let shown = text(&kim(&scratch, &repo, &["notifications"]).stdout);
    for (site, said) in [
        ("jace in a comment", ", see this"),
        ("jace in a question", " which?"),
        ("jace in an answer", "this, @"),
        ("jace in an edited comment", "a note for @"),
        ("jace in the description", "over to @"),
    ] {
        let line = shown.lines().find(|line| line.contains(said));
        assert!(
            line.is_some_and(|line| line.contains(site)),
            "{site}: {shown}"
        );
    }
}
