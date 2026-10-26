use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use tm_store::Store;
use tm_store::workspace::{RepoFile, Workspace};
use uuid::Uuid;

use crate::support::{Scratch, fixture, git, text, tm, tm_command};

/// Every file and directory under `dir`, `.git` included, with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut out = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(dir).unwrap().to_path_buf();
            if path.is_dir() {
                out.insert(relative, None);
                pending.push(path);
            } else {
                out.insert(relative, Some(fs::read(&path).unwrap()));
            }
        }
    }
    out
}

/// Whether the scratch directory's disk tells names apart by case.
fn case_sensitive(scratch: &Scratch) -> bool {
    let probe = scratch.0.join("case-probe");
    fs::write(&probe, "").unwrap();
    let sensitive = !scratch.0.join("CASE-PROBE").exists();
    fs::remove_file(&probe).unwrap();
    sensitive
}

#[test]
fn init_creates_a_workspace_and_writes_only_the_repo_file() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    fs::create_dir_all(repo.join("src")).unwrap();
    let before = snapshot(&repo);
    let output = tm(
        &scratch,
        &repo.join("src"),
        &["init", "--key", "tm", "--yes"],
    );
    assert!(output.status.success(), "{output:?}");
    let stdout = text(&output.stdout);
    assert!(stdout.contains("+++ b/.config/tm.toml"), "{stdout}");
    assert!(stdout.contains("+project = \"TM\""), "{stdout}");

    let mut after = snapshot(&repo);
    let written = after.remove(Path::new(".config/tm.toml")).unwrap().unwrap();
    assert_eq!(after.remove(Path::new(".config")), Some(None));
    assert_eq!(
        after, before,
        "nothing else in the repo, .git included, changed"
    );
    let status = git(
        &scratch,
        &repo,
        &["status", "--porcelain", "--untracked-files=all"],
    );
    assert_eq!(text(&status.stdout), "?? .config/tm.toml\n");

    let file = RepoFile::parse(&text(&written)).unwrap();
    assert_eq!(file.project, "TM");
    assert!(stdout.contains(&format!("+workspace = \"{}\"", file.workspace)));
    let workspace = Workspace::at(&scratch.home(), file.workspace);
    let store = Store::open_existing(&workspace.store_path(), file.workspace).unwrap();
    let project = store.project_by_key("TM").unwrap();
    assert!(project.is_some());
    let actor: String = store
        .connection()
        .query_row(
            "SELECT actor FROM ops WHERE kind = 'create_project'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(actor, "jace");
}

#[test]
fn init_in_json_mode_reports_what_it_made_and_its_batch() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let made = tm(&scratch, &repo, &["init", "--yes", "--json", "--key", "TM"]);
    assert!(made.status.success(), "{made:?}");
    assert!(made.stderr.is_empty(), "{}", text(&made.stderr));
    let doc: serde_json::Value = serde_json::from_slice(&made.stdout).unwrap();
    let data = &doc["data"];
    assert_eq!(
        (&data["created"], &data["project"]),
        (&true.into(), &"TM".into())
    );
    let workspace: Uuid = data["workspace"].as_str().unwrap().parse().unwrap();
    let batch: Uuid = data["batch"].as_str().unwrap().parse().unwrap();
    let store = Store::open(&Workspace::at(&scratch.home(), workspace).store_path()).unwrap();
    let batched: Vec<u8> = store
        .connection()
        .query_row("SELECT batch FROM op_batches", [], |row| row.get(0))
        .unwrap();
    assert_eq!(Uuid::from_slice(&batched).unwrap(), batch);

    let again = tm(&scratch, &repo, &["init", "--json"]);
    assert!(again.status.success(), "{again:?}");
    let doc: serde_json::Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(doc["data"]["created"], false);
    assert_eq!(doc["data"]["workspace"], workspace.to_string());
}

