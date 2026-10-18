//! Costs behind core-commands.md: what `tm list --json` pays to open the
//! store, find Issues and read each one through `Store::issue`, what opening
//! a store scans, and whether a one-pass ready query stays finite on a merged
//! `blocks` cycle.
//!
//! cli-cost seed   <dir> <n>  n Issues; every 5th blocked by the one before; every 3rd done
//! cli-cost list   <dir>      open + find("") + issue() per id + JSON encode
//! cli-cost open   <dir>      the statements `Store::open` runs on every open, timed alone
//! cli-cost ready  <dir>      ready query and "would unblock" counts, then again on a merged cycle
//! cli-cost errors <dir>      the store's error shapes; writes a Claim and a no-op Link removal
//! cli-cost undo   <dir>      whether flipping `present` undoes a duplicate add

use std::io::Write as _;
use std::path::Path;
use std::time::Instant;

use rusqlite::{Connection, OptionalExtension};
use tm_core::Change;
use tm_core::filter::Context;
use tm_store::{Draft, Store};
use uuid::Uuid;

const READY: &str = "
SELECT i.id FROM issues i
WHERE i.archived = 0 AND i.status = 'todo'
  AND NOT EXISTS (SELECT 1 FROM claims c WHERE c.issue_id = i.id)
  AND NOT EXISTS (
    SELECT 1 FROM links l JOIN issues b ON b.id = l.src
    WHERE l.type = 'blocks' AND l.dst = i.id AND b.status NOT IN ('done', 'cancelled'))
";

/// For each ready Issue, how many Issues it blocks whose only open blocker it is.
const UNBLOCKS: &str = "
WITH ready(id) AS (READY_SQL)
SELECT r.id, count(t.id) AS unblocks FROM ready r
LEFT JOIN links l ON l.type = 'blocks' AND l.src = r.id
LEFT JOIN issues t ON t.id = l.dst AND t.status NOT IN ('done', 'cancelled')
  AND NOT EXISTS (
    SELECT 1 FROM links o JOIN issues ob ON ob.id = o.src
    WHERE o.type = 'blocks' AND o.dst = t.id AND o.src <> r.id
      AND ob.status NOT IN ('done', 'cancelled'))
GROUP BY r.id
";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = Path::new(&args[2]);
    let db = dir.join("tm.db");
    match args[1].as_str() {
        "seed" => seed(&db, args[3].parse().unwrap()),
        "list" => list(&db),
        "open" => open(&db),
        "ready" => ready(&db),
        "errors" => errors(&db),
        "undo" => undo(&db),
        other => panic!("unknown mode {other}"),
    }
}

fn status_done(project: tm_core::ProjectId, id: Uuid) -> Draft {
    Draft {
        actor: "probe".into(),
        project,
        subject: id,
        change: Change::SetField {
            field: "status".into(),
            label: String::new(),
            value: serde_json::json!("done"),
            value_label: None,
        },
    }
}

fn seed(db: &Path, n: usize) {
    let mut store = Store::open(db).unwrap();
    let (project, _) = store.create_project("probe", "PR", "Probe").unwrap();
    let mut ids = Vec::with_capacity(n);
    let mut transactions = 1;
    for i in 0..n {
        let (id, _) = store
            .create_issue("probe", project, "task", &format!("Issue {i}"))
            .unwrap();
        transactions += 1;
        if i % 5 == 0 && i > 0 {
            store.link("probe", "blocks", ids[i - 1], id).unwrap();
            transactions += 1;
        }
        if i % 3 == 0 {
            store.append(status_done(project, id)).unwrap();
            transactions += 1;
        }
        ids.push(id);
    }
    let conn = Connection::open(db).unwrap();
    let (ops, stamps): (i64, i64) = conn
        .query_row(
            "SELECT count(*), count(DISTINCT utc_ms) FROM ops",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    eprintln!(
        "seeded {n} Issues: {transactions} write transactions, {ops} operations, {stamps} distinct utc_ms"
    );
}

fn list(db: &Path) {
    let t0 = Instant::now();
    let store = Store::open(db).unwrap();
    let opened = t0.elapsed();
    let ids = store.find("", Context::default()).unwrap();
    let found = t0.elapsed();
    let views: Vec<_> = ids
        .iter()
        .map(|id| store.issue(*id).unwrap().unwrap())
        .collect();
    let read = t0.elapsed();
    let json = serde_json::to_vec(&views).unwrap();
    let encoded = t0.elapsed();
    std::io::stdout().write_all(&json).unwrap();
    eprintln!(
        "list: {} Issues, {} bytes; open {:.2} ms, find {:.2} ms, issue() x{} {:.2} ms, encode {:.2} ms, total {:.2} ms",
        ids.len(),
        json.len(),
        ms(opened),
        ms(found - opened),
        ids.len(),
        ms(read - found),
        ms(encoded - read),
        ms(encoded)
    );
}

/// The two statements of `Store::init` that read the store, copied from
/// lib.rs at 49b8560, each timed on its own (best of 5).
fn open(db: &Path) {
    let conn = Connection::open(db).unwrap();
    let ops: i64 = conn
        .query_row("SELECT count(*) FROM ops", [], |row| row.get(0))
        .unwrap();
    let best = |sql: &str| {
        (0..5)
            .map(|_| {
                let t = Instant::now();
                let _: i64 = conn.query_row(sql, [], |row| row.get(0)).unwrap();
                ms(t.elapsed())
            })
            .fold(f64::INFINITY, f64::min)
    };
    let scan = best(
        "SELECT coalesce(max(hlc), 0) FROM ops
         WHERE replica_id = (SELECT v FROM meta WHERE k = 'replica_id')",
    );
    let version = best("PRAGMA user_version");
    let plan: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT coalesce(max(hlc), 0) FROM ops
             WHERE replica_id = (SELECT v FROM meta WHERE k = 'replica_id')",
            [],
            |row| row.get(3),
        )
        .optional()
        .unwrap()
        .unwrap_or_default();
    eprintln!(
        "open: {ops} operations; max(hlc) for this Replica {scan:.3} ms (plan: {plan}); PRAGMA user_version {version:.3} ms"
    );
}

