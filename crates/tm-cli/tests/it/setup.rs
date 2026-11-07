use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

use crate::support::{Scratch, fixture, jace, text, tm, tm_command};

const SETTINGS: &str = ".claude/settings.json";
const LOCAL: &str = ".claude/settings.local.json";

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
fn each_hook_exits_quietly_where_tm_is_not_installed() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let made = tm(&scratch, &repo, &["setup", "claude", "--yes"]);
    assert!(made.status.success(), "{made:?}");
    let written = doc(&fs::read(repo.join(LOCAL)).unwrap());
    let commands: Vec<&str> = written["hooks"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|groups| groups.as_array().unwrap())
        .flat_map(|group| group["hooks"].as_array().unwrap())
        .map(|handler| handler["command"].as_str().unwrap())
        .collect();
    assert_eq!(commands.len(), 6);
    for command in commands {
        let ran = Command::new("/bin/sh")
            .args(["-c", command])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(ran.status.success(), "{command}: {ran:?}");
        assert!(
            ran.stdout.is_empty() && ran.stderr.is_empty(),
            "{command}: {ran:?}"
        );
    }
}

#[test]
fn yes_writes_the_hooks_once_and_undo_removes_the_file_it_made() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");

    // Git's default excludes file, where Claude Code ignores the local file.
    let made = jace(&scratch, &repo, &["setup", "claude", "--yes"])
        .env("XDG_CONFIG_HOME", &scratch.0)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    let shown = text(&made.stdout);
    assert!(
        shown.starts_with("--- /dev/null\n+++ b/.claude/settings.local.json\n"),
        "{shown}"
    );
    assert!(
        shown.ends_with(".claude/settings.local.json stays on this machine; it is not committed\n"),
        "{shown}"
    );
    assert!(
        text(&made.stderr).contains(
            "git does not ignore .claude/settings.local.json, so it could be committed; add it \
             to .gitignore"
        ),
        "{made:?}"
    );
    assert!(!repo.join(SETTINGS).exists());
    assert!(
        shown.contains(
            "Added tm's hooks to .claude/settings.local.json: tm prime when a session starts, \
             each sub-agent's tm commands written under its own name, a warning before an edit to \
             a file another session's Claim covers, a heartbeat after each tool use, and \
             releasing Claims when a sub-agent or the session ends."
        ),
        "the summary names every hook it writes: {shown}"
    );
    let written: Value =
        serde_json::from_str(&fs::read_to_string(repo.join(LOCAL)).unwrap()).unwrap();
    let events = [
        "SessionStart",
        "PreToolUse",
        "PostToolUse",
        "SubagentStop",
        "SessionEnd",
    ];
    let commands: Vec<&str> = events
        .iter()
        .map(|event| {
            written["hooks"][event][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
        })
        .collect();
    let guard = "command -v tm >/dev/null || exit 0; ";
    assert_eq!(
        commands,
        [
            "tm prime",
            "tm hook pre-bash claude || exit 1",
            "cd \"${CLAUDE_PROJECT_DIR:?}\" && tm hook heartbeat claude",
            "cd \"${CLAUDE_PROJECT_DIR:?}\" && tm hook subagent-stop claude || exit 1",
            "cd \"${CLAUDE_PROJECT_DIR:?}\" && tm hook session-end claude",
        ]
        .map(|command| format!("{guard}{command}"))
    );
    assert_eq!(written["hooks"]["PreToolUse"][0]["matcher"], "Bash");
    assert_eq!(written["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], 10);
    let edit = &written["hooks"]["PreToolUse"][1];
    assert_eq!(edit["matcher"], "Edit|MultiEdit|Write|NotebookEdit");
    assert_eq!(
        edit["hooks"][0]["command"],
        format!("{guard}cd \"${{CLAUDE_PROJECT_DIR:?}}\" && tm hook pre-edit claude || exit 1")
    );
    assert_eq!(written["hooks"]["PostToolUse"][0]["matcher"], "*");
    assert_eq!(
        written["hooks"]["PostToolUse"][0]["hooks"][0]["async"],
        true
    );
    assert_eq!(
        written["hooks"]["SubagentStop"][0]["hooks"][0]["timeout"],
        10
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
    assert_eq!(data["changed"].as_array().unwrap().len(), 6);
    assert_eq!(data["commit"], serde_json::json!([]));
    assert!(!repo.join(LOCAL).exists());

    let none = tm(&scratch, &repo, &["setup", "claude", "--undo", "--yes"]);
    assert!(none.status.success(), "{none:?}");
    assert_eq!(
        text(&none.stdout),
        ".claude/settings.local.json holds none of tm's hooks; nothing to remove\n"
    );
}

#[test]
fn shared_writes_the_file_everyone_runs_and_a_rerun_keeps_hooks_where_they_are() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");

    let made = tm(&scratch, &repo, &["setup", "claude", "--shared", "--yes"]);
    assert!(made.status.success(), "{made:?}");
    assert!(
        text(&made.stdout).ends_with("Commit .claude/settings.json\n"),
        "{made:?}"
    );
    assert!(!text(&made.stderr).contains("gitignore"), "{made:?}");
    assert!(repo.join(SETTINGS).exists() && !repo.join(LOCAL).exists());

    let again = tm(&scratch, &repo, &["setup", "claude", "--yes"]);
    assert_eq!(
        text(&again.stdout),
        "tm's hooks are already in .claude/settings.json\n",
        "{again:?}"
    );
    assert!(!repo.join(LOCAL).exists());

    // Hooks in both files run once in Claude Code; one undo takes both out.
    fs::copy(repo.join(SETTINGS), repo.join(LOCAL)).unwrap();
    let both = tm(&scratch, &repo, &["setup", "claude", "--shared", "--yes"]);
    assert!(
        text(&both.stderr).contains(".claude/settings.local.json holds tm's hooks too"),
        "{both:?}"
    );
    let undone = tm(
        &scratch,
        &repo,
        &["--json", "setup", "claude", "--undo", "--yes"],
    );
    assert!(undone.status.success(), "{undone:?}");
    let data = &doc(&undone.stdout)["data"];
    assert_eq!(
        (&data["file"], &data["also"]["file"]),
        (&serde_json::json!(SETTINGS), &serde_json::json!(LOCAL))
    );
    assert!(!repo.join(SETTINGS).exists() && !repo.join(LOCAL).exists());
}

#[test]
fn undo_across_both_files_shows_both_and_writes_neither_unless_both_can_be() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let made = tm(&scratch, &repo, &["setup", "claude", "--shared", "--yes"]);
    assert!(made.status.success(), "{made:?}");
    fs::copy(repo.join(SETTINGS), repo.join(LOCAL)).unwrap();

    let asked = tm(&scratch, &repo, &["--json", "setup", "claude", "--undo"]);
    assert!(!asked.status.success(), "{asked:?}");
    let refusal = doc(&asked.stdout);
    let diff = refusal["diff"].as_str().unwrap();
    assert!(
        diff.contains("--- a/.claude/settings.json")
            && diff.contains("--- a/.claude/settings.local.json"),
        "{refusal}"
    );
    assert!(
        refusal["message"]
            .as_str()
            .unwrap()
            .starts_with(".claude/settings.json and .claude/settings.local.json not written"),
        "{refusal}"
    );

    let mut perms = fs::metadata(repo.join(LOCAL)).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(repo.join(LOCAL), perms).unwrap();
    let refused = tm(&scratch, &repo, &["setup", "claude", "--undo", "--yes"]);
    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        repo.join(SETTINGS).exists() && repo.join(LOCAL).exists(),
        "{refused:?}"
    );
}

