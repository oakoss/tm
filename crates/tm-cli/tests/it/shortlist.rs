//! `tm shortlist`: showing, creating and changing Short-lists (ADR 0016).

use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};
use tm_core::Actor;

use crate::support::{Scratch, initialized, text, tm, tm_command};

/// `tm --json` with `words` split on spaces, as jace.
fn run(scratch: &Scratch, repo: &Path, words: &str) -> (Option<i32>, Value) {
    let args: Vec<&str> = ["--json"].into_iter().chain(words.split(' ')).collect();
    let out = tm(scratch, repo, &args);
    let doc = serde_json::from_slice(&out.stdout).unwrap();
    (out.status.code(), doc)
}

/// `tm` as the Agent `agent`, writing for `jace`.
fn as_agent(scratch: &Scratch, repo: &Path, agent: &str, words: &str) -> Output {
    tm_command()
        .args(words.split(' '))
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", agent)
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

fn shown(scratch: &Scratch, repo: &Path) -> String {
    text(&tm(scratch, repo, &["shortlist"]).stdout)
}

fn keys(doc: &Value) -> Vec<String> {
    doc["data"]["shortlists"][0]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| member["key"].as_str().unwrap().to_owned())
        .collect()
}

/// A repo with four todo Issues, TM-1 to TM-4, and a list `docs` worked
/// through by jace and claude.
fn with_list(scratch: &Scratch) -> PathBuf {
    let (repo, mut store, project) = initialized(scratch);
    for title in ["a", "b", "c", "d"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    let created = "shortlist create docs --actor @me --actor claude";
    let (code, doc) = run(scratch, &repo, created);
    assert_eq!(code, Some(0), "{doc}");
    let actors = &doc["data"]["shortlists"][0]["actors"];
    assert_eq!(actors, &json!(["claude", "jace"]));
    repo
}

#[test]
fn issues_are_added_moved_and_removed_in_order() {
    let scratch = Scratch::new();
    let repo = with_list(&scratch);
    let (code, doc) = run(&scratch, &repo, "shortlist add docs TM-1 TM-2");
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(keys(&doc), ["TM-1", "TM-2"]);
    let (_, doc) = run(&scratch, &repo, "shortlist add docs TM-3 --top");
    assert_eq!(keys(&doc), ["TM-3", "TM-1", "TM-2"]);
    let (_, doc) = run(&scratch, &repo, "shortlist add docs TM-4 --after TM-1");
    assert_eq!(keys(&doc), ["TM-3", "TM-1", "TM-4", "TM-2"]);
    let (_, doc) = run(&scratch, &repo, "shortlist move TM-3 --end");
    assert_eq!(keys(&doc), ["TM-1", "TM-4", "TM-2", "TM-3"]);
    let first = &doc["data"]["shortlists"][0]["members"][0];
    assert_eq!(first["position"], 1);
    // Ranks are never shown; position stands for them.
    assert_eq!(first["fields"].get("shortlist_rank"), None);
    let (_, doc) = run(&scratch, &repo, "shortlist remove TM-4 TM-2");
    assert_eq!(keys(&doc), ["TM-1", "TM-3"]);

    let batch = doc["data"]["batch"].as_str().unwrap().to_owned();
    let (code, _) = run(&scratch, &repo, &format!("undo {batch}"));
    assert_eq!(code, Some(0));
    let list = shown(&scratch, &repo);
    let heading = "docs  (claude, jace)  4 of 10\n";
    assert!(list.starts_with(heading), "{list}");
    assert!(list.contains("  2  TM-4"), "{list}");
}

#[test]
fn a_session_sees_the_lists_that_name_it() {
    let scratch = Scratch::new();
    let repo = with_list(&scratch);
    let claude = as_agent(&scratch, &repo, "claude", "shortlist");
    assert!(text(&claude.stdout).starts_with("docs  "), "{claude:?}");
    let codex = as_agent(&scratch, &repo, "codex", "shortlist");
    let none = "no Short-list names codex; tm shortlist --all";
    assert!(text(&codex.stdout).starts_with(none), "{codex:?}");
    let all = as_agent(&scratch, &repo, "codex", "shortlist --all");
    assert!(text(&all.stdout).starts_with("docs  "), "{all:?}");
}

#[test]
fn members_that_wait_are_marked_with_why() {
    let scratch = Scratch::new();
    let repo = with_list(&scratch);
    run(&scratch, &repo, "shortlist add docs TM-1 TM-2 TM-3");
    let started = as_agent(&scratch, &repo, "claude", "start TM-2");
    assert!(started.status.success(), "{started:?}");
    let (_, doc) = run(&scratch, &repo, "link TM-4 blocks TM-3");
    assert!(doc["data"]["link"].is_object(), "{doc}");

    let (code, doc) = run(&scratch, &repo, "shortlist");
    assert_eq!(code, Some(0), "{doc}");
    let members = &doc["data"]["shortlists"][0]["members"];
    assert_eq!(members[0]["passed_over"], json!([]));
    assert_eq!(members[1]["passed_over"], json!(["claimed", "started"]));
    assert_eq!(members[2]["passed_over"], json!(["blocked"]));
    let list = shown(&scratch, &repo);
    assert!(list.contains("[claimed, started]"), "{list}");
}

#[test]
fn refusals_carry_a_code_and_a_fix() {
    let scratch = Scratch::new();
    let repo = with_list(&scratch);
    let (code, doc) = run(&scratch, &repo, "shortlist edit docs --size 1");
    assert_eq!(code, Some(0), "{doc}");
    run(&scratch, &repo, "shortlist add docs TM-1");
    let (code, doc) = run(&scratch, &repo, "shortlist add docs TM-2");
    assert_eq!(code, Some(1));
    assert_eq!(doc["error"], "shortlist_full");
    assert!(doc["fix"].as_str().unwrap().contains("--size"), "{doc}");

    run(&scratch, &repo, "move TM-3 done");
    let (_, doc) = run(&scratch, &repo, "shortlist edit docs --size 10");
    assert_eq!(doc["data"]["shortlists"][0]["size"], 10);
    let (code, doc) = run(&scratch, &repo, "shortlist add docs TM-3");
    assert_eq!(code, Some(1));
    assert_eq!(doc["error"], "cannot_join");
    assert_eq!(doc["key"], "TM-3");

    let (code, doc) = run(&scratch, &repo, "shortlist add ops TM-2");
    assert_eq!(code, Some(1));
    assert_eq!(doc["error"], "not_found");
    assert_eq!(doc["name"], "ops");
    for words in ["shortlist move TM-2 --top", "shortlist remove TM-2"] {
        let (code, doc) = run(&scratch, &repo, words);
        assert_eq!(code, Some(1));
        assert_eq!(doc["error"], "not_on_shortlist");
        assert_eq!(doc["key"], "TM-2", "{words}: named by its Key");
    }
    let (code, doc) = run(&scratch, &repo, "shortlist edit docs --remove-actor nobody");
    assert_eq!(code, Some(1));
    assert_eq!(doc["error"], "invalid_value");
    assert!(doc["message"].as_str().unwrap().contains("nobody"), "{doc}");
}

#[test]
fn an_edit_that_changes_nothing_writes_nothing() {
    let scratch = Scratch::new();
    let repo = with_list(&scratch);
    for words in [
        "shortlist edit docs --name docs",
        "shortlist edit docs --size 10 --actor claude",
    ] {
        let (code, doc) = run(&scratch, &repo, words);
        assert_eq!(code, Some(0), "{doc}");
        assert_eq!(doc["data"]["batch"], Value::Null, "{words}");
    }
    let args = ["shortlist", "edit", "docs", "--name", "docs"];
    let out = tm(&scratch, &repo, &args);
    assert_eq!(text(&out.stdout), "docs: nothing changed\n");
}

#[test]
fn a_remove_across_lists_names_each_list_left() {
    let scratch = Scratch::new();
    let repo = with_list(&scratch);
    run(&scratch, &repo, "shortlist create ops --actor @me");
    run(&scratch, &repo, "shortlist add docs TM-1 TM-2");
    run(&scratch, &repo, "shortlist add ops TM-3");
    let out = tm(&scratch, &repo, &["shortlist", "remove", "TM-1", "TM-3"]);
    let said = text(&out.stdout);
    let took = "took TM-1 off docs\ntook TM-3 off ops\n";
    assert!(said.starts_with(took), "{said}");
    // An add shows the list an Issue left as well as the one it joined.
    let (_, doc) = run(&scratch, &repo, "shortlist add ops TM-2 TM-4");
    let names: Vec<&Value> = doc["data"]["shortlists"]
        .as_array()
        .unwrap()
        .iter()
        .map(|list| &list["name"])
        .collect();
    assert_eq!(names, [&json!("ops"), &json!("docs")]);
    // Reordering a member shows its list once.
    let (_, doc) = run(&scratch, &repo, "shortlist add ops TM-4 --top");
    assert_eq!(doc["data"]["shortlists"].as_array().unwrap().len(), 1);
}

#[test]
fn a_list_can_be_named_by_its_id() {
    let scratch = Scratch::new();
    let repo = with_list(&scratch);
    let (_, doc) = run(&scratch, &repo, "shortlist");
    let id = &doc["data"]["shortlists"][0]["id"];
    let words = format!("shortlist edit {} --name guides", id.as_str().unwrap());
    let (code, doc) = run(&scratch, &repo, &words);
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(doc["data"]["shortlists"][0]["name"], "guides");

    // A name that looks like an id still reaches its list by name.
    let named = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
    let created = format!("shortlist create {named} --actor @me");
    run(&scratch, &repo, &created);
    let (code, doc) = run(&scratch, &repo, &format!("shortlist delete {named}"));
    assert_eq!(code, Some(0), "{doc}");
}

#[test]
fn a_placement_that_changes_nothing_says_so() {
    let scratch = Scratch::new();
    let repo = with_list(&scratch);
    run(&scratch, &repo, "shortlist add docs TM-1");
    let out = tm(&scratch, &repo, &["shortlist", "move", "TM-1", "--top"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(text(&out.stdout), "docs: nothing changed\n");
}

#[test]
fn a_deleted_list_takes_its_issues_off_and_undo_restores_both() {
    let scratch = Scratch::new();
    let repo = with_list(&scratch);
    run(&scratch, &repo, "shortlist add docs TM-1 TM-2");
    let (code, doc) = run(&scratch, &repo, "shortlist delete docs");
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(doc["data"]["shortlists"][0]["deleted"], true);
    let (_, all) = run(&scratch, &repo, "shortlist --all");
    assert_eq!(all["data"]["shortlists"], json!([]));
    let batch = doc["data"]["batch"].as_str().unwrap().to_owned();
    run(&scratch, &repo, &format!("undo {batch}"));
    let (_, mine) = run(&scratch, &repo, "shortlist");
    assert_eq!(mine["data"]["shortlists"][0]["count"], 2);
}

#[test]
fn show_names_the_list_and_no_command_shows_a_rank() {
    let scratch = Scratch::new();
    let repo = with_list(&scratch);
    run(&scratch, &repo, "shortlist add docs TM-1 TM-2");
    let shown = text(&tm(&scratch, &repo, &["show", "TM-2"]).stdout);
    let line = "shortlist: docs (position 2 of 2)\n";
    assert!(shown.contains(line), "{shown}");
    assert!(!shown.contains("shortlist_rank"), "{shown}");

    let (_, doc) = run(&scratch, &repo, "show TM-2");
    let membership = &doc["data"]["shortlist"];
    assert_eq!(membership["name"], "docs");
    assert_eq!(membership["position"], 2);
    let fields = &doc["data"]["fields"];
    assert_eq!(fields.get("shortlist"), None);
    assert_eq!(fields.get("shortlist_rank"), None);
    for words in ["list", "ready"] {
        let (_, doc) = run(&scratch, &repo, words);
        let issues = doc["data"]["issues"].as_array().unwrap();
        assert!(!issues.is_empty(), "{words}");
        for issue in issues {
            assert_eq!(issue["fields"].get("shortlist_rank"), None, "{words}");
        }
    }

    run(&scratch, &repo, "shortlist move TM-2 --top");
    let history = text(&tm(&scratch, &repo, &["history", "TM-2"]).stdout);
    assert!(history.contains("Short-list: (none) → docs\n"), "{history}");
    // Joining sets a rank too, which the list line covers.
    let reordered = history.matches("Short-list: reordered").count();
    assert_eq!(reordered, 1, "{history}");
    assert!(!history.contains("rank"), "{history}");
    let (_, doc) = run(&scratch, &repo, "history TM-2");
    for write in doc["data"]["writes"].as_array().unwrap() {
        for change in write["changes"].as_array().unwrap() {
            assert_ne!(change["change"]["field"], "shortlist_rank", "{change}");
        }
    }

    let (_, doc) = run(&scratch, &repo, "shortlist remove TM-1");
    let batch = doc["data"]["batch"].as_str().unwrap().to_owned();
    let undone = text(&tm(&scratch, &repo, &["undo", &batch]).stdout);
    let line = "TM-1 Short-list: (none) -> docs\n";
    assert!(undone.contains(line), "{undone}");
    assert!(!undone.contains("rank"), "{undone}");
    assert!(!undone.contains("order"), "{undone}");
}

#[test]
fn tm_list_selects_a_lists_issues() {
    let scratch = Scratch::new();
    let repo = with_list(&scratch);
    run(&scratch, &repo, "shortlist add docs TM-1 TM-3");
    let (code, doc) = run(&scratch, &repo, "list shortlist:docs sort:title");
    assert_eq!(code, Some(0), "{doc}");
    let keys: Vec<&Value> = doc["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| &issue["key"])
        .collect();
    assert_eq!(keys, [&json!("TM-1"), &json!("TM-3")]);

    let (code, doc) = run(&scratch, &repo, "list shortlist:nope");
    assert_eq!(code, Some(1));
    assert_eq!(doc["error"], "not_found");
    assert_eq!(doc["kind"], "shortlist");
    assert_eq!(doc["fix"], "tm shortlist --all");
}

#[test]
fn membership_is_changed_only_through_tm_shortlist() {
    let scratch = Scratch::new();
    let repo = with_list(&scratch);
    for words in [
        "edit TM-1 --set shortlist=x",
        "edit TM-1 --unset shortlist_rank",
        "create --type task --title t --set shortlist=x",
        "move TM-1 in_progress --set shortlist_rank=V",
    ] {
        let args: Vec<&str> = words.split(' ').collect();
        let out = tm(&scratch, &repo, &args);
        assert!(!out.status.success(), "{words}");
        let said = text(&out.stderr);
        assert!(said.contains("tm shortlist add"), "{words}: {said}");
    }
}
