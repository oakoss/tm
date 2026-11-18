//! The Scheme read from the Workspace's and the repo's Scheme files
//! (ADR 0022).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use tm_store::workspace::{RepoFile, Workspace};

use crate::support::{Scratch, data, doc, initialized, tm, tm_piped};

/// The Workspace directory the repo file at `repo` names.
fn workspace_dir(scratch: &Scratch, repo: &Path) -> PathBuf {
    let file = RepoFile::parse(&fs::read_to_string(repo.join(".config/tm.toml")).unwrap()).unwrap();
    Workspace::at(&scratch.home(), file.workspace).dir
}

fn repo_scheme(repo: &Path, text: &str) {
    fs::create_dir_all(repo.join(".config/tm")).unwrap();
    fs::write(repo.join(".config/tm/scheme.toml"), text).unwrap();
}

#[test]
fn a_workspace_field_applies_and_a_repo_removal_leaves_its_values_readable() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    let workspace = workspace_dir(&scratch, &repo);
    fs::write(
        workspace.join("scheme.toml"),
        "[fields.area]\nlabel = \"Area\"\ntype = \"number\"\n",
    )
    .unwrap();
    let created = tm(
        &scratch,
        &repo,
        &[
            "--json", "create", "--type", "task", "--title", "t", "--set", "area=3",
        ],
    );
    assert!(created.status.success(), "{created:?}");
    assert_eq!(data(&created)["fields"]["area"], json!(3));

    repo_scheme(&repo, "[projects.TM.remove]\nfields = [\"area\"]\n");
    let refused = tm(
        &scratch,
        &repo,
        &["--json", "edit", "TM-1", "--set", "area=4"],
    );
    assert!(!refused.status.success(), "{refused:?}");
    let refused = doc(&refused);
    assert_eq!(refused["error"], "unknown_field", "{refused}");
    assert!(
        refused["message"].as_str().unwrap().contains("area"),
        "{refused}"
    );
    for args in [
        &["show", "TM-1"][..],
        &["list"],
        &["list", "type:task"],
        &["export"],
        &["rebuild"],
    ] {
        let out = tm(&scratch, &repo, args);
        assert!(out.status.success(), "{args:?}: {out:?}");
    }
}

#[test]
fn scheme_show_lists_the_loaded_fields_and_where_each_comes_from() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    fs::write(
        workspace_dir(&scratch, &repo).join("scheme.toml"),
        "[fields.area]\ntype = \"number\"\n",
    )
    .unwrap();
    repo_scheme(
        &repo,
        "[projects.TM.fields.tier]\nlabel = \"Tier\"\ntype = \"single_select\"\noptions = [{ id = \"gold\", label = \"Gold\" }, { id = \"free\" }]\n",
    );

    let shown = tm(&scratch, &repo, &["--json", "scheme", "show"]);
    let fields = data(&shown)["fields"].as_array().unwrap().clone();
    let from = |name: &str| {
        let field = fields.iter().find(|field| field["name"] == name).unwrap();
        field["from"].clone()
    };
    assert_eq!(from("title"), "built_in");
    assert_eq!(from("area"), "workspace");
    assert_eq!(from("tier"), "repo");

    let text = tm(&scratch, &repo, &["scheme", "show", "task"]);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(
        text.contains("field area (area) [from the Workspace's Scheme]\n"),
        "{text}"
    );
    assert!(
        text.contains("field tier (Tier) [from the repo's Scheme]: gold (Gold), free\n"),
        "{text}"
    );

    repo_scheme(&repo, "[projects.TM.fields.tier]\ntype = \"colour\"\n");
    let refused = tm(&scratch, &repo, &["--json", "scheme", "show"]);
    assert!(!refused.status.success());
    assert_eq!(doc(&refused)["error"], "invalid_scheme");

    let outside = tm(&scratch, &scratch.0, &["--json", "scheme", "show"]);
    assert!(outside.status.success(), "{outside:?}");
    assert!(
        doc(&outside)["warnings"][0]
            .as_str()
            .unwrap()
            .contains("built-in Scheme"),
        "{outside:?}"
    );
}

