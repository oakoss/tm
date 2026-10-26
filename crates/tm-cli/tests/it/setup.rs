use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

use crate::support::{Scratch, fixture, text, tm, tm_command};

const SETTINGS: &str = ".claude/settings.json";

fn doc(stdout: &[u8]) -> Value {
    serde_json::from_slice(stdout).unwrap()
}

fn agent_command(scratch: &Scratch, repo: &Path, args: &[&str]) -> Command {
    let mut command = tm_command();
    command
        .args(args)
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_AGENT", "claude")
        .env("TM_ACTOR", "jace");
    command
}

fn as_agent(scratch: &Scratch, repo: &Path, args: &[&str]) -> Output {
    agent_command(scratch, repo, args).output().unwrap()
}

#[test]
fn yes_writes_the_hooks_once_and_undo_removes_the_file_it_made() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");

    let made = tm(&scratch, &repo, &["setup", "claude", "--yes"]);
    assert!(made.status.success(), "{made:?}");
    let shown = text(&made.stdout);
    assert!(
        shown.starts_with("--- /dev/null\n+++ b/.claude/settings.json\n"),
        "{shown}"
    );
    assert!(shown.ends_with("Commit .claude/settings.json\n"), "{shown}");
    let written: Value =
        serde_json::from_str(&fs::read_to_string(repo.join(SETTINGS)).unwrap()).unwrap();
    let commands: Vec<&str> = ["SessionStart", "PostToolUse", "SessionEnd"]
        .iter()
        .map(|event| {
            written["hooks"][event][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(
        commands,
        [
            "tm prime",
            "cd \"${CLAUDE_PROJECT_DIR:?}\" && tm hook heartbeat claude",
            "cd \"${CLAUDE_PROJECT_DIR:?}\" && tm release --mine",
        ]
    );
    assert_eq!(written["hooks"]["PostToolUse"][0]["matcher"], "*");
    assert_eq!(
        written["hooks"]["PostToolUse"][0]["hooks"][0]["async"],
        true
    );
    assert_eq!(written["hooks"]["SessionEnd"][0]["hooks"][0]["timeout"], 10);

    let again = tm(&scratch, &repo, &["--json", "setup", "claude", "--yes"]);
    assert!(again.status.success(), "{again:?}");
    assert_eq!(doc(&again.stdout)["data"]["changed"], serde_json::json!([]));

    let undone = tm(
        &scratch,
        &repo,
        &["--json", "setup", "claude", "--undo", "--yes"],
    );
    assert!(undone.status.success(), "{undone:?}");
    let data = &doc(&undone.stdout)["data"];
    assert_eq!(data["deleted"], true);
    assert_eq!(data["changed"].as_array().unwrap().len(), 3);
    assert!(!repo.join(SETTINGS).exists());

    let none = tm(&scratch, &repo, &["setup", "claude", "--undo", "--yes"]);
    assert!(none.status.success(), "{none:?}");
    assert_eq!(
        text(&none.stdout),
        ".claude/settings.json holds none of tm's hooks; nothing to remove\n"
    );
}

#[test]
fn the_owners_settings_survive_setup_and_undo_byte_for_byte() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    fs::create_dir_all(repo.join(".claude")).unwrap();
    let owners = r#"{
  "permissions": {
    "allow": [
      "Bash(cargo test:*)"
    ]
  },
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit",
        "hooks": [
          {
            "type": "command",
            "command": "cargo fmt"
          }
        ]
      }
    ]
  },
  "env": {
    "A": "1"
  }
}
"#;
    fs::write(repo.join(SETTINGS), owners).unwrap();

    let made = tm(&scratch, &repo, &["setup", "claude", "--yes"]);
    assert!(made.status.success(), "{made:?}");
    let merged = fs::read_to_string(repo.join(SETTINGS)).unwrap();
    assert!(
        merged
            .starts_with("{\n  \"permissions\": {\n    \"allow\": [\n      \"Bash(cargo test:*)\""),
        "{merged}"
    );
    assert!(
        merged.ends_with("  \"env\": {\n    \"A\": \"1\"\n  }\n}\n"),
        "{merged}"
    );
    let shown = text(&made.stdout);
    assert!(
        !shown.contains("-  \"permissions\""),
        "only tm's lines change: {shown}"
    );

    let undone = tm(&scratch, &repo, &["setup", "claude", "--undo", "--yes"]);
    assert!(undone.status.success(), "{undone:?}");
    assert_eq!(fs::read_to_string(repo.join(SETTINGS)).unwrap(), owners);
    assert!(text(&undone.stdout).ends_with(
        "Removed tm's hooks from .claude/settings.json\nCommit .claude/settings.json\n"
    ));

    for variant in [owners.trim_end().to_owned(), owners.replace('\n', "\r\n")] {
        fs::write(repo.join(SETTINGS), &variant).unwrap();
        for args in [
            &["setup", "claude", "--yes"][..],
            &["setup", "claude", "--undo", "--yes"],
        ] {
            let out = tm(&scratch, &repo, args);
            assert!(out.status.success(), "{args:?}: {out:?}");
        }
        assert_eq!(fs::read_to_string(repo.join(SETTINGS)).unwrap(), variant);
    }
}