#[test]
fn git_s_own_complaints_stay_out_of_the_json_stream() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let exclude = repo.join(".git/info/exclude");
    fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    fs::write(&exclude, "").unwrap();
    fs::set_permissions(&exclude, fs::Permissions::from_mode(0o000)).unwrap();
    let out = tm(&scratch, &repo, &["--json", "setup", "claude", "--yes"]);
    fs::set_permissions(&exclude, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(text(&out.stderr), "", "{out:?}");
    doc(&out.stdout);
}

#[test]
fn hooks_in_the_local_file_are_found_there() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let made = tm(&scratch, &repo, &["setup", "claude", "--yes"]);
    assert!(made.status.success(), "{made:?}");
    let again = tm(&scratch, &repo, &["setup", "claude", "--yes"]);
    assert_eq!(
        text(&again.stdout),
        "tm's hooks are already in .claude/settings.local.json\n",
        "{again:?}"
    );
    let undone = tm(
        &scratch,
        &repo,
        &["setup", "claude", "--undo", "--shared", "--yes"],
    );
    assert!(undone.status.success(), "{undone:?}");
    assert!(!repo.join(LOCAL).exists() && !repo.join(SETTINGS).exists());
}

#[test]
fn a_settings_file_tm_cannot_read_is_named_not_passed_over() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    fs::create_dir_all(repo.join(".claude")).unwrap();
    fs::write(repo.join(SETTINGS), "{ not json").unwrap();
    for args in [
        &["--json", "setup", "claude", "--yes"][..],
        &["--json", "setup", "claude", "--undo", "--yes"],
    ] {
        let out = tm(&scratch, &repo, args);
        assert!(!out.status.success(), "{args:?}: {out:?}");
        let refusal = doc(&out.stdout);
        assert_eq!(refusal["error"], "setup_refused", "{args:?}");
        assert!(
            refusal["message"]
                .as_str()
                .unwrap()
                .starts_with(".claude/settings.json is not valid JSON"),
            "{refusal}"
        );
    }
    assert!(!repo.join(LOCAL).exists());
}