/// A repo whose Workspace Scheme gives bug only severity and chore no
/// Scheme field.
fn typed_fields(scratch: &Scratch) -> PathBuf {
    let (repo, store, _) = initialized(scratch);
    drop(store);
    fs::write(
        workspace_dir(scratch, &repo).join("scheme.toml"),
        r#"
[fields.severity]
type = "text"

[fields.area]
type = "text"

[types.bug]
label = "Bug"
workflow = "default"
fields = ["severity"]

[types.chore]
workflow = "default"
fields = []
"#,
    )
    .unwrap();
    repo
}

#[test]
fn a_type_offers_only_the_fields_it_carries() {
    let scratch = Scratch::new();
    let repo = typed_fields(&scratch);
    let shown = tm(&scratch, &repo, &["scheme", "show", "bug"]);
    let text = String::from_utf8_lossy(&shown.stdout);
    assert!(text.contains("field severity (severity)"), "{text}");
    assert!(text.contains("field priority (Priority)"), "{text}");
    assert!(!text.contains("field area"), "{text}");
    let shown = tm(&scratch, &repo, &["--json", "scheme", "show", "bug"]);
    let names: Vec<_> = data(&shown)["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|field| field["name"].clone())
        .collect();
    assert!(names.contains(&json!("severity")) && names.contains(&json!("waiting_on")));
    assert!(!names.contains(&json!("area")), "{names:?}");
    assert_eq!(data(&shown)["type"]["fields"], json!(["severity"]));
    let task = tm(&scratch, &repo, &["--json", "scheme", "show", "task"]);
    assert!(data(&task)["type"].get("fields").is_none(), "{task:?}");
    let shown = tm(&scratch, &repo, &["scheme", "show"]);
    let text = String::from_utf8_lossy(&shown.stdout);
    assert!(
        text.contains("type bug (Bug) [from the Workspace's Scheme]: carries severity\n"),
        "{text}"
    );
    assert!(
        text.contains("type chore (chore) [from the Workspace's Scheme]: no Scheme fields\n"),
        "{text}"
    );
    let chore = tm(&scratch, &repo, &["scheme", "show", "chore"]);
    let text = String::from_utf8_lossy(&chore.stdout);
    assert!(text.contains("field waiting_on"), "{text}");
    assert!(!text.contains("field severity"), "{text}");
}

#[test]
fn a_workflow_allows_only_the_moves_it_lists() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    fs::write(
        workspace_dir(&scratch, &repo).join("scheme.toml"),
        r#"
[workflows.gated]
start = "todo"
statuses = [
  { id = "todo", category = "todo" },
  { id = "doing", category = "active" },
  { id = "done", category = "done" },
]
transitions = [
  { from = "todo", to = ["done"] },
  { from = "done", to = [] },
]

[types.chore]
workflow = "gated"
"#,
    )
    .unwrap();
    let run = |args: &[&str]| tm(&scratch, &repo, args);
    assert!(
        run(&["create", "--type", "chore", "--title", "c"])
            .status
            .success()
    );
    let scheme = run(&["scheme", "show", "chore"]);
    let text = String::from_utf8_lossy(&scheme.stdout);
    assert!(
        text.contains("  todo (todo): todo, moves to done\n"),
        "{text}"
    );
    assert!(text.contains("  doing (doing): active\n"), "{text}");
    assert!(
        text.contains("  done (done): done, moves nowhere\n"),
        "{text}"
    );
    let shown = run(&["show", "TM-1"]);
    assert!(
        String::from_utf8_lossy(&shown.stdout).contains("moves to: done\n"),
        "{shown:?}"
    );
    let refused = run(&["--json", "move", "TM-1", "doing"]);
    let error = doc(&refused);
    assert_eq!(error["error"], "invalid_value", "{error}");
    assert_eq!(error["reason"], "cannot go from todo to doing", "{error}");
    assert_eq!(error["allowed"], json!(["done"]), "{error}");

    let started = run(&["--json", "start", "TM-1"]);
    let error = doc(&started);
    assert_eq!(error["error"], "invalid_value", "{error}");
    assert_eq!(error["allowed"], json!(["done"]), "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("its Workflow allows no active Status from todo"),
        "{error}"
    );

    let child = run(&[
        "--json", "create", "--type", "task", "--title", "t", "--parent", "TM-1",
    ]);
    assert!(child.status.success(), "{child:?}");
    let started = run(&["--json", "start", "TM-2"]);
    let doc = doc(&started);
    assert_eq!(data(&started)["fields"]["status"], "in_progress", "{doc}");
    assert!(
        doc["warnings"]
            .to_string()
            .contains("TM-1 was not started: its Workflow allows no active Status from todo"),
        "{doc}"
    );
    assert!(run(&["move", "TM-1", "done"]).status.success());
}

