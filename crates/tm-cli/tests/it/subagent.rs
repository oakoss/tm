//! Claude Code sub-agents: `tm hook pre-bash` names a sub-agent in its tm
//! commands, and the other hooks act for the sub-agent the hook's stdin names.

use std::path::Path;
use std::process::Output;

use serde_json::{Value, json};
use tm_core::Actor;

use crate::support::{Scratch, initialized, piped, text, tm, tm_command, wait_until};

const SESSION: &str = "ddc943db-8b98-424d-8fef-3dc1a05051a4";
const AGENT_ID: &str = "a0debfb50c6e9ec8e";
const NAME: &str = "general-purpose-a0debfb5";

fn pre_bash(scratch: &Scratch, repo: &Path, input: &Value) -> Output {
    let mut command = tm_command();
    command
        .args(["hook", "pre-bash", "claude"])
        .current_dir(repo)
        .env("TM_HOME", scratch.home());
    piped(command, input.to_string())
}

fn bash_input(agent: Option<(&str, &str)>, command: &str) -> Value {
    let mut input = json!({
        "session_id": SESSION,
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": command, "description": "d", "timeout": 5000 },
    });
    if let Some((id, kind)) = agent {
        input["agent_id"] = json!(id);
        input["agent_type"] = json!(kind);
    }
    input
}

/// `tm` from a Bash command of this Claude Code session, as `agent`.
fn from_session(scratch: &Scratch, repo: &Path, agent: Option<&str>, args: &[&str]) -> Output {
    let mut command = tm_command();
    command
        .args(args)
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("CLAUDE_CODE_SESSION_ID", SESSION)
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1");
    if let Some(agent) = agent {
        command.env("TM_AGENT", agent);
    }
    command.output().unwrap()
}

/// A Claude Code hook of this session, for the sub-agent `agent_id` names.
fn hook(scratch: &Scratch, repo: &Path, event: &str, agent_id: Option<&str>) -> Output {
    let mut input = json!({ "session_id": SESSION, "hook_event_name": event });
    if let Some(id) = agent_id {
        input["agent_id"] = json!(id);
        input["agent_type"] = json!("general-purpose");
    }
    let mut command = tm_command();
    command
        .args(["hook", event, "claude"])
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("CLAUDE_CODE_SESSION_ID", SESSION)
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1");
    piped(command, input.to_string())
}

fn holder(scratch: &Scratch, repo: &Path, key: &str) -> Value {
    let out = tm(scratch, repo, &["--json", "show", key]);
    serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"]["claim"]["agent"].clone()
}

