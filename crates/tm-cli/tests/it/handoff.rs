//! `tm handoff`: a Claim handed to another Agent with a note, shown to that
//! Agent's sessions by `tm prime` and taken by the first to `tm start` it.

use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::Value;

use crate::support::{Scratch, initialized, text, tm, tm_command};

/// `words` run in `repo` as `agent` in `session`, or as the human with no
/// Agent.
fn run(scratch: &Scratch, repo: &Path, who: Option<(&str, &str)>, words: &str) -> Output {
    let words: Vec<&str> = words.split(' ').collect();
    let Some((agent, session)) = who else {
        return tm(scratch, repo, &words);
    };
    tm_command()
        .args(words)
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", agent)
        .env("TM_SESSION", session)
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

fn doc(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

fn ok(out: Output) -> Output {
    assert!(out.status.success(), "{out:?}");
    out
}

const S1: Option<(&str, &str)> = Some(("claude", "s1"));
const X1: Option<(&str, &str)> = Some(("codex", "x1"));
const HUMAN: Option<(&str, &str)> = None;

/// TM-1, which claude's session s1 has started.
fn started(scratch: &Scratch) -> PathBuf {
    let (repo, _, _) = initialized(scratch);
    ok(run(scratch, &repo, HUMAN, "create --type task --title t"));
    ok(run(scratch, &repo, S1, "start TM-1"));
    repo
}

#[test]
fn the_holder_hands_off_and_the_receivers_first_session_takes_it() {
    let scratch = Scratch::new();
    let repo = started(&scratch);
    let words = "--json handoff TM-1 --to codex parser-half-done";
    let handed = doc(&ok(run(&scratch, &repo, S1, words)));
    assert_eq!(handed["data"]["claim"]["agent"], "codex", "{handed}");
    assert_eq!(handed["data"]["claim"]["handed"], true, "{handed}");
    assert_eq!(handed["data"]["note"], "parser-half-done");
    assert!(handed["data"]["batch"].is_string(), "one undoable batch");
    let warned = handed["warnings"][0].as_str().unwrap();
    assert!(warned.starts_with("codex has written nothing"), "{handed}");

    let primed = text(&ok(run(&scratch, &repo, X1, "prime")).stdout);
    let section = "Handed to you (tm start KEY takes one):\n  TM-1  in_progress";
    assert!(primed.contains(section), "{primed}");
    let from = "(from claude, expires in 2h)\n    parser-half-done\n";
    assert!(primed.contains(from), "{primed}");
    let primed = doc(&ok(run(&scratch, &repo, X1, "--json prime")));
    let handed = &primed["data"]["handed_claims"][0];
    assert_eq!(handed["note"], "parser-half-done", "{primed}");
    let none = Value::Array(Vec::new());
    assert_eq!(primed["data"]["claims"], none, "not its own");
    let shown = text(&ok(run(&scratch, &repo, HUMAN, "show TM-1")).stdout);
    let line = "handed to codex for jace, expires in 2h; its next session";
    assert!(shown.contains(line), "{shown}");

    let giver = run(&scratch, &repo, S1, "--json edit TM-1 --set description=x");
    assert_eq!(doc(&giver)["error"], "already_claimed", "the giver let go");
    let early = run(&scratch, &repo, X1, "--json edit TM-1 --set description=x");
    let early = doc(&early)["message"].as_str().unwrap().to_owned();
    let hint = "codex takes it with tm start TM-1";
    assert!(early.contains(hint), "{early}");
    ok(run(&scratch, &repo, X1, "start TM-1"));
    let late = run(&scratch, &repo, Some(("codex", "x2")), "--json start TM-1");
    assert_eq!(doc(&late)["error"], "already_claimed");
    ok(run(&scratch, &repo, X1, "edit TM-1 --set description=mine"));
}

#[test]
fn another_session_needs_force_and_the_human_does_not() {
    let scratch = Scratch::new();
    let repo = started(&scratch);
    let s2 = Some(("claude", "s2"));
    // Stdin is empty: read first, the note would be refused as blank instead.
    let piped = "--json handoff TM-1 --to codex -";
    let err = doc(&run(&scratch, &repo, s2, piped));
    assert_eq!(err["error"], "already_claimed", "{err}");
    let message = err["message"].as_str().unwrap();
    assert!(message.ends_with("which tm handoff --force does"), "{err}");
    assert_eq!(err["fix"], Value::Null);
    let forced = "handoff TM-1 --to codex yours --force";
    ok(run(&scratch, &repo, s2, forced));

    let blank = doc(&run(&scratch, &repo, HUMAN, "--json handoff TM-1 --to= -"));
    assert_eq!(blank["field"], "to", "refused before stdin: {blank}");
    let rescued = "--json handoff TM-1 --to claude crashed";
    let rescued = doc(&ok(run(&scratch, &repo, HUMAN, rescued)));
    let none = Value::Array(Vec::new());
    assert_eq!(rescued["warnings"], none, "claude wrote");
    ok(run(&scratch, &repo, Some(("claude", "s3")), "start TM-1"));
}

#[test]
fn a_lapsed_claim_is_handed_without_force_to_a_trimmed_name() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    ok(run(&scratch, &repo, HUMAN, "create --type task --title t"));
    ok(run(&scratch, &repo, S1, "claim TM-1 --for 1s"));
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    // Another human's lapsed Claim would stay held for that human.
    let for_kim = tm_command()
        .args(["--json", "handoff", "TM-1", "--to", "codex", "n"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "kim")
        .env("TM_AGENT", "codex")
        .env("TM_SESSION", "k1")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert_eq!(doc(&for_kim)["error"], "already_claimed", "{for_kim:?}");
    let args = ["--json", "handoff", "TM-1", "--to", " codex ", "lapsed"];
    let out = tm_command()
        .args(args)
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "claude")
        .env("TM_SESSION", "s2")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    let handed = doc(&ok(out));
    assert_eq!(handed["data"]["claim"]["agent"], "codex", "{handed}");
    ok(run(&scratch, &repo, X1, "start TM-1"));
}

#[test]
fn an_unclaimed_issue_has_nothing_to_hand_off() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    ok(run(&scratch, &repo, HUMAN, "create --type task --title t"));
    let out = run(&scratch, &repo, HUMAN, "--json handoff TM-1 --to codex n");
    assert_eq!(doc(&out)["error"], "not_claim_holder", "{out:?}");
}