#[test]
fn a_type_takes_only_the_fields_it_carries() {
    let scratch = Scratch::new();
    let repo = typed_fields(&scratch);
    let refused_with = |args: &[&str], reason: &str| {
        let out = tm(&scratch, &repo, args);
        assert!(!out.status.success(), "{out:?}");
        let error = doc(&out);
        assert_eq!(error["error"], "invalid_value", "{error}");
        assert_eq!(error["field"], "area", "{error}");
        assert_eq!(error["reason"], reason, "{error}");
    };
    let not_carried = "is not a field of type bug";
    refused_with(
        &[
            "--json", "create", "--type", "bug", "--title", "b", "--set", "area=cli",
        ],
        not_carried,
    );
    let created = tm(
        &scratch,
        &repo,
        &[
            "--json",
            "create",
            "--type",
            "bug",
            "--title",
            "b",
            "--set",
            "severity=high",
        ],
    );
    assert_eq!(data(&created)["fields"]["severity"], "high");
    refused_with(
        &["--json", "edit", "TM-1", "--set", "area=cli"],
        not_carried,
    );
    refused_with(
        &["--json", "move", "TM-1", "in_progress", "--set", "area=cli"],
        not_carried,
    );

    let task = tm(
        &scratch,
        &repo,
        &[
            "--json", "create", "--type", "task", "--title", "t", "--set", "area=cli",
        ],
    );
    assert!(task.status.success(), "{task:?}");
    refused_with(
        &["--json", "edit", "TM-2", "--set", "type=bug"],
        "holds a value, and type bug does not carry it; clear it in the same change",
    );
    let changed = tm(
        &scratch,
        &repo,
        &[
            "--json", "edit", "TM-2", "--set", "type=bug", "--unset", "area",
        ],
    );
    assert_eq!(data(&changed)["changed"][0]["after"], "bug", "{changed:?}");
}

/// A repo holding epic TM-1, task TM-2 and story TM-3 under TM-2, whose
/// Workspace Scheme then gives an epic no parent and a story only an epic.
fn epics_hold_stories(scratch: &Scratch) -> PathBuf {
    let (repo, store, _) = initialized(scratch);
    drop(store);
    for (issue_type, parent) in [("epic", None), ("task", None), ("story", Some("TM-2"))] {
        let mut args = vec!["create", "--type", issue_type, "--title", issue_type];
        args.extend(
            parent
                .map(|parent| ["--parent", parent])
                .into_iter()
                .flatten(),
        );
        assert!(tm(scratch, &repo, &args).status.success(), "{args:?}");
    }
    fs::write(
        workspace_dir(scratch, &repo).join("scheme.toml"),
        r#"
[types.epic]
label = "Epic"
workflow = "default"
parents = []

[types.story]
label = "Story"
workflow = "default"
parents = ["epic"]

[types.chore]
workflow = "default"
fields = []
parents = ["epic", "task"]
"#,
    )
    .unwrap();
    repo
}

