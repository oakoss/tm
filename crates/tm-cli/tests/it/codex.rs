//! Codex: `tm setup codex`, and `tm hook` reading the session from the hook's
//! stdin, since Codex gives its hooks neither the Agent's variables nor its
//! session.

use std::fs;
use std::path::Path;
use std::process::Output;

use serde_json::{Value, json};
use tm_core::Actor;

use crate::support::{
    Scratch, fixture, initialized, piped, text, tm, tm_command, tm_piped, tm_pty, wait_until,
};

const SESSION: &str = "01a17b87-989b-72a0-a41b-abe9aef24379";

/// `tm` from the Agent's own shell, where Codex sets `CODEX_SESSION_ID`.
fn from_codex_shell(scratch: &Scratch, repo: &Path, args: &[&str]) -> Output {
    tm_command()
        .args(args)
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("CODEX_SESSION_ID", SESSION)
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

/// A Codex hook: no Agent variables, the session only in its stdin JSON.
fn hook(scratch: &Scratch, dir: &Path, event: &str, session: &str) -> Output {
    let input = json!({ "session_id": session, "cwd": dir, "hook_event_name": event });
    tm_piped(scratch, dir, &["hook", event, "codex"], input.to_string())
}

fn claim_of(scratch: &Scratch, repo: &Path) -> Value {
    let out = tm(scratch, repo, &["--json", "show", "TM-1"]);
    serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"]["claim"].clone()
}

#[test]
fn codex_hooks_act_for_the_session_their_stdin_names() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    let taken = from_codex_shell(&scratch, &repo, &["--json", "claim", "TM-1", "--for", "3s"]);
    assert!(taken.status.success(), "{taken:?}");
    let taken: Value = serde_json::from_slice(&taken.stdout).unwrap();
    let expires_ms = taken["data"]["claim"]["expires_ms"].as_i64().unwrap();
    // Codex runs hooks in the session's directory, which may be below the root.
    let below = repo.join("src");
    fs::create_dir_all(&below).unwrap();

    let started = hook(&scratch, &below, "session-start", SESSION);
    assert!(started.status.success(), "{started:?}");
    let primed = text(&started.stdout);
    let mine = primed.split("Your Claims:").nth(1).unwrap_or_default();
    assert!(
        mine.lines().nth(1).unwrap_or_default().contains("TM-1"),
        "{primed}"
    );
    let other = hook(&scratch, &below, "session-start", "another");
    let theirs = text(&other.stdout);
    assert!(
        !theirs
            .split("Your Claims:")
            .nth(1)
            .unwrap_or_default()
            .lines()
            .nth(1)
            .unwrap_or_default()
            .contains("TM-1"),
        "{theirs}"
    );

    wait_until(expires_ms - 2_000 + 50);
    let beat = hook(&scratch, &below, "heartbeat", SESSION);
    assert!(beat.status.success(), "{beat:?}");
    assert_eq!(text(&beat.stdout), "", "nothing to tell");
    let renewed = claim_of(&scratch, &repo)["expires_ms"].as_i64().unwrap();
    assert!(renewed > expires_ms, "{renewed} after {expires_ms}");

    let ended = hook(&scratch, &below, "session-end", SESSION);
    assert!(ended.status.success(), "{ended:?}");
    assert_eq!(claim_of(&scratch, &repo), Value::Null);
    let again = hook(&scratch, &below, "session-end", SESSION);
    assert!(again.status.success(), "nothing left to release: {again:?}");

    let elsewhere = scratch.home().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let untracked = hook(&scratch, &elsewhere, "session-end", SESSION);
    assert!(untracked.status.success(), "{untracked:?}");
    assert_eq!(text(&untracked.stdout), "");
}

#[test]
fn a_codex_hook_without_a_session_on_stdin_fails() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    for (input, why) in [
        ("not json", "the hook's input is not JSON"),
        ("{}", "the hook's input has no usable session_id"),
        (
            r#"{"session_id": ""}"#,
            "the hook's input has no usable session_id",
        ),
    ] {
        let out = tm_piped(&scratch, &repo, &["hook", "heartbeat", "codex"], input);
        assert!(!out.status.success(), "{input}: {out:?}");
        assert!(text(&out.stderr).contains(why), "{input}: {out:?}");
    }
}

