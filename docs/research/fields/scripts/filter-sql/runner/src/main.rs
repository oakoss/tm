//! Usage: filter-sql-runner <setup.sql> <queries.sql> <scratch-dir> [N ...]
//!
//! Builds a store of N Issues per size with the bundled SQLite, then prints
//! each query's EXPLAIN QUERY PLAN, row count and run times.

use std::{env, fs, path::Path, time::Instant};

use rusqlite::{Connection, params_from_iter};

const RUNS: usize = 7;

struct Probe {
    name: String,
    filter: String,
    params: Vec<String>,
    sql: String,
}

fn parse_queries(text: &str) -> Vec<Probe> {
    let mut probes = Vec::new();
    for block in text.split("-- name: ").skip(1) {
        let mut lines = block.lines();
        let name = lines.next().unwrap().trim().to_owned();
        let mut filter = String::new();
        let mut params = Vec::new();
        let mut sql = String::new();
        for line in lines {
            if let Some(f) = line.strip_prefix("-- filter: ") {
                filter = f.to_owned();
            } else if let Some(p) = line.strip_prefix("-- params: ") {
                params = p.split('|').map(str::to_owned).collect();
            } else if !line.starts_with("--") {
                sql.push_str(line);
                sql.push('\n');
            }
        }
        probes.push(Probe {
            name,
            filter,
            params,
            sql: sql.trim().trim_end_matches(';').to_owned(),
        });
    }
    probes
}

fn plan(conn: &Connection, probe: &Probe) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {}", probe.sql))
        .unwrap();
    let rows = stmt
        .query_map(params_from_iter(&probe.params), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut depth = std::collections::HashMap::new();
    rows.iter()
        .map(|(id, parent, detail)| {
            let d = depth.get(parent).map_or(0, |d| d + 1);
            depth.insert(*id, d);
            format!("{}{}", "  ".repeat(d), detail)
        })
        .collect()
}

fn run(conn: &Connection, probe: &Probe) -> (usize, Vec<f64>) {
    let mut stmt = conn.prepare(&probe.sql).unwrap();
    let mut count = 0;
    let mut times = Vec::new();
    for i in 0..=RUNS {
        let start = Instant::now();
        let mut rows = stmt.query(params_from_iter(&probe.params)).unwrap();
        let mut n = 0;
        while rows.next().unwrap().is_some() {
            n += 1;
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        // The first run warms the page cache and is not reported.
        if i > 0 {
            times.push(ms);
        }
        count = n;
    }
    times.sort_by(f64::total_cmp);
    (count, times)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let setup = fs::read_to_string(&args[1]).unwrap();
    let probes = parse_queries(&fs::read_to_string(&args[2]).unwrap());
    let dir = Path::new(&args[3]);
    let sizes: Vec<u64> = args[4..].iter().map(|s| s.parse().unwrap()).collect();

    let version: String = Connection::open_in_memory()
        .unwrap()
        .query_row("SELECT sqlite_version()", [], |r| r.get(0))
        .unwrap();
    println!("sqlite_version() = {version}, {RUNS} timed runs after one warm-up\n");

    for n in sizes {
        let path = dir.join(format!("filter-{n}.db"));
        let _ = fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        let start = Instant::now();
        conn.execute_batch(&setup.replace("{N}", &n.to_string()))
            .unwrap();
        let counts: (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM issues), (SELECT count(*) FROM field_values),
                        (SELECT count(*) FROM set_members)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        println!(
            "=== N = {n}: issues {}, field_values {}, set_members {} (setup {:.0} ms)\n",
            counts.0,
            counts.1,
            counts.2,
            start.elapsed().as_secs_f64() * 1000.0
        );
        for probe in &probes {
            let (rows, times) = run(&conn, probe);
            println!("--- {} [{}]", probe.name, probe.filter);
            for line in plan(&conn, probe) {
                println!("    {line}");
            }
            println!(
                "    rows {rows}; ms min {:.3} median {:.3} max {:.3}\n",
                times[0],
                times[times.len() / 2],
                times[times.len() - 1]
            );
        }
    }
}
