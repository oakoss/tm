//! Races many short-lived processes to take Issues in one SQLite store, the way
//! parallel `tm` invocations would, and counts how many believe they won.
//!
//! Usage:
//!   claim-race <dir> <mode> <rule> <agents> <procs> <issues> <runs> <gap_us>
//!
//! mode:   autocommit | deferred | immediate | conditional   (take Issue 1)
//!         next-autocommit | next-immediate                  (take the next unclaimed Issue)
//! rule:   strict      a take succeeds only on an unclaimed Issue
//!         idempotent  a take by the current holder also succeeds (beads' rule)
//! agents: distinct | same   whether every process uses its own Agent name
//! gap_us: microseconds of simulated work between the check and the write

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rusqlite::{Connection, ErrorCode, OptionalExtension, TransactionBehavior, params};

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
CREATE TABLE issues (id INTEGER PRIMARY KEY);
CREATE TABLE ops (
  seq INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  issue INTEGER NOT NULL,
  agent TEXT NOT NULL,
  prev_claim INTEGER
);
CREATE TABLE claims (
  issue INTEGER PRIMARY KEY,
  agent TEXT NOT NULL,
  op_seq INTEGER
);
";

#[derive(Clone, Copy, PartialEq)]
enum Rule {
    Strict,
    Idempotent,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("child") {
        child(&args[2..]);
        return;
    }
    let [_, dir, mode, rule, agents, procs, issues, runs, gap] = args.as_slice() else {
        eprintln!(
            "usage: claim-race <dir> <mode> <rule> <agents> <procs> <issues> <runs> <gap_us>"
        );
        std::process::exit(2);
    };
    let procs: usize = procs.parse().unwrap();
    let issues: i64 = issues.parse().unwrap();
    let runs: usize = runs.parse().unwrap();
    let mut totals: HashMap<String, usize> = HashMap::new();
    let mut violating_runs = 0;
    let mut max_ms = 0u128;
    for run in 0..runs {
        let db = PathBuf::from(dir).join(format!("race-{run}.db"));
        let go = PathBuf::from(dir).join(format!("go-{run}"));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
        }
        let _ = std::fs::remove_file(&go);
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        for id in 1..=issues {
            conn.execute("INSERT INTO issues (id) VALUES (?1)", [id])
                .unwrap();
        }
        drop(conn);

        let exe = std::env::current_exe().unwrap();
        let children: Vec<_> = (0..procs)
            .map(|i| {
                let agent = if agents == "same" {
                    "claude".to_owned()
                } else {
                    format!("agent-{i}")
                };
                Command::new(&exe)
                    .args(["child", db.to_str().unwrap(), go.to_str().unwrap()])
                    .args([mode.as_str(), rule.as_str(), agent.as_str(), gap.as_str()])
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap()
            })
            .collect();
        // Every child opens its connection, then waits for this file, so the
        // takes start together.
        std::thread::sleep(Duration::from_millis(300));
        let started = Instant::now();
        std::fs::write(&go, b"").unwrap();
        let mut believed: HashMap<i64, usize> = HashMap::new();
        for child in children {
            let out = child.wait_with_output().unwrap();
            let line = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            let mut parts = line.split_whitespace();
            let outcome = parts.next().unwrap_or("CRASH").to_owned();
            if outcome == "WON" || outcome == "MINE" {
                let issue: i64 = parts.next().unwrap().parse().unwrap();
                *believed.entry(issue).or_default() += 1;
            }
            *totals.entry(outcome).or_default() += 1;
        }
        max_ms = max_ms.max(started.elapsed().as_millis());
        let conn = Connection::open(&db).unwrap();
        let (claim_ops, claim_rows): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM ops WHERE kind = 'claim'), (SELECT count(*) FROM claims)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let doubled = believed.values().filter(|n| **n > 1).count();
        if doubled > 0 {
            violating_runs += 1;
        }
        let replay_winners = replay_by_prev(&conn);
        println!(
            "run {run}: claim ops logged={claim_ops} claims rows={claim_rows} \
             Issues held by >1 process={doubled} \
             takes a prev-matching replay keeps={replay_winners}"
        );
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
        }
        let _ = std::fs::remove_file(&go);
    }
    let mut totals: Vec<_> = totals.into_iter().collect();
    totals.sort();
    println!(
        "{mode} rule={rule} agents={agents} procs={procs} issues={issues} gap_us={gap} runs={runs}: \
         outcomes {totals:?}; runs where an Issue was held by >1 process: {violating_runs}; \
         slowest run {max_ms} ms"
    );
}

/// Replays the logged claims in log order, keeping a take only when the Claim
/// it recorded as current (`prev_claim`) is still the current one; returns how
/// many takes survive.
fn replay_by_prev(conn: &Connection) -> usize {
    let mut stmt = conn
        .prepare("SELECT seq, issue, prev_claim FROM ops WHERE kind = 'claim' ORDER BY seq")
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        })
        .unwrap();
    let mut head: HashMap<i64, i64> = HashMap::new();
    let mut kept = 0;
    for row in rows {
        let (seq, issue, prev) = row.unwrap();
        if head.get(&issue).copied() == prev {
            head.insert(issue, seq);
            kept += 1;
        }
    }
    kept
}

/// What one process concluded.
enum Outcome {
    Won(i64),
    /// The caller already held the Claim and the rule treats that as success.
    Mine(i64),
    Held(String),
    NoneReady,
}

