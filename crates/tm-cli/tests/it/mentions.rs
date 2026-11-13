//! `@mentions` are resolved when text is saved (ADR 0019).

use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

use crate::support::{Scratch, initialized, jace, tm};

/// `tm` writing as the human `actor`.
fn as_actor(scratch: &Scratch, repo: &Path, actor: &str, args: &[&str]) -> Output {
    let mut command: Command = jace(scratch, repo, args);
    command.env("TM_ACTOR", actor).stdin(Stdio::null());
    command.output().unwrap()
}

/// A Workspace where `kim@example.com` and `kim@other.org` have written, so
/// `@kim` names both, with TM-1 to comment on.
fn two_kims(scratch: &Scratch) -> std::path::PathBuf {
    let (repo, store, _) = initialized(scratch);
    drop(store);
    for kim in ["kim@example.com", "kim@other.org"] {
        let made = as_actor(
            scratch,
            &repo,
            kim,
            &["create", "--type", "task", "--title", kim],
        );
        assert!(made.status.success(), "{made:?}");
    }
    repo
}

fn doc(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|err| panic!("{err}: {out:?}"))
}

fn last_comment(scratch: &Scratch, repo: &Path) -> String {
    let shown = doc(&tm(
        scratch,
        repo,
        &["--json", "show", "TM-1", "--comments", "all"],
    ));
    let comments = shown["data"]["comments"].as_array().unwrap().clone();
    comments.last().unwrap()["body"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn a_mention_of_one_actor_is_saved_as_its_token_and_an_unknown_one_as_text() {
    let scratch = Scratch::new();
    let repo = two_kims(&scratch);
    let said = tm(
        &scratch,
        &repo,
        &[
            "comment",
            "TM-1",
            "thanks @kim@example.com, and `@kim` and @nobody",
        ],
    );
    assert!(said.status.success(), "{said:?}");
    assert_eq!(
        last_comment(&scratch, &repo),
        "thanks <@kim@example.com>, and `@kim` and @nobody"
    );
}

#[test]
fn a_mention_of_several_where_no_one_can_be_asked_is_refused_with_them() {
    let scratch = Scratch::new();
    let repo = two_kims(&scratch);
    for args in [
        &["--json", "comment", "TM-1", "ping @kim"][..],
        &[
            "--json",
            "create",
            "--type",
            "task",
            "--title",
            "t",
            "--set",
            "description=ask @kim",
        ][..],
    ] {
        let refused = tm(&scratch, &repo, args);
        assert!(!refused.status.success(), "{refused:?}");
        let said = doc(&refused);
        assert_eq!(said["error"], "ambiguous_mention", "{said}");
        let message = said["message"].as_str().unwrap();
        assert!(
            message.contains("@kim@example.com") && message.contains("@kim@other.org"),
            "{message}"
        );
    }
    let listed = doc(&tm(&scratch, &repo, &["--json", "list"]));
    assert_eq!(
        listed["data"]["issues"].as_array().unwrap().len(),
        2,
        "nothing was written"
    );
}

#[test]
fn a_person_at_a_terminal_picks_which_one_is_meant() {
    let scratch = Scratch::new();
    let repo = two_kims(&scratch);
    let tm = jace(&scratch, &repo, &[]);
    let mut command = Command::new(tm.get_program());
    command
        .current_dir(&repo)
        .args(["comment", "TM-1", "ping @kim"]);
    for (key, value) in tm.get_envs() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    let mut session = rexpect::session::spawn_command(command, Some(10_000)).unwrap();
    session.exp_string("names more than one").unwrap();
    session.exp_string("Which one?").unwrap();
    session.send_line("2").unwrap();
    session.exp_eof().unwrap();
    assert_eq!(last_comment(&scratch, &repo), "ping <@kim@other.org>");
}

#[test]
fn json_at_a_terminal_is_refused_with_the_text_not_asked() {
    let scratch = Scratch::new();
    let repo = two_kims(&scratch);
    let tm = jace(&scratch, &repo, &[]);
    let mut command = Command::new(tm.get_program());
    command
        .current_dir(&repo)
        .args(["--json", "comment", "TM-1", "ping @kim"]);
    for (key, value) in tm.get_envs() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    let mut session = rexpect::session::spawn_command(command, Some(10_000)).unwrap();
    let printed = session.exp_eof().unwrap();
    assert!(!printed.contains("Which one?"), "{printed}");
    assert!(printed.contains("\"ambiguous_mention\""), "{printed}");
    assert!(printed.contains("\"text\":\"ping @kim\""), "{printed}");
}

#[test]
fn a_mention_is_shown_by_name_and_listed_apart_in_json() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    let mut named = jace(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "a"],
    );
    named
        .env_remove("TM_ACTOR")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "user.email")
        .env("GIT_CONFIG_VALUE_0", "kim@example.com")
        .env("GIT_CONFIG_KEY_1", "user.name")
        .env("GIT_CONFIG_VALUE_1", "Kim Lee")
        .stdin(Stdio::null());
    assert!(named.output().unwrap().status.success());
    let said = tm(&scratch, &repo, &["comment", "TM-1", "thanks @Kim Lee"]);
    assert!(said.status.success(), "{said:?}");
    let printed = crate::support::text(&tm(&scratch, &repo, &["show", "TM-1"]).stdout);
    assert!(printed.contains("thanks @Kim Lee"), "{printed}");
    assert!(!printed.contains("<@"), "{printed}");
    let shown = doc(&tm(&scratch, &repo, &["--json", "show", "TM-1"]));
    assert_eq!(
        shown["data"]["mentions"],
        serde_json::json!([{
            "token": "<@kim@example.com>",
            "person": "kim@example.com",
            "shown": "@Kim Lee",
        }])
    );
    let history = crate::support::text(&tm(&scratch, &repo, &["history", "TM-1"]).stdout);
    assert!(history.contains("commented: thanks @Kim Lee"), "{history}");
}