#[test]
fn an_agent_writes_for_the_human_and_two_harnesses_warn() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let made = tm_command()
        .args(["init", "--yes", "--json", "--key", "TM"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("CLAUDE_CODE_SESSION_ID", "s")
        .env("CODEX_THREAD_ID", "t")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    let doc: serde_json::Value = serde_json::from_slice(&made.stdout).unwrap();
    let warnings = doc["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{doc}");
    assert!(warnings[0].as_str().unwrap().contains("TM_AGENT"), "{doc}");
    let workspace: Uuid = doc["data"]["workspace"].as_str().unwrap().parse().unwrap();
    let store = Store::open(&Workspace::at(&scratch.home(), workspace).store_path()).unwrap();
    let written: (String, Option<String>) = store
        .connection()
        .query_row("SELECT actor, on_behalf_of FROM ops", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(written, ("codex".to_owned(), Some("jace".to_owned())));
}

#[test]
fn without_a_terminal_init_needs_yes_and_writes_nothing() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let before = snapshot(&repo);
    let output = tm(&scratch, &repo, &["init", "--key", "TM"]);
    assert!(!output.status.success());
    assert!(
        text(&output.stdout).contains("+++ b/.config/tm.toml"),
        "the diff is shown"
    );
    assert!(text(&output.stderr).contains("--yes"), "{output:?}");
    assert_eq!(snapshot(&repo), before);
    assert!(!scratch.home().exists(), "no Workspace was made");
}

#[test]
fn init_again_reports_the_mapping_and_changes_nothing() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    assert!(
        tm(&scratch, &repo, &["init", "--key", "TM", "--yes"])
            .status
            .success()
    );
    let repo_before = snapshot(&repo);
    let home_before = snapshot(&scratch.home());
    let output = tm(&scratch, &repo, &["init", "--key", "OTHER", "--yes"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = text(&output.stdout);
    assert!(stdout.contains("already maps"), "{stdout}");
    assert!(stdout.contains("Project TM"), "{stdout}");
    assert!(
        stdout.contains("nothing changed"),
        "the ignored --key is mentioned: {stdout}"
    );
    assert_eq!(snapshot(&repo), repo_before);
    assert_eq!(snapshot(&scratch.home()), home_before);
}

/// Each case fails, before or after the file is confirmed, and must leave
/// neither the repo file nor a Workspace.
#[test]
fn a_failed_init_leaves_no_workspace() {
    let scratch = Scratch::new();
    let workspaces = scratch.home().join("workspaces");
    let count = || fs::read_dir(&workspaces).map_or(0, Iterator::count);

    let repo = fixture(&scratch, "config-file");
    fs::write(repo.join(".config"), "a file\n").unwrap();
    let output = tm(&scratch, &repo, &["init", "--key", "TM", "--yes"]);
    assert!(!output.status.success());
    assert!(
        text(&output.stderr).contains("not a directory"),
        "{output:?}"
    );

    let repo = fixture(&scratch, "blank-name");
    let output = tm(
        &scratch,
        &repo,
        &["init", "--key", "TM", "--name", " ", "--yes"],
    );
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("blank"), "{output:?}");
    assert!(
        !text(&output.stdout).contains("+++"),
        "refused before the file is shown"
    );

    let repo = fixture(&scratch, "locked-config");
    fs::create_dir_all(repo.join(".config")).unwrap();
    let locked = |bits| {
        fs::set_permissions(repo.join(".config"), fs::Permissions::from_mode(bits)).unwrap();
    };
    locked(0o555);
    let output = tm(&scratch, &repo, &["init", "--key", "TM", "--yes"]);
    locked(0o755);
    assert!(!output.status.success());
    assert!(!repo.join(".config/tm.toml").exists());

    assert_eq!(count(), 0, "no Workspace was left behind");
}

#[test]
fn a_config_link_is_refused_wherever_it_leads() {
    let scratch = Scratch::new();
    let elsewhere = scratch.0.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    for (name, target) in [
        ("outside", elsewhere.clone()),
        ("git", PathBuf::from(".git")),
    ] {
        let repo = fixture(&scratch, name);
        std::os::unix::fs::symlink(&target, repo.join(".config")).unwrap();
        let output = tm(&scratch, &repo, &["init", "--key", "TM", "--yes"]);
        assert!(!output.status.success(), "{name}");
        assert!(text(&output.stderr).contains("symlink"), "{output:?}");
        assert!(!repo.join(".git/tm.toml").exists(), "{name}");
    }
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    assert!(!scratch.home().exists());
}

#[test]
fn a_failure_after_confirmation_removes_the_config_it_made() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    // A file where the tm home's workspaces directory should be.
    fs::create_dir_all(scratch.home()).unwrap();
    fs::write(scratch.home().join("workspaces"), "a file\n").unwrap();
    let before = snapshot(&repo);
    let output = tm(&scratch, &repo, &["init", "--key", "TM", "--yes"]);
    assert!(!output.status.success());
    assert_eq!(snapshot(&repo), before, "no empty .config left behind");
}