#[test]
fn a_type_takes_only_the_parents_it_lists() {
    let scratch = Scratch::new();
    let repo = epics_hold_stories(&scratch);
    let run = |args: &[&str]| tm(&scratch, &repo, args);
    let text = String::from_utf8_lossy(&run(&["scheme", "show"]).stdout).into_owned();
    assert!(
        text.contains("type epic (Epic) [from the Workspace's Scheme]: takes no parent\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "type story (Story) [from the Workspace's Scheme]: takes a parent of type epic\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "type chore (chore) [from the Workspace's Scheme]: no Scheme fields; takes a parent of type epic or task\n"
        ),
        "{text}"
    );
    let json = run(&["--json", "scheme", "show", "story"]);
    assert_eq!(data(&json)["type"]["parents"], json!(["epic"]), "{json:?}");

    assert!(
        run(&["edit", "TM-3", "--set", "title=kept"])
            .status
            .success(),
        "a pair the Scheme now refuses stays editable"
    );
    let refused = |args: &[&str], message: &str| {
        let out = run(args);
        assert!(!out.status.success(), "{args:?}: {out:?}");
        let error = doc(&out);
        assert_eq!(error["error"], "parent_type", "{error}");
        assert_eq!(error["message"], message, "{error}");
        error
    };
    let error = refused(
        &[
            "--json", "create", "--type", "story", "--title", "s", "--parent", "TM-2",
        ],
        "the new Issue has type story, which takes a parent of type epic, so TM-2 of type task cannot be its parent",
    );
    assert_eq!(error["key"], json!(null), "{error}");
    assert_eq!(error["allowed"], json!(["epic"]), "{error}");
    assert_eq!(error["parent_key"], "TM-2", "{error}");
    refused(
        &["--json", "edit", "TM-1", "--parent", "TM-2"],
        "TM-1 has type epic, which takes no parent, so TM-2 of type task cannot be its parent",
    );
    assert!(run(&["edit", "TM-3", "--parent", "TM-1"]).status.success());
    refused(
        &["--json", "edit", "TM-1", "--set", "type=task"],
        "TM-3 has type story, which takes a parent of type epic, so TM-1 of type task cannot be its parent",
    );
    assert!(run(&["edit", "TM-2", "--parent", "TM-3"]).status.success());
    refused(
        &["--json", "edit", "TM-2", "--set", "type=story"],
        "TM-2 has type story, which takes a parent of type epic, so TM-3 of type story cannot be its parent",
    );

    let blank_stdin = |args: &[&str]| {
        let args = [&["--json"], args, &["--set", "description=-"]].concat();
        doc(&tm_piped(&scratch, &repo, &args, " \n"))["error"].clone()
    };
    assert_eq!(
        blank_stdin(&[
            "create", "--type", "story", "--title", "s", "--parent", "TM-2"
        ]),
        "parent_type",
        "refused before the piped text is read"
    );
    assert_eq!(
        blank_stdin(&["edit", "TM-2", "--set", "type=story"]),
        "parent_type"
    );
    assert!(
        run(&["create", "--type", "task", "--title", "t"])
            .status
            .success()
    );
    assert_eq!(
        blank_stdin(&["edit", "TM-1", "--parent", "TM-4"]),
        "parent_type"
    );
    assert_eq!(
        blank_stdin(&["edit", "TM-1", "--set", "type=task"]),
        "parent_type",
        "judged against the type being written, not the stored one"
    );
}

#[test]
fn a_pair_the_scheme_now_refuses_is_repaired_by_retyping_its_parent() {
    let scratch = Scratch::new();
    let repo = epics_hold_stories(&scratch);
    let run = |args: &[&str]| tm(&scratch, &repo, args).status.success();
    assert!(
        run(&["edit", "TM-3", "--set", "type=story"]),
        "the type it has"
    );
    assert!(
        run(&["edit", "TM-2", "--parent", "TM-1"]),
        "a new parent leaves its children unchecked"
    );
    assert!(run(&["edit", "TM-2", "--no-parent"]));
    assert!(run(&["edit", "TM-2", "--set", "type=epic"]));
    assert!(!run(&["edit", "TM-2", "--set", "type=task"]));
}

