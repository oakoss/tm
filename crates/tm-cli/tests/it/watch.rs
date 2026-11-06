//! `--watch` prints a list command's result again when another process
//! writes (ADR 0018).

use std::fs;
use std::io::{BufRead as _, BufReader};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::support::{Scratch, initialized, jace, tm};

/// Long enough for several polls on a loaded runner.
const WAIT: Duration = Duration::from_secs(10);

/// A `tm` watching, killed when dropped so a failed test leaves none behind.
struct Watching {
    child: Child,
    lines: Option<Receiver<String>>,
}

impl Watching {
    fn start(scratch: &Scratch, repo: &Path, args: &[&str]) -> Self {
        let mut child = jace(scratch, repo, args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            lines: Some(lines),
        }
    }

    fn next(&self) -> String {
        self.lines.as_ref().unwrap().recv_timeout(WAIT).unwrap()
    }

    fn quiet_for(&self, time: Duration) -> bool {
        self.lines.as_ref().unwrap().recv_timeout(time).is_err()
    }

    /// Stops reading, so the pipe closes once the reader thread next wakes.
    fn close(&mut self) {
        self.lines = None;
    }

    fn exited(&mut self) -> ExitStatus {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "still watching");
            thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn keys(line: &str) -> Vec<String> {
    let doc: Value = serde_json::from_str(line).unwrap_or_else(|err| panic!("{err}: {line}"));
    doc["data"]["issues"]
        .as_array()
        .unwrap_or_else(|| panic!("{line}"))
        .iter()
        .map(|issue| issue["key"].as_str().unwrap().to_owned())
        .collect()
}

/// A write from another process.
fn create(scratch: &Scratch, repo: &Path, title: &str) {
    let made = tm(
        scratch,
        repo,
        &["create", "--type", "task", "--title", title],
    );
    assert!(made.status.success(), "{made:?}");
}

fn store_path(scratch: &Scratch) -> PathBuf {
    let mut workspaces = fs::read_dir(scratch.home().join("workspaces")).unwrap();
    workspaces.next().unwrap().unwrap().path().join("store.db")
}

#[test]
fn a_write_elsewhere_prints_the_list_again_as_one_document_a_line() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    create(&scratch, &repo, "a");
    let watching = Watching::start(&scratch, &repo, &["--json", "list", "--watch"]);
    assert_eq!(keys(&watching.next()), ["TM-1"]);
    assert!(
        watching.quiet_for(Duration::from_secs(1)),
        "no write, no list"
    );
    create(&scratch, &repo, "b");
    assert_eq!(keys(&watching.next()).len(), 2);
}

#[test]
fn text_marks_each_list_after_the_first_with_when_it_was_read() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let watching = Watching::start(&scratch, &repo, &["claims", "--watch"]);
    assert_eq!(watching.next(), "Held (0):");
    create(&scratch, &repo, "a");
    let marked = (0..10)
        .map(|_| watching.next())
        .find(|line| line.starts_with("-- changed, read at "));
    assert!(marked.is_some(), "no line marks the second list");
}

#[test]
fn ready_reads_again_and_a_reader_closing_the_pipe_ends_it() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let mut watching = Watching::start(&scratch, &repo, &["--json", "ready", "--watch"]);
    watching.next();
    create(&scratch, &repo, "a");
    assert!(watching.next().contains("\"TM-1\""));
    watching.close();
    for title in ["b", "c"] {
        create(&scratch, &repo, title);
        thread::sleep(Duration::from_millis(600));
    }
    assert!(watching.exited().success());
}

#[test]
fn a_store_removed_while_watching_ends_it_with_an_error() {
    let scratch = Scratch::new();
    let (repo, store, _project) = initialized(&scratch);
    drop(store);
    let mut watching = Watching::start(&scratch, &repo, &["--json", "list", "--watch"]);
    watching.next();
    let path = store_path(&scratch);
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let said: Value = serde_json::from_str(&watching.next()).unwrap();
    assert_eq!(said["error"], "io", "{said}");
    assert!(!watching.exited().success());
}

#[test]
fn a_reader_refused_the_store_watches_the_snapshot() {
    let scratch = Scratch::new();
    let (repo, store, _project) = initialized(&scratch);
    drop(store);
    create(&scratch, &repo, "a");
    let path = store_path(&scratch);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let watching = Watching::start(&scratch, &repo, &["--json", "list", "--watch"]);
    let first = watching.next();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(first.contains("read from the Snapshot"), "{first}");
    assert_eq!(keys(&first), ["TM-1"]);
    create(&scratch, &repo, "b");
    assert_eq!(keys(&watching.next()).len(), 2);
}
