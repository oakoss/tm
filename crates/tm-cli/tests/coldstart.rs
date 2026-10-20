//! The cold-start budget (build-plan.md, Phase 1): each read command's median
//! time over warm-cache runs of a release build, on Workspaces of 200 and
//! 10,000 Issues, with identity read from git as a session-start hook reads
//! it. Writes are timed and printed, not held to a budget yet.
//!
//! Run with `mise run coldstart`; plain `cargo test` skips it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::json;
use tm_core::{Actor, Change};
use tm_store::workspace::Workspace;
use tm_store::{Draft, Store};
use uuid::Uuid;

/// Runs per command after one warm-up; the median absorbs a noisy runner.
const RUNS: usize = 15;

/// A read command and its budget, in milliseconds, at 200 and 10,000 Issues.
/// `prime` reads the whole ready queue until tm-u8g.24, so it grows with the
/// Workspace; `list` and `ready` print every row, which CI's runners took up
/// to 158 ms to do, so they have 200 ms until tm-u8g.25.
const READS: [(&[&str], u64, u64); 12] = [
    (&["--version"], 40, 60),
    (&["show", "TM-1"], 40, 60),
    (&["prime"], 40, 150),
    (&["scheme", "show"], 40, 60),
    (&["list"], 40, 200),
    (&["ready"], 40, 200),
    (&["--json", "--version"], 40, 60),
    (&["--json", "show", "TM-1"], 40, 60),
    (&["--json", "prime"], 40, 150),
    (&["--json", "scheme", "show"], 40, 60),
    (&["--json", "list"], 40, 200),
    (&["--json", "ready"], 40, 200),
];

struct Bench {
    root: PathBuf,
    repo: PathBuf,
}

impl Bench {
    fn new(size: usize) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("tm-coldstart-{size}-{}", Uuid::now_v7()));
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(
            root.join("gitconfig"),
            "[user]\n\temail = jace@example.com\n",
        )
        .unwrap();
        let bench = Self { root, repo };
        let made = bench.command("git").args(["init", "-q"]).output().unwrap();
        assert!(made.status.success(), "{made:?}");
        bench
    }

    /// A process with identity from the fixture's git config and the Agent
    /// named, as a session-start hook runs; nothing from this session leaks
    /// in.
    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.repo)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.root)
            .env("TM_HOME", self.root.join("tm-home"))
            .env("TM_AGENT", "claude")
            .env("GIT_CONFIG_GLOBAL", self.root.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1");
        command
    }

    fn tm(&self, args: &[&str]) -> Output {
        let out = self
            .command(env!("CARGO_BIN_EXE_tm"))
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "tm {args:?}: {out:?}");
        out
    }

    /// The median of [`RUNS`] timed runs after one warm-up.
    fn median(&self, args: &[&str]) -> Duration {
        self.tm(args);
        let mut times: Vec<Duration> = (0..RUNS)
            .map(|_| {
                let started = Instant::now();
                self.tm(args);
                started.elapsed()
            })
            .collect();
        times.sort();
        times[RUNS / 2]
    }

    /// `tm init`, then `size` Issues written through the store in one batch:
    /// a mix of Statuses and priorities, a label on each, a comment on every
    /// tenth and a `blocks` Link on every fifth.
    fn seed(&self, size: usize) -> Duration {
        let made = self.tm(&["--json", "init", "--yes", "--key", "TM"]);
        let doc: serde_json::Value = serde_json::from_slice(&made.stdout).unwrap();
        let workspace: Uuid = doc["data"]["workspace"].as_str().unwrap().parse().unwrap();
        let path = Workspace::at(&self.root.join("tm-home"), workspace).store_path();
        let mut store = Store::open(&path).unwrap();
        let project = store.project_by_key("TM").unwrap().unwrap();
        let jace = Actor::human("jace@example.com");
        let set = |field: &str, value| Change::SetField {
            field: field.into(),
            label: String::new(),
            value,
            value_label: None,
        };
        let mut drafts = Vec::new();
        let mut previous = None;
        for n in 0..size {
            let issue = Uuid::now_v7();
            let mut changes = vec![
                Change::Create,
                set("type", json!("task")),
                set(
                    "title",
                    json!(format!("Issue {n} with a title of a typical length")),
                ),
                set("status", json!(["todo", "in_progress", "done"][n % 3])),
                set("priority", json!(["none", "low", "medium", "high"][n % 4])),
                Change::Add {
                    field: "labels".into(),
                    label: String::new(),
                    elem: ["cli", "store", "tui"][n % 3].into(),
                    elem_label: None,
                },
            ];
            if n % 10 == 0 {
                changes.push(Change::Comment {
                    body: "A comment of a typical length on this Issue.".into(),
                });
            }
            drafts.extend(changes.into_iter().map(|change| Draft {
                actor: jace.clone(),
                project,
                subject: issue,
                change,
            }));
            if let Some(blocker) = previous.filter(|_| n % 5 == 0) {
                drafts.push(Draft {
                    actor: jace.clone(),
                    project,
                    subject: blocker,
                    change: Change::Link {
                        link_type: tm_core::link::BLOCKS.into(),
                        target: issue,
                        present: true,
                    },
                });
            }
            previous = Some(issue);
        }
        let started = Instant::now();
        store.append_all(drafts).unwrap();
        started.elapsed()
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The `n`th run of a timed write; moves and links alternate, so each run
/// writes something.
fn write(name: &str, n: usize) -> Vec<&'static str> {
    match (name, n % 2) {
        ("create", _) => vec!["create", "--type", "task", "--title", "timed"],
        ("comment", _) => vec!["comment", "TM-1", "timed"],
        ("move", 0) => vec!["move", "TM-4", "in_progress"],
        ("move", _) => vec!["move", "TM-4", "todo"],
        ("link", 0) => vec!["link", "TM-7", "relates_to", "TM-8"],
        _ => vec!["unlink", "TM-7", "relates_to", "TM-8"],
    }
}