#[test]
fn a_custom_type_follows_its_workflow_and_survives_its_removal() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    fs::write(
        workspace_dir(&scratch, &repo).join("scheme.toml"),
        r#"
[workflows.incident]
start = "open"
statuses = [
  { id = "open", label = "Open", category = "todo" },
  { id = "fixing", label = "Fixing", category = "active" },
  { id = "resolved", label = "Resolved", category = "done" },
]

[types.incident]
label = "Incident"
workflow = "incident"

[types.alpha]
label = "Zeta"
workflow = "incident"

[types.zeta]
workflow = "incident"
"#,
    )
    .unwrap();
    let zeta = tm(&scratch, &repo, &["--json", "scheme", "show", "zeta"]);
    assert_eq!(
        data(&zeta)["type"]["id"],
        "zeta",
        "an id wins over another type's label"
    );

    let created = tm(
        &scratch,
        &repo,
        &[
            "--json", "create", "--type", "incident", "--title", "outage",
        ],
    );
    assert_eq!(data(&created)["fields"]["status"], "open", "{created:?}");
    let moved = tm(&scratch, &repo, &["--json", "move", "TM-1", "fixing"]);
    assert!(moved.status.success(), "{moved:?}");
    let active = tm(&scratch, &repo, &["--json", "list", "category:active"]);
    assert_eq!(data(&active)["total"], 1, "{active:?}");

    let shown = tm(&scratch, &repo, &["scheme", "show"]);
    let text = String::from_utf8_lossy(&shown.stdout);
    assert!(
        text.contains("type incident (Incident) [from the Workspace's Scheme]\n"),
        "{text}"
    );
    assert!(
        text.contains("workflow incident, starting in open [from the Workspace's Scheme]\n"),
        "{text}"
    );

    let shown = tm(&scratch, &repo, &["--json", "scheme", "show"]);
    let from = |list: &str, key: &str, id: &str| {
        data(&shown)[list]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry[key] == id)
            .unwrap()["from"]
            .clone()
    };
    assert_eq!(from("types", "id", "incident"), "workspace");
    assert_eq!(from("types", "id", "task"), "built_in");
    assert_eq!(from("workflows", "name", "incident"), "workspace");
    assert_eq!(from("workflows", "name", "default"), "built_in");
    let primed = tm(&scratch, &repo, &["prime"]);
    let primed = String::from_utf8_lossy(&primed.stdout);
    assert!(
        primed.contains(
            "  statuses of incident (alpha, incident, zeta): open (todo), fixing (active), resolved (done)\n"
        ),
        "{primed}"
    );
    assert!(
        primed
            .contains("  statuses of default (epic, story, task, bug, sub_task, spike, design): "),
        "{primed}"
    );
    let incident = tm(&scratch, &repo, &["--json", "scheme", "show", "incident"]);
    assert_eq!(data(&incident)["type"]["from"], "workspace");
    assert_eq!(data(&incident)["workflow"]["from"], "workspace");

    repo_scheme(&repo, "[projects.TM.remove]\ntypes = [\"incident\"]\n");
    let listed = tm(&scratch, &repo, &["--json", "list", "category:triage"]);
    assert_eq!(
        data(&listed)["total"],
        1,
        "an Issue of a removed type reads as Triage: {listed:?}"
    );
    let edited = tm(
        &scratch,
        &repo,
        &["edit", "TM-1", "--set", "title=still here"],
    );
    assert!(edited.status.success(), "{edited:?}");
    let refused = tm(&scratch, &repo, &["--json", "move", "TM-1", "resolved"]);
    assert!(!refused.status.success(), "{refused:?}");
}

#[test]
fn scheme_show_stops_at_a_broken_repo_file_rather_than_show_the_built_in_scheme() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    fs::write(repo.join(".config/tm.toml"), "not toml at all").unwrap();
    let shown = tm(&scratch, &repo, &["--json", "scheme", "show"]);
    assert!(!shown.status.success(), "{shown:?}");
    let shown = doc(&shown);
    assert_eq!(shown["error"], "invalid_repo_file", "{shown}");
    assert_eq!(shown["warnings"], json!([]), "{shown}");
}