#[test]
fn a_sub_agents_tm_command_exports_its_name_first() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    let out = pre_bash(
        &scratch,
        &repo,
        &bash_input(Some((AGENT_ID, "general-purpose")), "cd x && tm claim TM-1"),
    );
    assert!(out.status.success(), "{out:?}");
    let shown: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        shown,
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "updatedInput": {
                    "command": format!("export TM_AGENT={NAME}; cd x && tm claim TM-1"),
                    "description": "d",
                    "timeout": 5000,
                },
            },
        })
    );

    let odd = pre_bash(
        &scratch,
        &repo,
        &bash_input(Some(("ab'c; rm", "my agent$(x)")), "tm ready"),
    );
    let odd: Value = serde_json::from_slice(&odd.stdout).unwrap();
    assert_eq!(
        odd["hookSpecificOutput"]["updatedInput"]["command"],
        "export TM_AGENT=my-agent--x--ab-c--rm; tm ready"
    );

    let sub = Some((AGENT_ID, "general-purpose"));
    for command in [
        "x;tm ready",
        "echo $(tm next)",
        "/usr/local/bin/tm ready",
        "tm claim TM-1 && printf '%s' 'TM_AGENT='",
        "tm create \"set TM_AGENT=x for sub-agents\"",
        "TM_AGENT=mine tm next",
    ] {
        let out = pre_bash(&scratch, &repo, &bash_input(sub, command));
        let shown: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            shown["hookSpecificOutput"]["updatedInput"]["command"],
            format!("export TM_AGENT={NAME}; {command}"),
        );
    }

    for (agent, command) in [
        (None, "tm claim TM-1"),
        (sub, "cargo test"),
        (sub, "echo html"),
        (sub, "tmux ls"),
    ] {
        let out = pre_bash(&scratch, &repo, &bash_input(agent, command));
        assert!(out.status.success(), "{command}: {out:?}");
        assert_eq!(text(&out.stdout), "", "{command}");
    }
    // The name exported first gives way to one the command sets itself.
    for (command, runs_as) in [
        ("printenv TM_AGENT && : tm", NAME),
        ("TM_AGENT=mine printenv TM_AGENT && : tm", "mine"),
        ("export TM_AGENT=mine; printenv TM_AGENT && : tm", "mine"),
    ] {
        let out = pre_bash(&scratch, &repo, &bash_input(sub, command));
        let shown: Value = serde_json::from_slice(&out.stdout).unwrap();
        let rewritten = shown["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap();
        let ran = std::process::Command::new("sh")
            .args(["-c", rewritten])
            .env_remove("TM_AGENT")
            .output()
            .unwrap();
        assert_eq!(text(&ran.stdout), format!("{runs_as}\n"), "{command}");
    }

    let mut empty = tm_command();
    empty
        .args(["hook", "pre-bash", "claude"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home());
    let out = piped(empty, "");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(text(&out.stdout), "");
}

#[test]
fn sub_agents_renew_and_release_their_own_claims_and_the_session_end_releases_all() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["a", "b", "c"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    let taken = from_session(
        &scratch,
        &repo,
        Some(NAME),
        &["--json", "claim", "TM-1", "--for", "3s"],
    );
    assert!(taken.status.success(), "{taken:?}");
    let taken: Value = serde_json::from_slice(&taken.stdout).unwrap();
    let expires_ms = taken["data"]["claim"]["expires_ms"].as_i64().unwrap();
    let other = from_session(
        &scratch,
        &repo,
        Some("Explore-b1c2d3e4"),
        &["claim", "TM-2"],
    );
    assert!(other.status.success(), "{other:?}");
    let main = from_session(&scratch, &repo, None, &["claim", "TM-3"]);
    assert!(main.status.success(), "{main:?}");
    assert_eq!(holder(&scratch, &repo, "TM-1"), NAME);
    assert_eq!(holder(&scratch, &repo, "TM-3"), "claude");

    wait_until(expires_ms - 2_000 + 50);
    let beat = hook(&scratch, &repo, "heartbeat", Some(AGENT_ID));
    assert!(beat.status.success(), "{beat:?}");
    let shown = tm(&scratch, &repo, &["--json", "show", "TM-1"]);
    let renewed =
        serde_json::from_slice::<Value>(&shown.stdout).unwrap()["data"]["claim"]["expires_ms"]
            .as_i64()
            .unwrap();
    assert!(renewed > expires_ms, "{renewed} after {expires_ms}");

    let stopped = hook(&scratch, &repo, "subagent-stop", Some(AGENT_ID));
    assert!(stopped.status.success(), "{stopped:?}");
    assert_eq!(text(&stopped.stdout), "released TM-1\n");
    assert_eq!(holder(&scratch, &repo, "TM-2"), "Explore-b1c2d3e4");
    assert_eq!(holder(&scratch, &repo, "TM-3"), "claude");

    let ended = hook(&scratch, &repo, "session-end", None);
    assert!(ended.status.success(), "{ended:?}");
    assert_eq!(text(&ended.stdout), "released TM-2, TM-3\n");
    for key in ["TM-1", "TM-2", "TM-3"] {
        assert_eq!(holder(&scratch, &repo, key), Value::Null, "{key}");
    }
}

#[test]
fn the_session_end_leaves_other_sessions_and_humans_claims() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["a", "b"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    let elsewhere = tm_command()
        .args(["claim", "TM-1"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", NAME)
        .env("CLAUDE_CODE_SESSION_ID", "another")
        .output()
        .unwrap();
    assert!(elsewhere.status.success(), "{elsewhere:?}");
    let someone = tm_command()
        .args(["claim", "TM-2"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "kim")
        .env("TM_AGENT", NAME)
        .env("CLAUDE_CODE_SESSION_ID", SESSION)
        .output()
        .unwrap();
    assert!(someone.status.success(), "{someone:?}");

    let ended = hook(&scratch, &repo, "session-end", None);
    assert!(ended.status.success(), "{ended:?}");
    assert_eq!(text(&ended.stdout), "this session holds no Claims\n");
    assert_eq!(holder(&scratch, &repo, "TM-1"), NAME);
    assert_eq!(holder(&scratch, &repo, "TM-2"), NAME);
}

#[test]
fn the_session_end_leaves_a_claim_its_sub_agent_handed_off() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    let taken = from_session(&scratch, &repo, Some(NAME), &["claim", "TM-1"]);
    assert!(taken.status.success(), "{taken:?}");
    let handed = from_session(
        &scratch,
        &repo,
        Some(NAME),
        &["handoff", "TM-1", "--to", "codex", "carry on"],
    );
    assert!(handed.status.success(), "{handed:?}");

    let ended = hook(&scratch, &repo, "session-end", None);
    assert!(ended.status.success(), "{ended:?}");
    assert_eq!(text(&ended.stdout), "this session holds no Claims\n");
    assert_eq!(holder(&scratch, &repo, "TM-1"), "codex");
}

#[test]
fn a_subagent_stop_without_an_agent_id_releases_nothing() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    drop(store);
    let main = from_session(&scratch, &repo, None, &["claim", "TM-1"]);
    assert!(main.status.success(), "{main:?}");
    let stopped = hook(&scratch, &repo, "subagent-stop", None);
    assert!(!stopped.status.success(), "{stopped:?}");
    assert!(
        text(&stopped.stderr).contains("the hook's input has no agent_id"),
        "{stopped:?}"
    );
    assert_eq!(holder(&scratch, &repo, "TM-1"), "claude");
}