fn ms(time: Duration) -> f64 {
    time.as_secs_f64() * 1000.0
}

/// Times every read and write on a Workspace of `size` Issues; returns each
/// read over its budget.
fn run(size: usize, budget: impl Fn(u64, u64) -> u64) -> Vec<String> {
    let bench = Bench::new(size);
    let seeded = bench.seed(size);
    eprintln!("{size} Issues seeded in {:.1} s", seeded.as_secs_f64());
    // A failed identity read is a warning, not an error, so a broken git
    // would drop the read the budget means to time.
    let primed: serde_json::Value =
        serde_json::from_slice(&bench.tm(&["--json", "prime"]).stdout).unwrap();
    assert_eq!(primed["warnings"], json!([]), "{primed}");
    assert_eq!(
        primed["data"]["actor"],
        json!({"name": "claude", "agent": true, "on_behalf_of": "jace@example.com"})
    );

    let mut over = Vec::new();
    for (args, small, large) in READS {
        let limit = budget(small, large);
        let median = bench.median(args);
        let verdict = if median > Duration::from_millis(limit) {
            over.push(format!(
                "tm {} at {size} Issues: {:.1} ms, budget {limit} ms",
                args.join(" "),
                ms(median)
            ));
            "OVER"
        } else {
            "ok"
        };
        eprintln!(
            "{size:>6}  read   tm {:<24} {:>7.1} ms  (budget {limit} ms) {verdict}",
            args.join(" "),
            ms(median)
        );
    }

    for name in ["create", "comment", "move", "link"] {
        let mut times: Vec<Duration> = (0..=RUNS)
            .map(|n| {
                let started = Instant::now();
                bench.tm(&write(name, n));
                started.elapsed()
            })
            .skip(1)
            .collect();
        times.sort();
        eprintln!(
            "{size:>6}  write  tm {name:<24} {:>7.1} ms  (not budgeted)",
            ms(times[RUNS / 2])
        );
    }
    over
}

#[test]
#[ignore = "times a release build; run with `mise run coldstart`"]
fn read_commands_meet_the_cold_start_budget() {
    assert!(
        Path::new(env!("CARGO_BIN_EXE_tm"))
            .components()
            .any(|part| part.as_os_str() == "release"),
        "the budget is for release builds; run `mise run coldstart`"
    );
    let mut over = run(200, |small, _| small);
    over.extend(run(10_000, |_, large| large));
    assert!(over.is_empty(), "over budget:\n{}", over.join("\n"));
}
