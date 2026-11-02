//! Times ways to copy a tm store for the Snapshot, and checks whether each
//! keeps the rowids of `issues` and the WAL mark.
//!
//! Usage: snapbench DIR ISSUES

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::{Connection, backup::Backup};
use serde_json::json;
use tm_core::{Actor, Change};
use tm_store::{Draft, Store};
use uuid::Uuid;

const RUNS: usize = 7;

/// Seeds a store as the cold-start test does: one batch of Issues with a
/// title, Status, priority and label, a comment on every tenth and a blocks
/// Link on every fifth.
fn seed(path: &Path, size: usize) {
    let mut store = Store::open(path).unwrap();
    let jace = Actor::human("jace@example.com");
    let (project, _) = store.create_project(&jace, "TM", "bench").unwrap();
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
    store.append_all(drafts).unwrap();
}

fn vacuum_into(src: &Connection, to: &Path) {
    src.execute("VACUUM INTO ?1", [to.to_str().unwrap()])
        .unwrap();
}

fn backup_api(src: &Connection, to: &Path) {
    let mut dst = Connection::open(to).unwrap();
    let backup = Backup::new(src, &mut dst).unwrap();
    backup
        .run_to_completion(i32::MAX, Duration::ZERO, None)
        .unwrap();
}

/// A page copy of a WAL store is marked WAL, which a reader denied every
/// write cannot open without `immutable=1`.
fn backup_rollback(src: &Connection, to: &Path) {
    backup_api(src, to);
    Connection::open(to)
        .unwrap()
        .query_row("PRAGMA journal_mode = DELETE", [], |_| Ok(()))
        .unwrap();
}

/// Header bytes 18 and 19 are 2 in a WAL database.
fn marked_wal(path: &Path) -> bool {
    let header = fs::read(path).unwrap();
    header[18] == 2 || header[19] == 2
}

/// Checkpoints the log into the main file, then copies the file, which
/// APFS turns into a clone. Consistent only while no write runs.
fn checkpoint_copy(src: &Connection, from: &Path, to: &Path) {
    src.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    fs::copy(from, to).unwrap();
}

/// The `issues` rowids in id order, to compare a copy with its source.
fn rowids(path: &Path) -> Vec<i64> {
    let conn = Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare("SELECT rowid FROM issues ORDER BY id")
        .unwrap();
    stmt.query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().unwrap());
    let size: usize = args.next().unwrap().parse().unwrap();
    fs::create_dir_all(&dir).unwrap();
    let store = dir.join(format!("store-{size}.db"));
    if !store.exists() {
        let started = Instant::now();
        seed(&store, size);
        eprintln!("seeded {size} in {:.1}s", started.elapsed().as_secs_f64());
    }
    let src = Connection::open(&store).unwrap();
    let source_rowids = rowids(&store);
    let size_mb = fs::metadata(&store).unwrap().len() as f64 / 1e6;
    println!("{size} Issues, {size_mb:.1} MB");

    let copy = dir.join("copy.db");
    let methods: [(&str, &dyn Fn(&Path)); 4] = [
        ("VACUUM INTO", &|to| vacuum_into(&src, to)),
        ("backup API", &|to| backup_api(&src, to)),
        ("backup + rollback", &|to| backup_rollback(&src, to)),
        ("checkpoint + copy", &|to| checkpoint_copy(&src, &store, to)),
    ];
    for (name, method) in methods {
        let mut times = Vec::new();
        for _ in 0..RUNS {
            let _ = fs::remove_file(&copy);
            let started = Instant::now();
            method(&copy);
            times.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        let wal = marked_wal(&copy);
        let kept = rowids(&copy) == source_rowids;
        println!(
            "  {name:<18} median {:>7.1} ms  min {:>7.1}  max {:>7.1}  rowids kept: {kept}  marked WAL: {wal}",
            times[RUNS / 2],
            times[0],
            times[RUNS - 1],
        );
    }
    let _ = fs::remove_file(&copy);
}
