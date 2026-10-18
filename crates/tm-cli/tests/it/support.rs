//! Fixture repos and a `tm` that runs in them, shared by the suites.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use uuid::Uuid;

/// A scratch directory outside any other repo's reach: each fixture repo has
/// its own `.git`, so discovery stops there.
pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    pub(crate) fn new() -> Self {
        let path =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("tm-cli-{}", Uuid::now_v7()));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(&path).unwrap())
    }

    pub(crate) fn home(&self) -> PathBuf {
        self.0.join("tm-home")
    }

    /// Empty, so no user or system git config reaches the fixture.
    pub(crate) fn git_config(&self) -> PathBuf {
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

pub(crate) fn git(scratch: &Scratch, dir: &Path, args: &[&str]) -> Output {
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
pub(crate) fn fixture(scratch: &Scratch, name: &str) -> PathBuf {
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

/// Variables of the session the tests run in that would change what `tm`
/// does, such as an Agent's, so each spawned `tm` starts without them.
const SESSION_VARS: [&str; 9] = [
    "TM_AGENT",
    "CODEX_SESSION_ID",
    "CODEX_THREAD_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "GEMINI_CLI",
    "OPENCODE",
    "CURSOR_AGENT",
    "TM_JSON",
];

pub(crate) fn tm_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tm"));
    for var in SESSION_VARS {
        command.env_remove(var);
    }
    command
}

/// `tm` in `dir`, writing as the human `jace` into the scratch home.
pub(crate) fn tm(scratch: &Scratch, dir: &Path, args: &[&str]) -> Output {
    tm_command()
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

pub(crate) fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