#[test]
fn a_repo_file_git_would_ignore_is_refused() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    fs::write(repo.join(".gitignore"), "target/\n.config/\n").unwrap();
    git(
        &scratch,
        &repo,
        &["commit", "--quiet", "-am", "ignore .config"],
    );
    let before = snapshot(&repo);
    // The bad key shows the repo is checked before the key, so a person at a
    // terminal is refused before being asked for one.
    let output = tm(&scratch, &repo, &["init", "--key", "1x", "--yes"]);
    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(stderr.contains("git ignores"), "{stderr}");
    assert!(stderr.contains(".gitignore:2"), "names the rule: {stderr}");
    assert_eq!(snapshot(&repo), before);
    assert!(!scratch.home().exists());
}

#[test]
fn a_repo_file_deleted_but_still_tracked_can_be_written_again() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    assert!(
        tm(&scratch, &repo, &["init", "--key", "TM", "--yes"])
            .status
            .success()
    );
    git(&scratch, &repo, &["add", ".config/tm.toml"]);
    git(
        &scratch,
        &repo,
        &["commit", "--quiet", "-m", "map the repo"],
    );
    fs::remove_file(repo.join(".config/tm.toml")).unwrap();
    let output = tm(&scratch, &repo, &["init", "--key", "TM", "--yes"]);
    assert!(output.status.success(), "{output:?}");
    assert!(repo.join(".config/tm.toml").exists());
}

#[test]
fn a_repo_file_tracked_in_another_case_is_refused_and_cleaned_up() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    fs::create_dir(repo.join(".Config")).unwrap();
    fs::write(repo.join(".Config/tm.toml"), "old\n").unwrap();
    git(&scratch, &repo, &["add", ".Config/tm.toml"]);
    git(&scratch, &repo, &["commit", "--quiet", "-m", "old mapping"]);
    fs::remove_file(repo.join(".Config/tm.toml")).unwrap();
    fs::remove_dir(repo.join(".Config")).unwrap();
    let before = snapshot(&repo);
    let output = tm(&scratch, &repo, &["init", "--key", "TM", "--yes"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        text(&output.stderr).contains(".Config/tm.toml"),
        "{output:?}"
    );
    assert_eq!(snapshot(&repo), before);
    let workspaces = scratch.home().join("workspaces");
    assert_eq!(fs::read_dir(&workspaces).map_or(0, Iterator::count), 0);
}

#[test]
fn an_existing_mapping_git_ignores_is_reported() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    assert!(
        tm(&scratch, &repo, &["init", "--key", "TM", "--yes"])
            .status
            .success()
    );
    fs::write(repo.join(".gitignore"), "target/\n.config/\n").unwrap();
    let output = tm(&scratch, &repo, &["init", "--yes"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(text(&output.stderr).contains("does not see"), "{output:?}");
}

#[test]
fn a_rule_letting_the_file_through_is_not_blamed() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    fs::write(repo.join(".gitignore"), ".config/*\n!.config/tm.toml\n").unwrap();
    git(
        &scratch,
        &repo,
        &["commit", "--quiet", "-am", "ignore most of .config"],
    );
    let output = tm(&scratch, &repo, &["init", "--key", "TM", "--yes"]);
    assert!(output.status.success(), "{output:?}");
    let status = git(
        &scratch,
        &repo,
        &["status", "--porcelain", "--untracked-files=all"],
    );
    assert_eq!(text(&status.stdout), "?? .config/tm.toml\n");
}

#[test]
fn a_config_that_is_its_own_repo_is_refused_and_cleaned_up() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    fs::create_dir(repo.join(".config")).unwrap();
    git(&scratch, &repo.join(".config"), &["init", "--quiet"]);
    let output = tm(&scratch, &repo, &["init", "--key", "TM", "--yes"]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("does not see"), "{output:?}");
    assert!(!repo.join(".config/tm.toml").exists());
    let workspaces = scratch.home().join("workspaces");
    assert_eq!(fs::read_dir(&workspaces).map_or(0, Iterator::count), 0);
}