fn child(args: &[String]) {
    let [db, go, mode, rule, agent, gap] = args else {
        panic!("bad child args")
    };
    let rule = if rule == "idempotent" {
        Rule::Idempotent
    } else {
        Rule::Strict
    };
    let gap = Duration::from_micros(gap.parse().unwrap());
    let mut conn = Connection::open(db).unwrap();
    conn.busy_timeout(Duration::from_secs(5)).unwrap();
    conn.execute_batch("PRAGMA synchronous = NORMAL;").unwrap();
    while !Path::new(go).exists() {
        std::thread::sleep(Duration::from_micros(200));
    }
    let result = match mode.as_str() {
        "autocommit" => take_autocommit(&mut conn, 1, agent, rule, gap),
        "deferred" => take_in_tx(
            &mut conn,
            TransactionBehavior::Deferred,
            1,
            agent,
            rule,
            gap,
        ),
        "immediate" => take_in_tx(
            &mut conn,
            TransactionBehavior::Immediate,
            1,
            agent,
            rule,
            gap,
        ),
        "conditional" => take_conditional(&mut conn, 1, agent, gap),
        "next-autocommit" => next_autocommit(&mut conn, agent, gap),
        "next-immediate" => next_immediate(&mut conn, agent, gap),
        other => panic!("unknown mode {other}"),
    };
    match result {
        Ok(Outcome::Won(issue)) => println!("WON {issue}"),
        Ok(Outcome::Mine(issue)) => println!("MINE {issue}"),
        Ok(Outcome::Held(holder)) => println!("HELD {holder}"),
        Ok(Outcome::NoneReady) => println!("NONE"),
        Err(err) if err.sqlite_error_code() == Some(ErrorCode::DatabaseBusy) => println!("BUSY"),
        Err(err) => println!("ERR {err}"),
    }
}

fn holder(conn: &Connection, issue: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT agent FROM claims WHERE issue = ?1", [issue], |r| {
        r.get(0)
    })
    .optional()
}

fn decide(current: Option<String>, agent: &str, rule: Rule, issue: i64) -> Option<Outcome> {
    match current {
        None => None,
        Some(h) if rule == Rule::Idempotent && h == agent => Some(Outcome::Mine(issue)),
        Some(h) => Some(Outcome::Held(h)),
    }
}

/// Appends the claim operation and updates the projection; the caller owns the
/// transaction. Every caller writes only after seeing the Issue unclaimed, so
/// the operation records no previous Claim.
fn write_claim(conn: &Connection, issue: i64, agent: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO ops (kind, issue, agent, prev_claim) VALUES ('claim', ?1, ?2, NULL)",
        params![issue, agent],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO claims (issue, agent, op_seq) VALUES (?1, ?2, last_insert_rowid())",
        params![issue, agent],
    )?;
    Ok(())
}

/// The check runs outside any transaction; only the write is atomic.
fn take_autocommit(
    conn: &mut Connection,
    issue: i64,
    agent: &str,
    rule: Rule,
    gap: Duration,
) -> rusqlite::Result<Outcome> {
    if let Some(outcome) = decide(holder(conn, issue)?, agent, rule, issue) {
        return Ok(outcome);
    }
    std::thread::sleep(gap);
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    write_claim(&tx, issue, agent)?;
    tx.commit()?;
    Ok(Outcome::Won(issue))
}

/// Check and write in one transaction of the given kind.
fn take_in_tx(
    conn: &mut Connection,
    behavior: TransactionBehavior,
    issue: i64,
    agent: &str,
    rule: Rule,
    gap: Duration,
) -> rusqlite::Result<Outcome> {
    let tx = conn.transaction_with_behavior(behavior)?;
    if let Some(outcome) = decide(holder(&tx, issue)?, agent, rule, issue) {
        return Ok(outcome);
    }
    std::thread::sleep(gap);
    write_claim(&tx, issue, agent)?;
    tx.commit()?;
    Ok(Outcome::Won(issue))
}

/// The projection row is inserted first by one conditional statement; the
/// operation is logged only if that statement inserted.
fn take_conditional(
    conn: &mut Connection,
    issue: i64,
    agent: &str,
    gap: Duration,
) -> rusqlite::Result<Outcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let inserted: Option<i64> = tx
        .query_row(
            "INSERT INTO claims (issue, agent) VALUES (?1, ?2)
             ON CONFLICT (issue) DO NOTHING RETURNING issue",
            params![issue, agent],
            |r| r.get(0),
        )
        .optional()?;
    if inserted.is_none() {
        let h = holder(&tx, issue)?.unwrap_or_default();
        return Ok(Outcome::Held(h));
    }
    std::thread::sleep(gap);
    tx.execute(
        "INSERT INTO ops (kind, issue, agent) VALUES ('claim', ?1, ?2)",
        params![issue, agent],
    )?;
    tx.execute(
        "UPDATE claims SET op_seq = last_insert_rowid() WHERE issue = ?1",
        [issue],
    )?;
    tx.commit()?;
    Ok(Outcome::Won(issue))
}

const NEXT_READY: &str = "SELECT id FROM issues WHERE id NOT IN (SELECT issue FROM claims)
                          ORDER BY id LIMIT 1";

fn next_autocommit(conn: &mut Connection, agent: &str, gap: Duration) -> rusqlite::Result<Outcome> {
    let Some(issue) = conn
        .query_row(NEXT_READY, [], |r| r.get::<_, i64>(0))
        .optional()?
    else {
        return Ok(Outcome::NoneReady);
    };
    std::thread::sleep(gap);
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    write_claim(&tx, issue, agent)?;
    tx.commit()?;
    Ok(Outcome::Won(issue))
}

fn next_immediate(conn: &mut Connection, agent: &str, gap: Duration) -> rusqlite::Result<Outcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(issue) = tx
        .query_row(NEXT_READY, [], |r| r.get::<_, i64>(0))
        .optional()?
    else {
        return Ok(Outcome::NoneReady);
    };
    std::thread::sleep(gap);
    write_claim(&tx, issue, agent)?;
    tx.commit()?;
    Ok(Outcome::Won(issue))
}