#[test]
fn a_local_file_git_ignores_is_not_warned_about() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    fs::write(repo.join(".gitignore"), ".claude/settings.local.json\n").unwrap();
    let made = jace(&scratch, &repo, &["setup", "claude", "--yes"])
        .env("XDG_CONFIG_HOME", &scratch.0)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    assert!(!text(&made.stderr).contains("gitignore"), "{made:?}");
}

#[test]
fn codex_has_one_hooks_file_so_shared_is_refused() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let out = tm(
        &scratch,
        &repo,
        &["--json", "setup", "codex", "--shared", "--yes"],
    );
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(doc(&out.stdout)["error"], "usage");
    assert!(!repo.join(".codex").exists());
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

    let made = tm(&scratch, &repo, &["setup", "claude", "--shared", "--yes"]);
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
            &["setup", "claude", "--shared", "--yes"][..],
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
            .contains("+            \"command\": \"command -v tm >/dev/null || exit 0; tm prime\""),
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
        text(&agent.stderr).contains(
            "a person runs tm setup claude from their own terminal to write it, since a \
             command run in the Agent's session, Claude Code's ! included, counts as the Agent"
        ),
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
            .contains("+            \"command\": \"command -v tm >/dev/null || exit 0; tm prime\""),
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
        stderr.contains("a person runs tm setup claude from their own terminal to write it")
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
        text(&undo.stdout).starts_with("--- a/.claude/settings.local.json\n"),
        "{undo:?}"
    );
    assert!(
        text(&undo.stderr)
            .contains("a person runs tm setup claude --undo from their own terminal to write it"),
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
            .contains("a person runs tm setup claude --undo from their own terminal to write it"),
        "{refusal}"
    );
    assert!(repo.join(LOCAL).exists());
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
        let out = tm(
            &scratch,
            &repo,
            &["--json", "setup", "claude", "--shared", "--yes"],
        );
        assert!(!out.status.success(), "{broken}: {out:?}");
        assert_eq!(doc(&out.stdout)["error"], "setup_refused", "{broken}");
        assert_eq!(fs::read_to_string(repo.join(SETTINGS)).unwrap(), broken);
    }

    fs::write(repo.join(SETTINGS), "{}\n").unwrap();
    fs::set_permissions(repo.join(SETTINGS), fs::Permissions::from_mode(0o444)).unwrap();
    let out = tm(
        &scratch,
        &repo,
        &["--json", "setup", "claude", "--shared", "--yes"],
    );
    assert_eq!(doc(&out.stdout)["error"], "setup_refused", "{out:?}");
    assert_eq!(fs::read_to_string(repo.join(SETTINGS)).unwrap(), "{}\n");
    fs::remove_file(repo.join(SETTINGS)).unwrap();

    // The rename that writes the file would replace a link with a copy, and
    // undo would delete the link.
    let elsewhere = scratch.0.join("shared-settings.json");
    for (text, args) in [
        (
            "{}\n",
            &["--json", "setup", "claude", "--shared", "--yes"][..],
        ),
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