#[test]
fn a_repo_file_spelled_in_another_case_is_reported() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    assert!(
        tm(&scratch, &repo, &["init", "--key", "TM", "--yes"])
            .status
            .success()
    );
    fs::rename(repo.join(".config/tm.toml"), repo.join(".config/TM.toml")).unwrap();
    let output = tm(&scratch, &repo, &["init", "--key", "TM", "--yes"]);
    if case_sensitive(&scratch) {
        // TM.toml is not a repo file there, so init starts afresh beside it.
        assert!(output.status.success(), "{output:?}");
        assert!(
            text(&output.stdout).contains("Created Workspace"),
            "{output:?}"
        );
        assert!(repo.join(".config/tm.toml").exists());
        assert!(repo.join(".config/TM.toml").exists());
        return;
    }
    assert!(!output.status.success());
    assert!(
        text(&output.stderr).contains("spelled TM.toml"),
        "{output:?}"
    );
}

#[test]
fn a_config_spelled_in_another_case_is_refused() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    fs::create_dir(repo.join(".Config")).unwrap();
    let output = tm(&scratch, &repo, &["init", "--key", "TM", "--yes"]);
    if case_sensitive(&scratch) {
        // .Config is another directory there, and .config is made beside it.
        assert!(output.status.success(), "{output:?}");
        assert!(repo.join(".config/tm.toml").exists());
        assert_eq!(fs::read_dir(repo.join(".Config")).unwrap().count(), 0);
        return;
    }
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("another case"), "{output:?}");
    assert_eq!(fs::read_dir(repo.join(".Config")).unwrap().count(), 0);
    assert!(!scratch.home().exists());
}

#[test]
fn a_mapping_to_a_missing_project_is_reported() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    assert!(
        tm(&scratch, &repo, &["init", "--key", "TM", "--yes"])
            .status
            .success()
    );
    let path = repo.join(".config/tm.toml");
    let mut file = RepoFile::parse(&fs::read_to_string(&path).unwrap()).unwrap();
    file.project = "GONE".into();
    fs::write(&path, file.render()).unwrap();
    let output = tm(&scratch, &repo, &["init", "--yes"]);
    assert!(!output.status.success());
    assert!(
        text(&output.stderr).contains("names Project GONE"),
        "{output:?}"
    );
}

#[test]
fn a_failure_never_removes_a_config_this_run_did_not_make() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    fs::create_dir(repo.join(".config")).unwrap();
    fs::create_dir_all(scratch.home()).unwrap();
    fs::write(scratch.home().join("workspaces"), "a file\n").unwrap();
    let output = tm(&scratch, &repo, &["init", "--key", "TM", "--yes"]);
    assert!(!output.status.success());
    assert!(repo.join(".config").is_dir(), "the existing .config stays");
}

#[test]
fn a_git_warning_is_passed_on() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let output = tm_command()
        .args(["init", "--key", "TM", "--yes"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env_remove("TM_ACTOR")
        .env("GIT_CONFIG_GLOBAL", &scratch.0)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("git said"), "{output:?}");
}

#[test]
fn a_set_but_unusable_tm_actor_is_refused() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let output = tm_command()
        .args(["init", "--key", "TM", "--yes"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "  ")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "user.email")
        .env("GIT_CONFIG_VALUE_0", "someone@example.com")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "a blank TM_ACTOR must not fall back to git"
    );
    assert!(text(&output.stderr).contains("TM_ACTOR"), "{output:?}");
    assert!(!scratch.home().exists());
}

