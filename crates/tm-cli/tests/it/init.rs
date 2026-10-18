use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tm_store::Store;
use tm_store::workspace::{RepoFile, Workspace};
use uuid::Uuid;

/// A scratch directory outside any other repo's reach: each fixture repo has
/// its own `.git`, so discovery stops there.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("tm-cli-{}", Uuid::now_v7()));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(&path).unwrap())
    }

    fn home(&self) -> PathBuf {
        self.0.join("tm-home")
    }

    /// Empty, so no user or system git config reaches the fixture.
    fn git_config(&self) -> PathBuf {
        let path = self.0.join("gitconfig");
        if !path.exists() {
            fs::write(&path, "").unwrap();
        }
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(scratch: &Scratch, dir: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
            // Background maintenance after a commit writes into .git while
            // the test compares it byte for byte.
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    output
}

/// A committed repo holding the files tm must never touch.
fn fixture(scratch: &Scratch, name: &str) -> PathBuf {
    let repo = scratch.0.join(name);
    fs::create_dir_all(&repo).unwrap();
    git(scratch, &repo, &["init", "--quiet"]);
    for (file, text) in [
        ("README.md", "# fixture\n"),
        ("AGENTS.md", "agents\n"),
        ("CLAUDE.md", "claude\n"),
        (".gitignore", "target/\n"),
    ] {
        fs::write(repo.join(file), text).unwrap();
    }
    git(scratch, &repo, &["add", "--all"]);
    git(scratch, &repo, &["commit", "--quiet", "-m", "fixture"]);
    repo
}

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

fn tm(scratch: &Scratch, dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tm"))
        .args(args)
        .current_dir(dir)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
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
    let output = tm(&scratch, &repo, &["init", "--key", "TM", "--yes"]);
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
    let output = tm(&scratch, &repo, &["init", "--yes"]);
    if !repo.join(".config/tm.toml").exists() {
        // A case-sensitive disk finds no repo file and starts afresh.
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
    let output = Command::new(env!("CARGO_BIN_EXE_tm"))
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
    let output = Command::new(env!("CARGO_BIN_EXE_tm"))
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
    let output = Command::new(env!("CARGO_BIN_EXE_tm"))
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
    };
    fs::create_dir_all(repo.join(".config")).unwrap();
    fs::write(repo.join(".config/tm.toml"), file.render()).unwrap();
    let output = tm(&scratch, &repo, &["init", "--yes"]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains(&id.to_string()), "{output:?}");
    assert!(
        !scratch.home().exists(),
        "a fresh clone gets no new Workspace"
    );
}

#[test]
fn the_key_defaults_to_the_directory_name() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "my-repo");
    let output = tm(&scratch, &repo, &["init", "--yes"]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        text(&output.stdout).contains("+project = \"MYREPO\""),
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
        let mut command = Command::new(env!("CARGO_BIN_EXE_tm"));
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
    // The scratch directory sits inside this checkout, so use the system's.
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
        Command::new(env!("CARGO_BIN_EXE_tm"))
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
        text(&output.stderr).contains("is not absolute"),
        "{output:?}"
    );
    let output = init(&project, &scratch.0.join("missing"));
    assert!(output.status.success(), "a missing $HOME is not this repo");
    fs::remove_dir_all(home.join(".config")).unwrap();

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
    let output = Command::new(env!("CARGO_BIN_EXE_tm"))
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
    assert!(
        text(&output.stderr).contains("home directory"),
        "{output:?}"
    );
    assert!(!home.join(".config").exists());
}

#[test]
fn concurrent_inits_write_the_file_once() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let children: Vec<_> = (0..8)
        .map(|_| {
            Command::new(env!("CARGO_BIN_EXE_tm"))
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