#[test]
fn scheme_show_escapes_a_label_that_would_drive_the_terminal() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    repo_scheme(
        &repo,
        "[projects.TM.fields.tier]\nlabel = \"Tier\\u001b[2J\"\ntype = \"single_select\"\noptions = [{ id = \"gold\", label = \"Gold\\u202e\" }]\n",
    );
    let shown = tm(&scratch, &repo, &["scheme", "show"]);
    let text = String::from_utf8_lossy(&shown.stdout);
    assert!(
        text.contains(
            "field tier (Tier\\u{1b}[2J) [from the repo's Scheme]: gold (Gold\\u{202e})\n"
        ),
        "{text}"
    );
    let json = tm(&scratch, &repo, &["--json", "scheme", "show"]);
    let tier = data(&json)["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["name"] == "tier")
        .unwrap()
        .clone();
    assert_eq!(
        tier["label"], "Tier\u{1b}[2J",
        "JSON keeps the label as written"
    );
}

#[test]
fn a_malformed_scheme_stops_commands_and_tm_doctor_reports_it() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    repo_scheme(&repo, "[projects.TM.fields.area]\ntype = \"colour\"\n");
    let path = fs::canonicalize(&repo)
        .unwrap()
        .join(".config/tm/scheme.toml");

    let listed = tm(&scratch, &repo, &["--json", "list"]);
    assert!(!listed.status.success());
    let error = doc(&listed);
    assert_eq!(error["error"], "invalid_scheme", "{error}");
    assert_eq!(error["path"], path.display().to_string());
    assert_eq!(error["line"], 2);

    let records = repo.join("records.jsonl");
    fs::write(&records, "").unwrap();
    let records = records.display().to_string();
    for args in [
        &["--json", "create", "--type", "task", "--title", "t"][..],
        &["--json", "import", "--records", &records, "--dry-run"],
    ] {
        let refused = tm(&scratch, &repo, args);
        assert_eq!(doc(&refused)["error"], "invalid_scheme", "{args:?}");
    }

    let doctor = tm(&scratch, &repo, &["--json", "doctor"]);
    assert!(doctor.status.success(), "{doctor:?}");
    let scheme = &data(&doctor)["scheme"];
    assert_eq!(scheme["line"], 2, "{scheme}");
    assert_eq!(scheme["path"], path.display().to_string());
    assert!(
        scheme["message"].as_str().unwrap().contains("colour"),
        "{scheme}"
    );
    let text = tm(&scratch, &repo, &["doctor"]);
    assert!(
        String::from_utf8_lossy(&text.stdout).contains("the Scheme cannot be read"),
        "{text:?}"
    );

    fs::remove_file(&path).unwrap();
    let doctor = tm(&scratch, &repo, &["--json", "doctor"]);
    assert_eq!(
        data(&doctor).get("scheme"),
        Some(&json!(null)),
        "no fault, no report"
    );
}