#[test]
fn a_broken_git_config_is_reported_as_itself() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let broken = scratch.0.join("broken-gitconfig");
    fs::write(
        &broken,
        "[user]\n\temail = me@example.com\nthis is not config\n",
    )
    .unwrap();
    let output = tm_command()
        .args(["init", "--key", "TM", "--yes"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env_remove("TM_ACTOR")
        .env("GIT_CONFIG_GLOBAL", &broken)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(stderr.contains("failed"), "{stderr}");
    assert!(!stderr.contains("set TM_ACTOR"), "{stderr}");
}

#[test]
fn a_repo_file_naming_a_missing_workspace_is_reported() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let id = Uuid::now_v7();
    let file = RepoFile {
        workspace: id,
        project: "TM".into(),
        paths: Vec::new(),
        claim_expiry: None,
    };
    fs::create_dir_all(repo.join(".config")).unwrap();
    fs::write(repo.join(".config/tm.toml"), file.render()).unwrap();
    git(&scratch, &repo, &["add", ".config/tm.toml"]);
    git(&scratch, &repo, &["commit", "--quiet", "-m", "map"]);
    let output = tm(&scratch, &repo, &["init", "--yes"]);
    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(stderr.contains(&id.to_string()), "{output:?}");
    assert!(
        stderr.contains(
            "; if nobody has an export, a person may start a new Workspace instead: git rm -q :/.config/tm.toml, tm init, git add :/.config/tm.toml, then commit"
        ),
        "says how to start over: {stderr}"
    );
    let deep = repo.join("src/deep");
    fs::create_dir_all(&deep).unwrap();
    let output = tm(&scratch, &deep, &["--json", "init", "--yes"]);
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["error"], "unknown_workspace", "{output:?}");
    assert_eq!(doc["fix"], "tm import FILE");
    let steps = serde_json::json!([
        "git rm -q :/.config/tm.toml",
        "tm init",
        "git add :/.config/tm.toml"
    ]);
    assert_eq!(doc["start_over"], steps);
    assert!(
        !scratch.home().exists(),
        "a fresh clone gets no new Workspace"
    );
    // The first step works as printed from where init ran.
    git(&scratch, &deep, &["rm", "-q", ":/.config/tm.toml"]);
    assert!(!repo.join(".config/tm.toml").exists());

    // A mapping kept in a subproject is named from the repo root.
    let sub = repo.join("svc/a");
    fs::create_dir_all(sub.join(".config")).unwrap();
    fs::write(sub.join(".config/tm.toml"), file.render()).unwrap();
    git(&scratch, &repo, &["add", "svc/a/.config/tm.toml"]);
    git(&scratch, &repo, &["commit", "--quiet", "-m", "map svc/a"]);
    let output = tm(&scratch, &sub, &["--json", "init", "--yes"]);
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        doc["start_over"][0], "git rm -q :/svc/a/.config/tm.toml",
        "{output:?}"
    );
    git(&scratch, &sub, &["rm", "-q", ":/svc/a/.config/tm.toml"]);
    assert!(!sub.join(".config/tm.toml").exists());
}

#[test]
fn a_repo_with_beads_is_told_how_to_bring_them_in() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    fs::create_dir(repo.join(".beads")).unwrap();
    let output = tm(&scratch, &repo, &["init", "--key", "TM", "--yes"]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        text(&output.stdout).contains(
            "To bring beads' Issues into tm, run:\n  bd export -o beads.jsonl\n  tm import --from beads beads.jsonl\n"
        ),
        "{output:?}"
    );

    let repo = fixture(&scratch, "json");
    fs::create_dir(repo.join(".beads")).unwrap();
    let made = tm(&scratch, &repo, &["--json", "init", "--key", "JS", "--yes"]);
    assert!(made.status.success(), "{made:?}");
    let doc: serde_json::Value = serde_json::from_slice(&made.stdout).unwrap();
    assert_eq!(
        doc["data"]["beads"],
        serde_json::json!({
            "state": "to_import",
            "file": null,
            "commands": ["bd export -o beads.jsonl", "tm import --from beads beads.jsonl"],
        })
    );
    let listed = tm(&scratch, &repo, &["--json", "list"]);
    let listed: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed["data"]["total"], 0, "nothing is imported unasked");

    let repo = fixture(&scratch, "plain");
    fs::write(repo.join(".beads"), "not a directory").unwrap();
    let made = tm(&scratch, &repo, &["--json", "init", "--key", "PL", "--yes"]);
    assert!(made.status.success(), "{made:?}");
    let doc: serde_json::Value = serde_json::from_slice(&made.stdout).unwrap();
    assert_eq!(doc["data"].get("beads"), None, "only a directory counts");
}

#[test]
fn the_key_defaults_to_the_directory_names_initials() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "my-repo");
    let output = tm(&scratch, &repo, &["init", "--yes"]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        text(&output.stdout).contains("+project = \"MR\""),
        "{output:?}"
    );

    let repo = fixture(&scratch, "2fa-tool");
    let output = tm(&scratch, &repo, &["init", "--yes"]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("--key"), "{output:?}");
    assert!(!repo.join(".config").exists());
}