#[test]
fn nothing_is_written_without_a_yes() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");

    let json = tm(&scratch, &repo, &["--json", "setup", "claude"]);
    assert!(!json.status.success(), "{json:?}");
    let refusal = doc(&json.stdout);
    assert_eq!(refusal["error"], "setup_refused");
    assert!(
        refusal["diff"]
            .as_str()
            .unwrap()
            .contains("+            \"command\": \"tm prime\""),
        "{refusal}"
    );

    // A stdout that takes no writes cannot show the diff a --yes confirms.
    let unwritable = scratch.0.join("read-only-stdout");
    fs::write(&unwritable, "").unwrap();
    let blind = tm_command()
        .args(["setup", "claude", "--yes"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .stdout(fs::File::open(&unwritable).unwrap())
        .output()
        .unwrap();
    assert!(!blind.status.success(), "{blind:?}");
    assert!(
        text(&blind.stderr).contains("cannot write output"),
        "{blind:?}"
    );
    assert!(!repo.join(".claude").exists());
}

#[test]
fn an_agent_is_only_shown_the_diff_even_with_yes() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");

    let agent = as_agent(&scratch, &repo, &["setup", "claude"]);
    assert!(!agent.status.success(), "{agent:?}");
    assert!(
        text(&agent.stdout).starts_with("--- /dev/null\n"),
        "the diff is shown"
    );
    assert!(
        text(&agent.stderr).contains("a person runs tm setup claude to write it"),
        "{agent:?}"
    );
    assert!(!repo.join(".claude").exists());

    // An Agent's --yes is not a person's, in JSON or not.
    let yes = as_agent(&scratch, &repo, &["setup", "claude", "--yes"]);
    assert!(!yes.status.success(), "{yes:?}");
    assert!(text(&yes.stdout).starts_with("--- /dev/null\n"), "{yes:?}");
    let json = as_agent(&scratch, &repo, &["--json", "setup", "claude", "--yes"]);
    assert!(!json.status.success(), "{json:?}");
    let refusal = doc(&json.stdout);
    assert_eq!(refusal["error"], "setup_refused");
    assert!(
        refusal["diff"]
            .as_str()
            .unwrap()
            .contains("+            \"command\": \"tm prime\""),
        "{refusal}"
    );
    let harness = agent_command(&scratch, &repo, &["setup", "claude", "--yes"])
        .env_remove("TM_AGENT")
        .env("CLAUDE_CODE_SESSION_ID", "s1")
        .output()
        .unwrap();
    assert!(!harness.status.success(), "{harness:?}");
    assert!(!repo.join(".claude").exists());

    // An Agent is refused even when the diff cannot be shown to it.
    let unwritable = scratch.0.join("read-only-stdout");
    fs::write(&unwritable, "").unwrap();
    let unshown = agent_command(&scratch, &repo, &["setup", "claude", "--yes"])
        .stdout(fs::File::open(&unwritable).unwrap())
        .output()
        .unwrap();
    assert!(!unshown.status.success(), "{unshown:?}");
    let stderr = text(&unshown.stderr);
    assert!(
        stderr.contains("a person runs tm setup claude to write it")
            && stderr.contains("the diff was not shown: cannot write output"),
        "{unshown:?}"
    );
    assert!(!repo.join(".claude").exists());

    // Nor can an Agent take them out.
    let made = tm(&scratch, &repo, &["setup", "claude", "--yes"]);
    assert!(made.status.success(), "{made:?}");
    let undo = as_agent(&scratch, &repo, &["setup", "claude", "--undo", "--yes"]);
    assert!(!undo.status.success(), "{undo:?}");
    assert!(
        text(&undo.stdout).starts_with("--- a/.claude/settings.json\n"),
        "{undo:?}"
    );
    assert!(
        text(&undo.stderr).contains("a person runs tm setup claude --undo to write it"),
        "{undo:?}"
    );
    let json = as_agent(
        &scratch,
        &repo,
        &["--json", "setup", "claude", "--undo", "--yes"],
    );
    assert!(!json.status.success(), "{json:?}");
    let refusal = doc(&json.stdout);
    assert_eq!(refusal["error"], "setup_refused");
    assert!(
        refusal["message"]
            .as_str()
            .unwrap()
            .contains("a person runs tm setup claude --undo to write it"),
        "{refusal}"
    );
    assert!(repo.join(SETTINGS).exists());
}