fn ready(db: &Path) {
    let mut conn = Connection::open(db).unwrap();
    report(&conn, "as seeded");
    // A cycle a merge could leave: two todo Issues blocking each other.
    let tx = conn.transaction().unwrap();
    let pair: Vec<Vec<u8>> = tx
        .prepare(&format!("{READY} LIMIT 2"))
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for (src, dst) in [(&pair[0], &pair[1]), (&pair[1], &pair[0])] {
        tx.execute(
            "INSERT INTO links (type, src, dst) VALUES ('blocks', ?1, ?2)",
            [src, dst],
        )
        .unwrap();
    }
    report(&tx, "with a merged 2-cycle");
    tx.rollback().unwrap();
}

fn report(conn: &Connection, label: &str) {
    let t0 = Instant::now();
    let ready: i64 = conn
        .query_row(&format!("SELECT count(*) FROM ({READY})"), [], |row| {
            row.get(0)
        })
        .unwrap();
    let counted = t0.elapsed();
    let sql = UNBLOCKS.replace("READY_SQL", READY);
    let unblocking: i64 = conn
        .query_row(
            &format!("SELECT count(*) FROM ({sql}) WHERE unblocks > 0"),
            [],
            |row| row.get(0),
        )
        .unwrap();
    let total = t0.elapsed();
    eprintln!(
        "ready ({label}): {ready} ready, {unblocking} would unblock something; ready {:.2} ms, with unblock counts {:.2} ms",
        ms(counted),
        ms(total - counted)
    );
}

fn links(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM links", [], |row| row.get(0))
        .unwrap()
}

/// Two Issues that hold no Link of any type between them, checked in the table.
fn unlinked_pair(conn: &Connection, ids: &[Uuid]) -> (Uuid, Uuid) {
    for pair in ids.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let held: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM links WHERE (src = ?1 AND dst = ?2) OR (src = ?2 AND dst = ?1)",
                [a.as_bytes(), b.as_bytes()],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        if held.is_none() {
            return (a, b);
        }
    }
    panic!("every adjacent pair holds a Link");
}

/// What the store's errors look like today: `FilterError` serializes, the
/// other `StoreError` variants have only their Display text.
fn errors(db: &Path) {
    let mut store = Store::open(db).unwrap();
    for filter in ["stauts:todo", "status:doing", "title:x"] {
        match store.find(filter, Context::default()) {
            Err(tm_store::StoreError::Filter(err)) => {
                println!("{filter}: {}", serde_json::to_string(&err).unwrap());
            }
            other => println!("{filter}: {other:?}"),
        }
    }
    let ids = store.find("", Context::default()).unwrap();
    let project = store.issue(ids[0]).unwrap().unwrap().project;
    let missing = store.append_all(vec![Draft {
        actor: "probe".into(),
        project,
        subject: Uuid::now_v7(),
        change: Change::Create,
    }]);
    println!("create without fields: {}", missing.unwrap_err());
    store.take("probe", ids[1], "claude", None).unwrap();
    let held = store.take("probe", ids[1], "claude", None).unwrap_err();
    println!("second take: {held}");
    let conn = Connection::open(db).unwrap();
    let (a, b) = unlinked_pair(&conn, &ids);
    let before = links(&conn);
    let unlinked = store.unlink("probe", "blocks", a, b);
    println!(
        "unlink of an unheld Link: ok={}, links {before} -> {}",
        unlinked.is_ok(),
        links(&conn)
    );
}

/// Undo as "flip the last operation": a duplicate add's inverse is a remove,
/// which also takes away the Link the first add made.
fn undo(db: &Path) {
    let mut store = Store::open(db).unwrap();
    let ids = store.find("", Context::default()).unwrap();
    let conn = Connection::open(db).unwrap();
    let (a, b) = unlinked_pair(&conn, &ids);
    let before = links(&conn);
    store.link("probe", "blocks", a, b).unwrap();
    let after_first = links(&conn);
    store.link("probe", "blocks", a, b).unwrap();
    let after_duplicate = links(&conn);
    store.unlink("probe", "blocks", a, b).unwrap();
    println!(
        "link, duplicate link, flipped inverse: links {before} -> {after_first} -> {after_duplicate} -> {}",
        links(&conn)
    );
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