#[test]
fn the_actor_falls_back_to_git_and_is_required() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let run = |extra: &[(&str, &str)]| {
        let mut command = tm_command();
        command
            .args(["init", "--key", "TM", "--yes"])
            .current_dir(&repo)
            .env("TM_HOME", scratch.home())
            .env_remove("TM_ACTOR")
            .env("GIT_CONFIG_GLOBAL", scratch.git_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null());
        for (key, value) in extra {
            command.env(key, value);
        }
        command.output().unwrap()
    };
    let output = run(&[]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("TM_ACTOR"), "{output:?}");
    assert!(!repo.join(".config").exists());
    let output = run(&[("TM_JSON", "1")]);
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["error"], "no_identity", "{doc}");

    let output = run(&[
        ("GIT_CONFIG_COUNT", "1"),
        ("GIT_CONFIG_KEY_0", "user.email"),
        ("GIT_CONFIG_VALUE_0", "someone@example.com"),
    ]);
    assert!(output.status.success(), "{output:?}");
    let file = RepoFile::parse(&fs::read_to_string(repo.join(".config/tm.toml")).unwrap()).unwrap();
    let store = Store::open_existing(
        &Workspace::at(&scratch.home(), file.workspace).store_path(),
        file.workspace,
    )
    .unwrap();
    let actor: String = store
        .connection()
        .query_row(
            "SELECT actor FROM ops WHERE kind = 'create_project'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(actor, "someone@example.com");
}

#[test]
fn init_outside_a_repo_is_refused() {
    let outside = std::env::temp_dir().join(format!("tm-cli-outside-{}", Uuid::now_v7()));
    fs::create_dir_all(&outside).unwrap();
    let outside = fs::canonicalize(&outside).unwrap();
    if tm_store::workspace::repo_root(&outside).unwrap().is_some() {
        fs::remove_dir_all(&outside).unwrap();
        return;
    }
    let scratch = Scratch::new();
    let output = tm(&scratch, &outside, &["init", "--key", "TM", "--yes"]);
    let left = fs::read_dir(&outside).unwrap().count();
    fs::remove_dir_all(&outside).unwrap();
    assert!(!output.status.success());
    assert!(
        text(&output.stderr).contains("git repository"),
        "{output:?}"
    );
    assert_eq!(left, 0, "nothing was written");
    assert!(!scratch.home().exists());
}

