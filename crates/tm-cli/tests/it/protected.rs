//! Protected fields: while one Agent session holds a live Claim, another
//! session's write to the Issue's Status or description is refused unless it
//! says `--force`, and a human's goes through with a warning (ADR 0009).

use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};

use crate::support::{Scratch, initialized, text, tm, tm_command};

const HELD: &str = "TM-3 is claimed by claude (session s1) for jace, who may be working on it";

fn as_agent(scratch: &Scratch, repo: &Path, session: &str, words: &str) -> Output {
    tm_command()
        .args(words.split(' '))
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

fn as_human(scratch: &Scratch, repo: &Path, words: &str) -> Output {
    tm(scratch, repo, &words.split(' ').collect::<Vec<_>>())
}

fn doc(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

fn ok(out: Output) -> Output {
    assert!(out.status.success(), "{out:?}");
    out
}

/// TM-1 an epic session s1 holds, TM-2 its todo child, and TM-3 a task s1
/// has started.
fn workspace(scratch: &Scratch) -> PathBuf {
    let (repo, _, _) = initialized(scratch);
    ok(as_human(scratch, &repo, "create --type epic --title e"));
    let child = "create --type task --title c --parent TM-1";
    ok(as_human(scratch, &repo, child));
    ok(as_human(scratch, &repo, "create --type task --title t"));
    ok(as_agent(scratch, &repo, "s1", "start TM-3"));
    ok(as_agent(scratch, &repo, "s1", "claim TM-1"));
    repo
}

#[test]
fn another_session_is_refused_the_status_and_description_naming_force() {
    let scratch = Scratch::new();
    let repo = workspace(&scratch);
    for (words, field, force) in [
        ("edit TM-3 --set description=x", "description", "tm edit"),
        ("move TM-3 in_review", "status", "tm move"),
    ] {
        let out = as_agent(&scratch, &repo, "s2", &format!("--json {words}"));
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        let err = doc(&out);
        assert_eq!(err["error"], "already_claimed", "{err}");
        assert_eq!(err["field"], field, "{err}");
        assert_eq!(err["session"], "s1", "{err}");
        let message = err["message"].as_str().unwrap();
        assert!(
            message.ends_with(&format!("; {force} --force writes it anyway")),
            "{err}"
        );
        assert_eq!(err["fix"], Value::Null, "never a fix to run by rote");
    }
    let open = "edit TM-3 --set priority=low";
    ok(as_agent(&scratch, &repo, "s2", open));
    let own = "edit TM-3 --set description=mine";
    ok(as_agent(&scratch, &repo, "s1", own));
}

#[test]
fn force_and_a_human_write_past_the_claim_with_a_warning() {
    let scratch = Scratch::new();
    let repo = workspace(&scratch);
    let words = "--json edit TM-3 --set description=x --force";
    let out = ok(as_agent(&scratch, &repo, "s2", words));
    assert_eq!(doc(&out)["warnings"], json!([HELD]));

    let words = "edit TM-3 --set description=y --force";
    let shown = text(&ok(as_agent(&scratch, &repo, "s2", words)).stdout);
    assert!(
        !shown.contains("undo this with"),
        "an undo the Claim refuses is not offered: {shown}"
    );

    let out = ok(as_human(&scratch, &repo, "move TM-3 in_review"));
    assert_eq!(text(&out.stderr), format!("tm: warning: {HELD}\n"));
    let out = ok(as_human(&scratch, &repo, "start TM-3"));
    assert_eq!(
        text(&out.stderr),
        format!("tm: warning: {HELD}\n"),
        "a start that writes no protected field still warns, once"
    );
}

#[test]
fn a_refusal_comes_before_piped_text_is_read() {
    let scratch = Scratch::new();
    let repo = workspace(&scratch);
    for words in [
        "--json edit TM-3 --set description=-",
        "--json move TM-3 in_review --set description=-",
        "--json move TM-3 done --set description=-",
        "--json edit TM-3 --set description=x --set title=-",
    ] {
        // Stdin is empty: read first, it would be refused as blank instead.
        let out = as_agent(&scratch, &repo, "s2", words);
        assert_eq!(doc(&out)["error"], "already_claimed", "{words}: {out:?}");
    }
    // Staying in its Status writes no protected field, so stdin is read.
    let words = "--json move TM-3 in_progress --set title=-";
    let out = as_agent(&scratch, &repo, "s2", words);
    assert_eq!(doc(&out)["error"], "invalid_value", "{out:?}");
}

#[test]
fn an_undo_is_refused_a_protected_field_until_forced() {
    let scratch = Scratch::new();
    let repo = workspace(&scratch);
    let words = "--json edit TM-3 --set description=x";
    let out = ok(as_agent(&scratch, &repo, "s1", words));
    let batch = doc(&out)["data"]["batch"].as_str().unwrap().to_owned();
    let out = as_agent(&scratch, &repo, "s2", &format!("--json undo {batch}"));
    assert_eq!(doc(&out)["error"], "already_claimed", "{out:?}");
    let words = format!("--json undo {batch} --force");
    let out = ok(as_agent(&scratch, &repo, "s2", &words));
    assert_eq!(doc(&out)["warnings"], json!([HELD]));
}

#[test]
fn starting_a_child_leaves_a_parent_another_session_holds() {
    let scratch = Scratch::new();
    let repo = workspace(&scratch);
    let started = doc(&ok(as_agent(&scratch, &repo, "s2", "--json start TM-2")));
    assert_eq!(started["data"]["started"], json!([]), "{started}");
    assert_eq!(
        started["warnings"],
        json!(["TM-1 was not started: claude (session s1) for jace holds it"])
    );
    let parent = doc(&as_human(&scratch, &repo, "--json show TM-1"));
    assert_eq!(parent["data"]["fields"]["status"], "todo");
}