#[test]
fn moves_tm_makes_follow_the_workflow_or_say_why_not() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    fs::write(
        workspace_dir(&scratch, &repo).join("scheme.toml"),
        r#"
[workflows.gated]
start = "todo"
statuses = [
  { id = "todo", category = "todo" },
  { id = "doing", category = "active" },
  { id = "done", category = "done" },
  { id = "dropped", category = "cancelled" },
]
transitions = [{ from = "todo", to = ["dropped"] }]

[types.chore]
workflow = "gated"
"#,
    )
    .unwrap();
    let run = |args: &[&str]| tm(&scratch, &repo, args);
    assert!(
        run(&["create", "--type", "chore", "--title", "c"])
            .status
            .success()
    );
    let shown = run(&["--json", "scheme", "show", "chore"]);
    assert_eq!(
        data(&shown)["workflow"]["transitions"],
        json!([{ "from": "todo", "to": ["dropped"] }])
    );
    let defaults = run(&["--json", "scheme", "show", "task"]);
    assert_eq!(data(&defaults)["workflow"]["transitions"], json!("any"));

    let peeked = run(&["next", "--peek"]);
    let text = String::from_utf8_lossy(&peeked.stdout);
    assert!(!text.contains("TM-1 is next"), "{peeked:?}");
    let ready = run(&["--json", "ready"]);
    assert_eq!(data(&ready)["issues"], json!([]), "{ready:?}");
    assert!(
        run(&["shortlist", "create", "docs", "--actor", "@me"])
            .status
            .success()
    );
    assert!(run(&["shortlist", "add", "docs", "TM-1"]).status.success());
    let next = run(&["next"]);
    let text = String::from_utf8_lossy(&next.stdout);
    assert!(
        text.contains(
            "TM-1 cannot start: its Workflow allows no active Status from todo; tm show TM-1 lists its moves"
        ),
        "{next:?}"
    );
    assert!(run(&["shortlist", "delete", "docs"]).status.success());

    // Refused before stdin is read, which would otherwise be found empty.
    let piped = run(&["--json", "move", "TM-1", "doing", "--set", "description=-"]);
    let error = doc(&piped);
    assert_eq!(error["reason"], "cannot go from todo to doing", "{error}");

    let child = run(&[
        "--json", "create", "--type", "task", "--title", "t", "--parent", "TM-1",
    ]);
    assert!(child.status.success(), "{child:?}");
    let finished = run(&["--json", "move", "TM-2", "done"]);
    let finished = doc(&finished);
    assert!(
        finished["warnings"].to_string().contains(
            "tm cannot suggest closing TM-1: its Workflow allows no done Status from todo; it can move to dropped"
        ),
        "{finished}"
    );

    let message = "fix: c\n\nCloses TM-1";
    crate::support::git(
        &scratch,
        &repo,
        &["commit", "-q", "--allow-empty", "-m", message],
    );
    let scanned = run(&["--json", "git", "scan"]);
    let scanned = doc(&scanned);
    assert_eq!(scanned["data"]["failed"][0]["key"], "TM-1", "{scanned}");
    assert_eq!(
        scanned["data"]["failed"][0]["error"], "invalid_value",
        "{scanned}"
    );
    assert_eq!(
        scanned["data"]["failed"][0]["allowed"],
        json!(["dropped"]),
        "{scanned}"
    );
    assert!(
        scanned["warnings"]
            .to_string()
            .contains("its Workflow allows no done Status from todo; it can move to dropped"),
        "{scanned}"
    );
    let text = String::from_utf8_lossy(&run(&["git", "scan"]).stdout).into_owned();
    assert!(text.contains("1 not moved; the warnings say why"), "{text}");
}