#[test]
fn init_below_a_home_repo_is_refused_but_runs_at_home() {
    let scratch = Scratch::new();
    let home = fixture(&scratch, "home");
    let project = home.join("code/new-thing");
    fs::create_dir_all(&project).unwrap();
    let linked = scratch.0.join("home-link");
    std::os::unix::fs::symlink(&home, &linked).unwrap();
    let before = snapshot(&home);
    let init = |dir: &Path, user_home: &Path| {
        tm_command()
            .args(["init", "--key", "TM", "--yes"])
            .current_dir(dir)
            .env("HOME", user_home)
            .env("TM_HOME", scratch.home())
            .env("TM_ACTOR", "jace")
            .env("GIT_CONFIG_GLOBAL", scratch.git_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    for user_home in [&home, &linked] {
        let output = init(&project, user_home);
        assert!(!output.status.success());
        let stderr = text(&output.stderr);
        assert!(stderr.contains("git init here"), "{stderr}");
        assert!(
            stderr.starts_with(&format!(
                "tm: {} is inside the git repository at your home directory {};",
                project.display(),
                home.display()
            )),
            "{stderr}"
        );
    }
    assert_eq!(snapshot(&home), before);
    assert!(!scratch.home().exists(), "no Workspace was made");

    let nested = fixture(&scratch, "home/code/other");
    let output = init(&nested, &home);
    assert!(output.status.success(), "{output:?}");
    assert!(nested.join(".config/tm.toml").exists());

    let output = init(&project, Path::new("../.."));
    assert!(!output.status.success());
    assert!(
        text(&output.stderr).contains("must be an absolute path"),
        "{output:?}"
    );
    fs::write(scratch.0.join("a-file"), "").unwrap();
    for missing in [scratch.0.join("missing"), scratch.0.join("a-file/sub")] {
        let output = init(&project, &missing);
        assert!(
            output.status.success(),
            "a missing $HOME is not this repo: {output:?}"
        );
        fs::remove_dir_all(home.join(".config")).unwrap();
    }

    let output = init(&home, &home);
    assert!(output.status.success(), "{output:?}");
    assert!(home.join(".config/tm.toml").exists());
    let output = init(&project, &home);
    assert!(output.status.success(), "{output:?}");
    assert!(
        text(&output.stdout).contains("already maps"),
        "a mapped $HOME is reported, not refused"
    );
}

#[test]
fn a_home_that_cannot_be_resolved_stops_init() {
    let scratch = Scratch::new();
    let home = fixture(&scratch, "home");
    let project = home.join("code/new-thing");
    fs::create_dir_all(&project).unwrap();
    let locked = scratch.0.join("locked");
    fs::create_dir(&locked).unwrap();
    std::os::unix::fs::symlink(&home, locked.join("link")).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let output = tm_command()
        .args(["init", "--key", "TM", "--yes"])
        .current_dir(&project)
        .env("HOME", locked.join("link"))
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(
        stderr.starts_with(&format!(
            "tm: the home directory {}: ",
            locked.join("link").display()
        )),
        "{stderr}"
    );
    assert!(stderr.contains("set HOME"), "{stderr}");
    assert!(!home.join(".config").exists());
}

#[test]
fn concurrent_inits_write_the_file_once() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let children: Vec<_> = (0..8)
        .map(|_| {
            tm_command()
                .args(["init", "--key", "TM", "--yes"])
                .current_dir(&repo)
                .env("TM_HOME", scratch.home())
                .env("TM_ACTOR", "jace")
                .env("GIT_CONFIG_GLOBAL", scratch.git_config())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let outputs: Vec<Output> = children
        .into_iter()
        .map(|child| child.wait_with_output().unwrap())
        .collect();
    let created: Vec<&Output> = outputs
        .iter()
        .filter(|output| output.status.success() && text(&output.stdout).contains("Created"))
        .collect();
    assert_eq!(created.len(), 1, "{outputs:?}");
    let file = RepoFile::parse(&fs::read_to_string(repo.join(".config/tm.toml")).unwrap()).unwrap();
    assert!(
        text(&created[0].stdout).contains(&file.workspace.to_string()),
        "the file names the Workspace its writer reported"
    );
}

#[test]
fn an_invalid_repo_file_stops_init_untouched() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    fs::create_dir_all(repo.join(".config")).unwrap();
    fs::write(repo.join(".tm.toml"), "not toml at all").unwrap();
    let output = tm(&scratch, &repo, &["init", "--key", "TM", "--yes"]);
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(repo.join(".tm.toml")).unwrap(),
        "not toml at all"
    );
    assert!(!repo.join(".config/tm.toml").exists());
}

const START: &str = "<!-- tm:start -->";
const LINE: &str = "This repo tracks its work in tm. Run `tm guide` for how to work with it.";
const END: &str = "<!-- tm:end -->";

/// `tm init` as the Agent `claude` writing for `jace`.
fn init_as_agent(scratch: &Scratch, repo: &Path, args: &[&str]) -> Output {
    tm_command()
        .args([&["init"], args].concat())
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "claude")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn an_agent_is_given_the_lines_and_hook_and_writes_neither() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    for file in ["AGENTS.md", "CLAUDE.md"] {
        fs::remove_file(repo.join(file)).unwrap();
    }
    let made = init_as_agent(&scratch, &repo, &["--key", "TM", "--yes"]);
    assert!(made.status.success(), "{made:?}");
    let shown = text(&made.stdout);
    assert!(
        shown.contains(&format!(
            "Create AGENTS.md with these lines:\n{START}\n{LINE}\n{END}\n"
        )),
        "{shown}"
    );
    assert!(
        shown.contains(
            "a person runs tm setup claude, which adds tm's hooks to .claude/settings.json"
        ),
        "{shown}"
    );
    assert!(
        shown.ends_with("Commit .config/tm.toml and AGENTS.md\n"),
        "{shown}"
    );
    assert!(!repo.join("AGENTS.md").exists());
    assert!(!repo.join(".claude").exists());
}

