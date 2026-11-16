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
    assert!(!refused.status.success(), "area is gone from this Project");
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
    let text = tm(&scratch, &repo, &["doctor"]);
    assert!(
        String::from_utf8_lossy(&text.stdout).contains("the Scheme cannot be read"),
        "{text:?}"
    );
}
