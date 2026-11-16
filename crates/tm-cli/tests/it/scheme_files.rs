//! The Scheme read from the Workspace's and the repo's Scheme files
//! (ADR 0022).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use tm_store::workspace::{RepoFile, Workspace};

use crate::support::{Scratch, data, doc, initialized, tm};

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
