//! Sequential Key assignment by many short-lived writer processes.
//!
//! `key-probe concurrent <strategy> <procs> <each> <rollback_every> <dir>`
//! `key-probe kill <dir>`
//! `key-probe purge <dir>`
//! `key-probe rebuild <db>`
//!
//! Strategies: `counter-immediate` (a `key_counters` row bumped inside
//! `BEGIN IMMEDIATE`), `max-immediate` (`max(number) + 1` inside
//! `BEGIN IMMEDIATE`), `max-deferred` (`max(number) + 1` inside a plain
//! `BEGIN`), `max-outside` (`max(number) + 1` read before `BEGIN IMMEDIATE`,
//! the bug the UNIQUE constraint must catch). Children never retry, so every
//! failure is counted.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rusqlite::{Connection, ErrorCode, OptionalExtension, TransactionBehavior, params};

const PROJECT: &str = "CPL";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS ops (
  seq INTEGER PRIMARY KEY,
  op_id TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL,
  project TEXT NOT NULL,
  issue_id TEXT NOT NULL,
  assigned_key TEXT
) STRICT;
CREATE TABLE IF NOT EXISTS keys (
  key TEXT PRIMARY KEY,
  issue_id TEXT NOT NULL,
  project TEXT NOT NULL,
  number INTEGER,
  provisional INTEGER NOT NULL,
  UNIQUE (project, number)
) STRICT;
CREATE TABLE IF NOT EXISTS key_counters (
  project TEXT PRIMARY KEY,
  next_number INTEGER NOT NULL
) STRICT;
";

fn open(db: &Path) -> Connection {
    let conn = Connection::open(db).unwrap();
    conn.busy_timeout(Duration::from_secs(5)).unwrap();
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
        .unwrap();
    conn
}

fn fresh(dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let db = dir.join(name);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
    }
    open(&db).execute_batch(SCHEMA).unwrap();
    db
}