#[test]
fn a_file_tm_cannot_merge_into_is_left_alone() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    fs::create_dir_all(repo.join(".claude")).unwrap();
    for broken in [
        "{ not json",
        "[]",
        "{\"hooks\": []}",
        "{\"hooks\": {}, \"hooks\": {\"Stop\": []}}",
    ] {
        fs::write(repo.join(SETTINGS), broken).unwrap();
        let out = tm(&scratch, &repo, &["--json", "setup", "claude", "--yes"]);
        assert!(!out.status.success(), "{broken}: {out:?}");
        assert_eq!(doc(&out.stdout)["error"], "setup_refused", "{broken}");
        assert_eq!(fs::read_to_string(repo.join(SETTINGS)).unwrap(), broken);
    }

    fs::write(repo.join(SETTINGS), "{}\n").unwrap();
    fs::set_permissions(repo.join(SETTINGS), fs::Permissions::from_mode(0o444)).unwrap();
    let out = tm(&scratch, &repo, &["--json", "setup", "claude", "--yes"]);
    assert_eq!(doc(&out.stdout)["error"], "setup_refused", "{out:?}");
    assert_eq!(fs::read_to_string(repo.join(SETTINGS)).unwrap(), "{}\n");
    fs::remove_file(repo.join(SETTINGS)).unwrap();

    // The rename that writes the file would replace a link with a copy, and
    // undo would delete the link.
    let elsewhere = scratch.0.join("shared-settings.json");
    for (text, args) in [
        ("{}\n", &["--json", "setup", "claude", "--yes"][..]),
        (
            "{\"hooks\": {\"SessionStart\": [{\"hooks\": [{\"command\": \"tm prime\"}]}]}}\n",
            &["--json", "setup", "claude", "--undo", "--yes"],
        ),
    ] {
        fs::write(&elsewhere, text).unwrap();
        std::os::unix::fs::symlink(&elsewhere, repo.join(SETTINGS)).unwrap();
        let out = tm(&scratch, &repo, args);
        assert_eq!(
            doc(&out.stdout)["error"],
            "setup_refused",
            "{args:?}: {out:?}"
        );
        let link = fs::symlink_metadata(repo.join(SETTINGS)).unwrap();
        assert!(link.file_type().is_symlink());
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), text);
        fs::remove_file(repo.join(SETTINGS)).unwrap();
    }

    let outside = scratch.0.join("plain");
    fs::create_dir_all(&outside).unwrap();
    let out = tm(&scratch, &outside, &["--json", "setup", "claude", "--yes"]);
    assert_eq!(doc(&out.stdout)["error"], "setup_refused", "{out:?}");
}
