//! `tm guide` and the habit lines in help: how to work with tm, from the
//! binary itself.

use std::fs;
use std::path::Path;
use std::process::Output;

use serde_json::Value;
use tm_core::Actor;

use crate::support::{Scratch, initialized, text, tm, tm_command};

/// `tm` in a directory no repo or Workspace reaches.
fn elsewhere(scratch: &Scratch, args: &[&str]) -> String {
    let dir = scratch.home().join("elsewhere");
    fs::create_dir_all(&dir).unwrap();
    let out = tm_command()
        .args(args)
        .current_dir(&dir)
        .env("TM_HOME", scratch.home())
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    text(&out.stdout)
}

#[test]
fn the_guide_prints_anywhere_in_text_and_json() {
    let scratch = Scratch::new();
    let guide = elsewhere(&scratch, &["guide"]);
    assert!(guide.starts_with("How to work with tm\n"), "{guide}");
    for command in [
        "\nClaims\n",
        "  tm heartbeat ",
        "  tm claims ",
        "  tm release KEY ",
        "  tm hook print claude ",
    ] {
        assert!(guide.contains(command), "{command:?} in {guide}");
    }
    let doc: Value = serde_json::from_str(&elsewhere(&scratch, &["--json", "guide"])).unwrap();
    assert_eq!(doc["data"]["guide"].as_str(), Some(guide.as_str()));
    assert!(
        elsewhere(&scratch, &["--help"]).contains("\n  guide "),
        "tm --help lists it"
    );
}

#[test]
fn comment_and_edit_help_carry_their_habit() {
    let scratch = Scratch::new();
    let comment = elsewhere(&scratch, &["comment", "--help"]);
    assert!(
        comment.contains("Comments keep a dated trail an edit would overwrite"),
        "{comment}"
    );
    let edit = elsewhere(&scratch, &["edit", "--help"]);
    assert!(
        edit.contains("The description holds the current plan"),
        "{edit}"
    );
}

/// `tm --json` as the Agent `claude` writing for `jace`, with `input` on
/// stdin.
fn as_claude(scratch: &Scratch, repo: &Path, args: &[&str], input: &str) -> Output {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut child = tm_command()
        .arg("--json")
        .args(args)
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "claude")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// Every command the guide prints runs as printed, in order, with KEY,
/// OTHER, TEXT and BATCH filled in: an Agent runs all but the bare
/// `tm undo`, which the guide gives to a human.
#[test]
fn every_command_in_the_guide_runs_as_printed() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    for title in ["the work", "after it"] {
        store.create_issue(&jace, project, "task", title).unwrap();
    }
    drop(store);

    let guide = elsewhere(&scratch, &["guide"]);
    let mut batch = String::new();
    let mut ran = Vec::new();
    for line in guide.lines().map(str::trim_start) {
        let Some(command) = line.strip_prefix("tm ") else {
            continue;
        };
        let words: Vec<String> = command
            .split("  ")
            .next()
            .unwrap()
            .split_whitespace()
            .map(|word| match word {
                "KEY" => "TM-1".to_owned(),
                "OTHER" => "TM-2".to_owned(),
                "TEXT" => "found".to_owned(),
                "BATCH" => batch.clone(),
                word => word.replace("KEY", "TM-1"),
            })
            .collect();
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let out = if words == ["undo"] {
            tm(&scratch, &repo, &["--json", "undo"])
        } else {
            as_claude(&scratch, &repo, &words, "some text\n")
        };
        assert!(out.status.success(), "tm {}: {out:?}", words.join(" "));
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        if let Some(written) = doc["data"]["batch"].as_str() {
            batch = written.to_owned();
        }
        ran.push(words.join(" "));
    }
    assert!(ran.iter().any(|run| run.starts_with("undo ")), "{ran:?}");
    assert!(ran.len() >= 12, "{ran:?}");
}
