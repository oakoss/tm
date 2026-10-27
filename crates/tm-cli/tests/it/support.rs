//! Fixture repos and a `tm` that runs in them, shared by the suites.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tm_core::ProjectId;
use tm_store::Store;
use tm_store::workspace::Workspace;
use uuid::Uuid;

/// A scratch directory outside any other repo's reach. Not under the target
/// directory: this repo maps itself in `.config/tm.toml`, and a command run
/// outside a fixture repo would find that file.
pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    pub(crate) fn new() -> Self {
        let path = std::env::temp_dir().join(format!("tm-cli-{}", Uuid::now_v7()));
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

/// A committed repo holding files tm must not touch unasked.
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
const SESSION_VARS: [&str; 11] = [
    "TM_AGENT",
    "TM_SESSION",
    "TM_SHORTLIST",
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
    jace(scratch, dir, args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// [`tm`] with `input` on stdin.
pub(crate) fn tm_piped(
    scratch: &Scratch,
    dir: &Path,
    args: &[&str],
    input: impl AsRef<[u8]>,
) -> Output {
    piped(jace(scratch, dir, args), input)
}

/// `command` with `input` on stdin.
pub(crate) fn piped(mut command: Command, input: impl AsRef<[u8]>) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_ref())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// [`tm`] at a pseudo-terminal, as a person typing would run it. Every wait,
/// and the kill when the session drops, gives up after `TIMEOUT_MS` rather
/// than hanging the suite. Echo is off, so typed lines never appear in what
/// the test reads.
pub(crate) fn tm_pty(scratch: &Scratch, dir: &Path, args: &[&str]) -> rexpect::session::PtySession {
    const TIMEOUT_MS: u64 = 10_000;
    rexpect::session::spawn_command(jace(scratch, dir, args), Some(TIMEOUT_MS)).unwrap()
}

fn jace(scratch: &Scratch, dir: &Path, args: &[&str]) -> Command {
    let mut command = tm_command();
    command
        .args(args)
        .current_dir(dir)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1");
    command
}

/// A repo mapped to a new Workspace with the Project TM, and its store.
pub(crate) fn initialized(scratch: &Scratch) -> (PathBuf, Store, ProjectId) {
    let repo = fixture(scratch, "repo");
    let made = tm(scratch, &repo, &["init", "--yes", "--json", "--key", "TM"]);
    assert!(made.status.success(), "{made:?}");
    let doc: serde_json::Value = serde_json::from_slice(&made.stdout).unwrap();
    let workspace: Uuid = doc["data"]["workspace"].as_str().unwrap().parse().unwrap();
    let store = Store::open(&Workspace::at(&scratch.home(), workspace).store_path()).unwrap();
    let project = store.project_by_key("TM").unwrap().unwrap();
    (repo, store, project)
}

pub(crate) fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Sleeps until the wall clock reaches `ms`, as a `tm` process reads it.
pub(crate) fn wait_until(ms: i64) {
    let now_ms = || {
        i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
    };
    while now_ms() < ms {
        thread::sleep(Duration::from_millis(50));
    }
}