#[test]
fn only_known_actors_are_named_wherever_their_tokens_sit() {
    let scratch = Scratch::new();
    let repo = two_kims(&scratch);
    let set = tm(
        &scratch,
        &repo,
        &[
            "edit",
            "TM-1",
            "--set",
            "description=type ` to quote @kim@example.com",
        ],
    );
    assert!(set.status.success(), "{set:?}");
    for body in [
        "thanks @kim@example.com, see `x`",
        "literal <@not-a-person> and <@a for b> here",
    ] {
        let said = tm(&scratch, &repo, &["comment", "TM-1", body]);
        assert!(said.status.success(), "{said:?}");
    }
    let history = crate::support::text(&tm(&scratch, &repo, &["history", "TM-1"]).stdout);
    assert!(
        history.contains("commented: thanks @kim@example.com, see `x`"),
        "a backtick in another text does not hide a mention: {history}"
    );
    let printed = crate::support::text(&tm(&scratch, &repo, &["show", "TM-1"]).stdout);
    assert!(
        printed.contains("literal <@not-a-person> and <@a for b> here"),
        "{printed}"
    );
    let shown = doc(&tm(&scratch, &repo, &["--json", "show", "TM-1"]));
    let listed: Vec<&str> = shown["data"]["mentions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|mention| mention["token"].as_str().unwrap())
        .collect();
    assert_eq!(listed, ["<@kim@example.com>"]);
    let logged = doc(&tm(&scratch, &repo, &["--json", "history", "TM-1"]));
    let lines: Vec<&str> = logged["data"]["writes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|write| write["changes"].as_array().unwrap())
        .map(|change| change["line"].as_str().unwrap())
        .collect();
    assert!(
        lines.contains(&"Description: (none) → type ` to quote @kim@example.com"),
        "a history line is named, its stored change kept: {lines:?}"
    );
}

#[test]
fn a_mention_only_in_the_description_is_named_in_history_json() {
    let scratch = Scratch::new();
    let repo = two_kims(&scratch);
    let set = tm(
        &scratch,
        &repo,
        &["edit", "TM-1", "--set", "description=ping @kim@example.com"],
    );
    assert!(set.status.success(), "{set:?}");
    let logged = doc(&tm(&scratch, &repo, &["--json", "history", "TM-1"]));
    let lines = logged["data"]["writes"].to_string();
    assert!(
        lines.contains("\"line\":\"Description: (none) → ping @kim@example.com\""),
        "{lines}"
    );
}