#[test]
fn json_init_names_the_file_lines_hook_and_what_to_commit() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    fs::remove_file(repo.join("AGENTS.md")).unwrap();
    fs::write(repo.join("CLAUDE.md"), "# Repo\n").unwrap();
    let made = tm(&scratch, &repo, &["init", "--yes", "--json", "--key", "TM"]);
    assert!(made.status.success(), "{made:?}");
    let doc: serde_json::Value = serde_json::from_slice(&made.stdout).unwrap();
    let data = &doc["data"];
    assert_eq!(
        data["instructions"],
        serde_json::json!({
            "file": "CLAUDE.md",
            "create": false,
            "replace": false,
            "lines": format!("{START}\n{LINE}\n{END}\n"),
            "state": "to_add",
        })
    );
    assert_eq!(
        data["hook"],
        serde_json::json!({ "file": ".claude/settings.json", "setup": "tm setup claude" })
    );
    assert_eq!(
        data["commit"],
        serde_json::json!([".config/tm.toml", "CLAUDE.md"])
    );
    assert_eq!(
        fs::read_to_string(repo.join("CLAUDE.md")).unwrap(),
        "# Repo\n"
    );
}

#[test]
fn a_rerun_with_the_lines_and_hook_in_place_has_nothing_to_offer() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    assert!(
        tm(&scratch, &repo, &["init", "--key", "TM", "--yes"])
            .status
            .success()
    );
    let lines = format!("# Repo\n\n{START}\n{LINE}\n{END}\n");
    fs::write(repo.join("AGENTS.md"), &lines).unwrap();
    fs::write(repo.join("CLAUDE.md"), "@AGENTS.md\n").unwrap();
    assert!(
        tm(&scratch, &repo, &["setup", "claude", "--yes"])
            .status
            .success()
    );

    let again = init_as_agent(&scratch, &repo, &["--json"]);
    let doc: serde_json::Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(doc["data"]["instructions"]["file"], "AGENTS.md");
    assert_eq!(doc["data"]["instructions"]["state"], "present");
    assert_eq!(doc["data"]["hook"], serde_json::Value::Null);
    assert_eq!(doc["data"]["commit"], serde_json::json!([]));
    let shown = text(&init_as_agent(&scratch, &repo, &[]).stdout);
    assert!(
        !shown.contains("these lines") && !shown.contains("Commit"),
        "{shown}"
    );
    assert_eq!(fs::read_to_string(repo.join("AGENTS.md")).unwrap(), lines);
}

#[test]
fn old_or_broken_lines_are_offered_again_and_never_written_for_an_agent() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    assert!(
        tm(&scratch, &repo, &["init", "--key", "TM", "--yes"])
            .status
            .success()
    );
    for (contents, state) in [
        (
            format!("# Repo\n{START}\nan old pointer\n{END}\n"),
            "to_replace",
        ),
        (format!("# Repo\n{END}\n{START}\n"), "malformed"),
    ] {
        fs::write(repo.join("AGENTS.md"), &contents).unwrap();
        let again = init_as_agent(&scratch, &repo, &["--json"]);
        let doc: serde_json::Value = serde_json::from_slice(&again.stdout).unwrap();
        assert_eq!(doc["data"]["instructions"]["state"], state, "{doc}");
        assert_eq!(
            fs::read_to_string(repo.join("AGENTS.md")).unwrap(),
            contents
        );
    }
    let shown = text(&init_as_agent(&scratch, &repo, &[]).stdout);
    assert!(shown.contains("so it was left alone"), "{shown}");
}

#[test]
fn an_agent_needs_yes_and_is_told_to_add_lines_to_an_existing_file() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let refused = init_as_agent(&scratch, &repo, &["--key", "TM"]);
    assert!(!refused.status.success());
    assert!(text(&refused.stderr).contains("--yes"), "{refused:?}");
    assert!(!repo.join(".config/tm.toml").exists());

    fs::remove_file(repo.join("AGENTS.md")).unwrap();
    let made = init_as_agent(&scratch, &repo, &["--key", "TM", "--yes"]);
    assert!(
        text(&made.stdout).contains(&format!(
            "Add these lines to CLAUDE.md:\n{START}\n{LINE}\n{END}\n"
        )),
        "{made:?}"
    );
    assert_eq!(
        fs::read_to_string(repo.join("CLAUDE.md")).unwrap(),
        "claude\n"
    );
}