fn reserve_counter(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<i64> {
    tx.query_row(
        "INSERT INTO key_counters (project, next_number) VALUES (?1, 2)
         ON CONFLICT (project) DO UPDATE SET next_number = next_number + 1
         RETURNING next_number - 1",
        [PROJECT],
        |r| r.get(0),
    )
}

fn reserve_max(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<i64> {
    tx.query_row(
        "SELECT coalesce(max(number), 0) + 1 FROM keys WHERE project = ?1",
        [PROJECT],
        |r| r.get(0),
    )
}

fn record(tx: &rusqlite::Transaction<'_>, tag: &str, number: i64) -> rusqlite::Result<()> {
    let key = format!("{PROJECT}-{number}");
    tx.execute(
        "INSERT INTO ops (op_id, kind, project, issue_id, assigned_key)
         VALUES (?1, 'create', ?2, ?1, ?3)",
        params![tag, PROJECT, key],
    )?;
    tx.execute(
        "INSERT INTO keys (key, issue_id, project, number, provisional) VALUES (?1, ?2, ?3, ?4, 0)",
        params![key, tag, PROJECT, number],
    )?;
    Ok(())
}

#[derive(Default)]
struct Tally {
    ok: u64,
    rolled_back: u64,
    busy: u64,
    busy_snapshot: u64,
    unique: u64,
    other: u64,
}

fn classify(err: &rusqlite::Error, tally: &mut Tally) {
    match err {
        rusqlite::Error::SqliteFailure(e, _) if e.extended_code == 517 => tally.busy_snapshot += 1,
        rusqlite::Error::SqliteFailure(e, _) if e.code == ErrorCode::DatabaseBusy => {
            tally.busy += 1
        }
        rusqlite::Error::SqliteFailure(e, _) if e.code == ErrorCode::ConstraintViolation => {
            tally.unique += 1;
        }
        _ => tally.other += 1,
    }
}

fn child(strategy: &str, db: &Path, each: u64, rollback_every: u64) {
    let mut conn = open(db);
    let pid = std::process::id();
    let mut tally = Tally::default();
    for i in 0..each {
        let behavior = if strategy == "max-deferred" {
            TransactionBehavior::Deferred
        } else {
            TransactionBehavior::Immediate
        };
        let result = (|| -> rusqlite::Result<bool> {
            let early: i64 = if strategy == "max-outside" {
                conn.query_row(
                    "SELECT coalesce(max(number), 0) + 1 FROM keys WHERE project = ?1",
                    [PROJECT],
                    |r| r.get(0),
                )?
            } else {
                0
            };
            let tx = conn.transaction_with_behavior(behavior)?;
            let number = match strategy {
                "counter-immediate" => reserve_counter(&tx)?,
                "max-outside" => early,
                _ => reserve_max(&tx)?,
            };
            record(&tx, &format!("{pid}-{i}"), number)?;
            if rollback_every > 0 && i % rollback_every == rollback_every - 1 {
                tx.rollback()?;
                return Ok(false);
            }
            tx.commit()?;
            Ok(true)
        })();
        match result {
            Ok(true) => tally.ok += 1,
            Ok(false) => tally.rolled_back += 1,
            Err(err) => classify(&err, &mut tally),
        }
    }
    println!(
        "{} {} {} {} {} {}",
        tally.ok, tally.rolled_back, tally.busy, tally.busy_snapshot, tally.unique, tally.other
    );
}

struct Summary {
    rows: i64,
    distinct: i64,
    min: Option<i64>,
    max: Option<i64>,
}

fn summarize(db: &Path) -> Summary {
    let conn = open(db);
    conn.query_row(
        "SELECT count(*), count(DISTINCT number), min(number), max(number) FROM keys",
        [],
        |r| {
            Ok(Summary {
                rows: r.get(0)?,
                distinct: r.get(1)?,
                min: r.get(2)?,
                max: r.get(3)?,
            })
        },
    )
    .unwrap()
}

fn spawn_children(strategy: &str, db: &Path, procs: u64, each: u64, rollback_every: u64) -> Tally {
    let exe = std::env::current_exe().unwrap();
    let children: Vec<_> = (0..procs)
        .map(|_| {
            Command::new(&exe)
                .args(["child", strategy])
                .arg(db)
                .args([each.to_string(), rollback_every.to_string()])
                .stdout(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut total = Tally::default();
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "a child crashed");
        let line = String::from_utf8(out.stdout).unwrap();
        let n: Vec<u64> = line
            .split_whitespace()
            .map(|s| s.parse().unwrap())
            .collect();
        total.ok += n[0];
        total.rolled_back += n[1];
        total.busy += n[2];
        total.busy_snapshot += n[3];
        total.unique += n[4];
        total.other += n[5];
    }
    total
}

fn concurrent(strategy: &str, procs: u64, each: u64, rollback_every: u64, dir: &Path) {
    let db = fresh(dir, &format!("{strategy}.db"));
    let start = Instant::now();
    let t = spawn_children(strategy, &db, procs, each, rollback_every);
    let elapsed = start.elapsed();
    let s = summarize(&db);
    println!(
        "strategy={strategy} procs={procs} each={each} rollback_every={rollback_every} \
         elapsed_ms={} committed={} rolled_back={} busy={} busy_snapshot={} unique_violation={} other={} \
         | keys_rows={} distinct_numbers={} min={:?} max={:?} gaps={}",
        elapsed.as_millis(),
        t.ok,
        t.rolled_back,
        t.busy,
        t.busy_snapshot,
        t.unique,
        t.other,
        s.rows,
        s.distinct,
        s.min,
        s.max,
        s.max.unwrap_or(0) - s.rows,
    );
}

/// Holds a reserved Key inside an open `BEGIN IMMEDIATE` until killed.
fn hold(db: &Path) {
    let mut conn = open(db);
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let number = reserve_counter(&tx).unwrap();
    record(&tx, "killed", number).unwrap();
    println!("reserved {number}");
    std::io::stdout().flush().unwrap();
    std::thread::sleep(Duration::from_secs(60));
}

fn kill(dir: &Path) {
    let db = fresh(dir, "kill.db");
    let exe = std::env::current_exe().unwrap();
    let mut holder = Command::new(&exe)
        .arg("hold")
        .arg(&db)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(holder.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    holder.kill().unwrap();
    let status = holder.wait().unwrap();
    let t = spawn_children("counter-immediate", &db, 10, 10, 0);
    let s = summarize(&db);
    let killed_rows: i64 = open(&db)
        .query_row("SELECT count(*) FROM ops WHERE op_id = 'killed'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let who_has_1: Option<String> = open(&db)
        .query_row("SELECT issue_id FROM keys WHERE number = 1", [], |r| {
            r.get(0)
        })
        .optional()
        .unwrap();
    println!(
        "holder said `{}`, exited {status}; then 10x10 writers: committed={} busy={} \
         | keys_rows={} distinct={} min={:?} max={:?} killed_rows_in_log={killed_rows} \
         number_1_now_held_by_killed={}",
        line.trim(),
        t.ok,
        t.busy,
        s.rows,
        s.distinct,
        s.min,
        s.max,
        who_has_1.as_deref() == Some("killed"),
    );
}

fn next_from_keys(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT coalesce(max(number), 0) + 1 FROM keys WHERE project = ?1",
        [PROJECT],
        |r| r.get(0),
    )
    .unwrap()
}

fn next_from_log(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT coalesce(max(CAST(substr(assigned_key, length(?1) + 2) AS INTEGER)), 0) + 1
         FROM ops WHERE project = ?1 AND assigned_key IS NOT NULL",
        [PROJECT],
        |r| r.get(0),
    )
    .unwrap()
}

/// Rebuilds `keys` from the log the way `tm rebuild` would.
fn refill_keys(conn: &Connection) {
    conn.execute_batch(
        "DELETE FROM keys; DELETE FROM key_counters;
         INSERT INTO keys (key, issue_id, project, number, provisional)
           SELECT assigned_key, issue_id, project,
                  CAST(substr(assigned_key, length(project) + 2) AS INTEGER), 0
           FROM ops WHERE kind = 'create' AND assigned_key IS NOT NULL;",
    )
    .unwrap();
}

/// What "never reused, even after purge" needs from the log.
fn purge(dir: &Path) {
    let db = fresh(dir, "purge.db");
    let mut conn = open(&db);
    for i in 1..=3 {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let n = reserve_counter(&tx).unwrap();
        record(&tx, &format!("issue-{i}"), n).unwrap();
        tx.commit().unwrap();
    }
    let counter: i64 = conn
        .query_row("SELECT next_number FROM key_counters", [], |r| r.get(0))
        .unwrap();
    println!("created CPL-1..CPL-3; counter row next_number={counter}");

    // Purge A: the projection row goes, the log keeps the create row.
    conn.execute("DELETE FROM keys WHERE issue_id = 'issue-3'", [])
        .unwrap();
    println!(
        "purge A (projection row deleted, create op kept): next from max(keys)={} next from counter row={} next from log={}",
        next_from_keys(&conn),
        counter,
        next_from_log(&conn)
    );
    refill_keys(&conn);
    println!(
        "  after rebuild from log: next from max(keys)={} (CPL-3 present again: {})",
        next_from_keys(&conn),
        conn.query_row("SELECT count(*) FROM keys WHERE key = 'CPL-3'", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap()
            == 1
    );

    // Purge B: the create row itself is removed from the log.
    conn.execute("DELETE FROM ops WHERE issue_id = 'issue-3'", [])
        .unwrap();
    refill_keys(&conn);
    println!(
        "purge B (create op deleted, then rebuild): next from max(keys)={} next from log={} -> CPL-3 would be reused",
        next_from_keys(&conn),
        next_from_log(&conn)
    );
}

/// Drops and refills `keys` from `ops.assigned_key`, then compares.
fn rebuild(db: &Path) {
    let conn = open(db);
    let dump = |conn: &Connection| -> Vec<(String, String, i64)> {
        let mut stmt = conn
            .prepare("SELECT key, issue_id, number FROM keys ORDER BY key")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let before = dump(&conn);
    let counter: Option<i64> = conn
        .query_row(
            "SELECT next_number FROM key_counters WHERE project = ?1",
            [PROJECT],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    refill_keys(&conn);
    let after = dump(&conn);
    // Numbering recomputed from log order instead of read from the column.
    let renumbered: i64 = conn
        .query_row(
            "SELECT count(*) FROM (
               SELECT assigned_key, row_number() OVER (ORDER BY op_id) AS n FROM ops WHERE kind = 'create')
             WHERE assigned_key != 'CPL-' || n",
            [],
            |r| r.get(0),
        )
        .unwrap();
    println!(
        "rebuild: rows before={} after={} identical={} counter_row_before={counter:?} next_after_rebuild={} \
         | renumbering by another order (op_id) would change {renumbered} Keys",
        before.len(),
        after.len(),
        before == after,
        next_from_keys(&conn),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["child", strategy, db, each, rb] => {
            child(
                strategy,
                Path::new(db),
                each.parse().unwrap(),
                rb.parse().unwrap(),
            );
        }
        ["concurrent", strategy, procs, each, rb, dir] => concurrent(
            strategy,
            procs.parse().unwrap(),
            each.parse().unwrap(),
            rb.parse().unwrap(),
            Path::new(dir),
        ),
        ["hold", db] => hold(Path::new(db)),
        ["kill", dir] => kill(Path::new(dir)),
        ["purge", dir] => purge(Path::new(dir)),
        ["rebuild", db] => rebuild(Path::new(db)),
        _ => {
            eprintln!("see the module docs for usage");
            std::process::exit(2);
        }
    }
}