#[test]
fn version_url_and_file_paths_fields_are_set_added_and_found_from_the_cli() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    fs::write(
        workspace_dir(&scratch, &repo).join("scheme.toml"),
        "[fields.release]\ntype = \"version\"\n[fields.docs]\ntype = \"url\"\n[fields.paths]\ntype = \"file_paths\"\n",
    )
    .unwrap();
    let run = |args: &[&str]| tm(&scratch, &repo, args);
    let created = run(&[
        "--json",
        "create",
        "--type",
        "task",
        "--title",
        "t",
        "--set",
        "release=1.2.0",
        "--set",
        "docs=https://example.com/spec",
        "--add",
        "paths=src/a.rs",
        "--add",
        "paths=b.md",
    ]);
    assert!(created.status.success(), "{created:?}");
    let edited = run(&[
        "--json",
        "edit",
        "TM-1",
        "--remove",
        "paths=b.md",
        "--add",
        "paths=c/d.rs",
    ]);
    assert_eq!(
        data(&edited)["changed"],
        json!([{"field": "paths", "before": ["b.md", "src/a.rs"], "after": ["c/d.rs", "src/a.rs"]}]),
        "{edited:?}"
    );
    let shown = String::from_utf8_lossy(&run(&["show", "TM-1"]).stdout).into_owned();
    for line in ["release: 1.2.0", "docs: https://example.com/spec"] {
        assert!(shown.contains(line), "{line}: {shown}");
    }
    assert!(
        shown.contains("c/d.rs") && shown.contains("src/a.rs") && !shown.contains("b.md"),
        "{shown}"
    );
    let found = run(&["--json", "list", "paths:c/d.rs", "release:1.2.0"]);
    assert_eq!(data(&found)["total"], 1, "{found:?}");

    let refused = |args: &[&str], code: &str, message: &str| {
        let out = run(&[&["--json"], args].concat());
        let error = doc(&out);
        assert_eq!(error["error"], code, "{args:?}: {error}");
        assert!(
            error["message"].as_str().unwrap().contains(message),
            "{args:?}: {error}"
        );
    };
    refused(
        &["edit", "TM-1", "--set", "release=v1.2"],
        "invalid_value",
        "is not a semantic version",
    );
    refused(
        &["edit", "TM-1", "--set", "docs=example.com"],
        "invalid_value",
        "is not an http or https URL",
    );
    refused(
        &["edit", "TM-1", "--add", "paths=../x"],
        "invalid_value",
        "is not a path relative to the repo root",
    );
    refused(
        &["edit", "TM-1", "--add", "release=1.0.0"],
        "invalid_value",
        "release holds one value; give it with --set",
    );
    refused(
        &["edit", "TM-1", "--set", "paths=x.rs"],
        "invalid_value",
        "holds several values",
    );
    refused(
        &[
            "edit",
            "TM-1",
            "--add",
            "paths=x.rs",
            "--remove",
            "paths=x.rs",
        ],
        "invalid_value",
        "x.rs is both added and removed",
    );
    refused(&["edit", "TM-1", "--add", "labels=x"], "usage", "--label");
}

#[test]
fn several_valued_fields_refuse_what_would_quietly_do_nothing() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    fs::write(
        workspace_dir(&scratch, &repo).join("scheme.toml"),
        "[fields.release]\ntype = \"version\"\n[fields.paths]\ntype = \"file_paths\"\n[fields.tags]\ntype = \"multi_select\"\noptions = [{ id = \"x\" }, { id = \"y\" }]\n",
    )
    .unwrap();
    let run = |args: &[&str]| tm(&scratch, &repo, &[&["--json"], args].concat());
    let created = run(&[
        "create",
        "--type",
        "task",
        "--title",
        "t",
        "--add",
        "paths=a.rs",
        "--add",
        "tags=x",
    ]);
    assert!(created.status.success(), "{created:?}");

    let unchanged = run(&["edit", "TM-1", "--add", "paths=a.rs", "--add", "tags=x"]);
    assert_eq!(data(&unchanged)["changed"], json!([]), "{unchanged:?}");
    for (args, message) in [
        (
            &["edit", "TM-1", "--remove", "tags=X"][..],
            "tags has no such option; valid: x, y",
        ),
        (
            &["edit", "TM-1", "--unset", "paths"],
            "paths holds several values; take them out with --remove",
        ),
        (
            &[
                "create",
                "--type",
                "task",
                "--title",
                "u",
                "--add",
                "release=1.0.0",
            ],
            "release holds one value; give it with --set",
        ),
        (
            &["edit", "TM-1", "--add", "tags=z"],
            "tags has no such option; valid: x, y",
        ),
    ] {
        let error = doc(&run(args));
        assert_eq!(error["error"], "invalid_value", "{args:?}: {error}");
        assert_eq!(error["message"], message, "{args:?}: {error}");
    }

    fs::write(
        workspace_dir(&scratch, &repo).join("scheme.toml"),
        "[fields.tags]\ntype = \"multi_select\"\noptions = [{ id = \"y\" }]\n",
    )
    .unwrap();
    let removed = run(&["edit", "TM-1", "--remove", "tags=x"]);
    assert_eq!(
        data(&removed)["changed"],
        json!([{"field": "tags", "before": ["x"], "after": []}]),
        "a value the Scheme stopped offering still comes out: {removed:?}"
    );
}