#[test]
fn setup_codex_writes_codex_hooks_and_takes_them_out() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let file = repo.join(".codex/hooks.json");

    let made = tm(&scratch, &repo, &["setup", "codex", "--yes"]);
    assert!(made.status.success(), "{made:?}");
    let shown = text(&made.stdout);
    assert!(shown.contains("approve each hook in /hooks"), "{shown}");
    assert!(shown.ends_with("Commit .codex/hooks.json\n"), "{shown}");
    let written: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    let hooks = &written["hooks"];
    assert_eq!(
        hooks["SessionStart"][0]["hooks"][0]["command"],
        "tm hook session-start codex"
    );
    let beat = &hooks["PostToolUse"][0];
    assert_eq!(beat["matcher"], "*");
    assert_eq!(beat["hooks"][0]["command"], "tm hook heartbeat codex");
    assert_eq!(beat["hooks"][0]["async"], true);
    let end = &hooks["SessionEnd"][0]["hooks"][0];
    assert_eq!(end["command"], "tm hook session-end codex");
    assert_eq!(end["timeout"], 3, "Codex caps SessionEnd at 3 seconds");
    for (event, status) in [
        ("SessionStart", "tm: priming the session"),
        ("PostToolUse", "tm: renewing Claims"),
        ("SessionEnd", "tm: releasing Claims"),
    ] {
        let handler = &hooks[event][0]["hooks"][0];
        assert_eq!(handler["type"], "command", "{event}");
        assert_eq!(handler["statusMessage"], status, "{event}");
    }
    assert!(!repo.join(".claude").exists());

    let again = tm(&scratch, &repo, &["setup", "codex", "--yes"]);
    assert_eq!(
        text(&again.stdout),
        "tm's hooks are already in .codex/hooks.json\n"
    );

    let undone = tm(&scratch, &repo, &["setup", "codex", "--undo", "--yes"]);
    assert!(undone.status.success(), "{undone:?}");
    assert!(!file.exists());

    // Each TOML form of a `hooks` key; the warning holds on a rerun too.
    let config = repo.join(".codex/config.toml");
    for inline in [
        "[hooks]\n",
        "[[hooks.SessionStart]]\nmatcher = \"*\"\n",
        "hooks.SessionStart = []\n",
    ] {
        fs::write(&config, inline).unwrap();
        for _ in 0..2 {
            let both = tm(&scratch, &repo, &["setup", "codex", "--yes"]);
            assert!(both.status.success(), "{inline}: {both:?}");
            assert!(
                text(&both.stderr).contains(".codex/config.toml also defines hooks"),
                "{inline}: {both:?}"
            );
        }
    }
    fs::write(&config, "model = \"o4\"\n").unwrap();
    let plain = tm(&scratch, &repo, &["setup", "codex", "--yes"]);
    assert_eq!(text(&plain.stderr), "", "{plain:?}");
    fs::write(&config, "[hooks\n").unwrap();
    let broken = tm(&scratch, &repo, &["setup", "codex", "--yes"]);
    assert!(
        text(&broken.stderr).contains(".codex/config.toml is not valid TOML"),
        "{broken:?}"
    );
}

/// Whether a hook started with `exported` releases the Claim the Agent's shell
/// took with `shell` beside its `CODEX_SESSION_ID`.
fn hook_releases(shell: &[(&str, &str)], exported: &[(&str, &str)]) -> bool {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    let taken = tm_command()
        .args(["claim", "TM-1"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("CODEX_SESSION_ID", SESSION)
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .envs(shell.iter().copied())
        .output()
        .unwrap();
    assert!(taken.status.success(), "{taken:?}");
    let mut ended = tm_command();
    ended
        .args(["hook", "session-end", "codex"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .envs(exported.iter().copied());
    let out = piped(ended, json!({ "session_id": SESSION }).to_string());
    assert!(out.status.success(), "{out:?}");
    claim_of(&scratch, &repo) == Value::Null
}

#[test]
fn a_codex_hook_acts_as_its_shell_whatever_codex_passes_on() {
    let outer = [("TM_AGENT", "reviewer"), ("TM_SESSION", "outer")];
    // Codex's default shell policy passes exported variables to its shell.
    assert!(hook_releases(&outer, &outer));
    // With inherit = core, only its hooks see them.
    assert!(hook_releases(&[], &[("TM_SESSION", "outer")]));
    // A Codex started from another Codex's shell inherits the outer id.
    assert!(hook_releases(&[], &[("CODEX_SESSION_ID", "outer")]));
}

#[test]
fn a_codex_heartbeat_tells_a_takeover_in_codexs_hook_json() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    let taken = from_codex_shell(&scratch, &repo, &["--json", "claim", "TM-1", "--for", "1s"]);
    let taken: Value = serde_json::from_slice(&taken.stdout).unwrap();
    wait_until(taken["data"]["claim"]["expires_ms"].as_i64().unwrap());
    let other = tm_command()
        .args(["claim", "TM-1"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "claude")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(other.status.success(), "{other:?}");

    // Exported where Codex starts, which its hooks see and its shell may not.
    let mut beat = tm_command();
    beat.args(["hook", "heartbeat", "codex"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_SESSION", "outer")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let told = piped(beat, json!({ "session_id": SESSION }).to_string());
    assert!(told.status.success(), "{told:?}");
    let hooks = scratch.home().join("hooks");
    assert!(hooks.join(format!("{SESSION}.reported")).exists());
    assert!(!hooks.join("outer.reported").exists());
    let shown: Value = serde_json::from_slice(&told.stdout).unwrap();
    assert_eq!(
        shown,
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": "tm: TM-1 is now held by claude for jace; your Claim lapsed",
            },
        })
    );
    let again = hook(&scratch, &repo, "heartbeat", SESSION);
    assert_eq!(text(&again.stdout), "", "told once");
}

#[test]
fn a_codex_hooks_notes_are_kept_under_the_session_it_writes_in() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    // No TM_ACTOR, so the heartbeat has an error to note.
    let mut beat = tm_command();
    beat.args(["hook", "heartbeat", "codex"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env_remove("TM_ACTOR")
        .env("TM_AGENT", "rev")
        .env("TM_SESSION", "outer")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let told = piped(beat, json!({ "session_id": SESSION }).to_string());
    assert!(
        text(&told.stdout).contains("cannot tell who you are"),
        "{told:?}"
    );
    let hooks = scratch.home().join("hooks");
    assert!(hooks.join("outer.reported").exists(), "{told:?}");
    assert!(!hooks.join(format!("{SESSION}.reported")).exists());
}

#[test]
fn a_codex_hook_at_a_terminal_refuses_rather_than_waiting() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    let mut session = tm_pty(&scratch, &repo, &["hook", "heartbeat", "codex"]);
    session
        .exp_string("tm hook reads the hook's JSON on stdin, which is a terminal here")
        .unwrap();
    session.exp_eof().unwrap();
}
