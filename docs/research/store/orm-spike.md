# Would an ORM or query builder help tm-store?

- Date: 2026-10-31
- Author: Claude, for Jace Babin (TM-121)
- Scope: whether Diesel or sea-query would help tm-store more than they would cost, against today's rusqlite.

## Question

tm-store writes SQL by hand through rusqlite, and adding the `session` column in TM-116 touched six places. Would an ORM (Diesel, the closest Rust has to Drizzle) or a query builder (sea-query) catch such mistakes at compile time, and at what cost in binary size, start-up time, build time and rewriting?

## Sources

- Diesel 2.3.14 and sea-query 1.0.2 with sea-query-rusqlite 0.8.0, the current releases on crates.io (`cargo search`, `cargo info`).
- Diesel's README and guide drafts on raw SQL, `QueryableByName` and dynamic schemas: <https://github.com/diesel-rs/diesel/blob/main/README.md>, <https://github.com/diesel-rs/diesel/blob/main/guide_drafts/trait_derives.md>, <https://github.com/diesel-rs/diesel/blob/main/diesel_dynamic_schema/README.md>.
- sea-query's documentation on conditions and the SQLite builder: <https://deepwiki.com/SeaQL/sea-query/2.1-query-building-system>, <https://deepwiki.com/SeaQL/sea-query/4.2-sqlite-and-mysql-extensions>.
- Measurements from [scripts/orm](scripts/orm): one trivial program per library, each opening a store and reading one value, built with tm's release profile, and [bench.pl](scripts/orm/bench.pl) for start-up time.

## Findings

### What each can express

- **Diesel** declares each table once (`table!`) and checks every query built from it at compile time: column names, types and nullability. A filter added at runtime boxes the query (`into_boxed`). Anything the builder cannot say goes through `sql_query`, which Diesel's guide notes it cannot type-check, and whose rows are read by name. Triggers, `PRAGMA user_version` and schema DDL run as raw SQL.
- **sea-query** builds SQL at runtime with no schema: conditions combine with `Cond::all()` and `Cond::any()`, and its SQLite builder supports `EXISTS` subqueries but not `ANY` or `ALL`. It checks nothing at compile time; it replaces string assembly with a builder.

### What tm-store holds

- **Size:** 15,048 lines with 227 SQL calls (`execute`, `prepare`, `query_row` and kin) across 24 files.
- **SQLite-specific SQL:** triggers, `PRAGMA`, `ON CONFLICT`, `INSERT OR`, `json_each` and recursive queries appear in 11 of tm-store's files.
- **Shape:** the source of truth is an append-only operation log, replayed into projection tables with per-field last-writer-wins versions and a generic `field_values` table, not a set of entity tables.
- **Runtime filters:** `query.rs` (423 lines) builds every `tm list` filter at runtime from the filter language.

So most of the store would stay raw SQL under Diesel, unchecked. The typed part would be the log's insert and read and the simple projection reads.

### What they cost

Trivial program, tm's release profile (fat LTO, one codegen unit, stripped, `panic = abort`), Apple M2 Max:

| Library | Binary | Clean release build | Packages (`cargo tree -e normal`) | Start-up, median of 200 (p90) |
|---|---|---|---|---|
| rusqlite 0.40.2 | 2,032,544 B | 19 s | 10 | 7.58–7.64 ms (8.01–8.11) |
| Diesel 2.3.14 | 2,049,328 B (+0.8%) | 20 s | 19 | 7.58–7.64 ms (8.10–8.18) |
| sea-query 1.0.2 on rusqlite 0.38 | 2,131,840 B (+4.9%) | 21 s | 27 | 7.57 ms (7.97–7.98) |

Start-up differs by no more than the noise between two runs; process start and opening SQLite dominate. The bundled SQLite build dominates compile time too. A trivial program understates both libraries' cost: Diesel's binary size and compile time grow with each `table!` and query type. That growth was not measured (see Open questions).

### Fitting them in

- **Diesel** requires `libsqlite3-sys` below 0.39, and rusqlite 0.40.2 uses 0.38.2, so both can link the same SQLite in one binary. Only one crate may link `sqlite3`, so a newer rusqlite on another `libsqlite3-sys` would block this until Diesel follows.
- **sea-query-rusqlite** 0.8.0 depends on rusqlite 0.38. tm would have to step back from 0.40.2, or bind sea-query's values to rusqlite by hand.

## Conclusions

Neither earns its cost now. tm-store's work is an operation log and projections, written in SQL that leans on SQLite features. Diesel's compile-time checks would cover the smallest part of it, and the rest would move to `sql_query` with no checking and a second way of reading rows. sea-query would tidy `query.rs`'s string building but check nothing, and it pins an older rusqlite.

Neither costs anything at start-up, so that is not the reason. The reasons are what they can express and what moving 227 call sites would cost.

The mistake this spike started from, a row format repeated across files, was fixed more cheaply by putting it in one module (`log.rs`, TM-120).

## Implications / actions

- Keep rusqlite. No ADR is needed, since nothing changes.
- Keep each table's row format in one module, as `log.rs` does for `ops`, and let tests that read and write through it catch drift.
- Revisit sea-query only if `query.rs` grows hard to change, and only once its rusqlite binder supports the rusqlite tm uses.

## Open questions

- Diesel's compile-time and binary-size growth on a real slice of the store (the log's insert and replay read, typed through `table!`) was not measured. Writing that slice was interrupted twice, so it remains inferred from Diesel's design that it grows with the schema.
- A non-trivial `query.rs` rewrite on sea-query was not tried; how much shorter it would be is unknown.

## Raw data

To rerun: build each program in [scripts/orm](scripts/orm) with `cargo build --release`, make any SQLite file (`sqlite3 bench.db 'CREATE TABLE t (x)'`), then run `perl bench.pl bench.db basemin/target/release/basemin diesel/target/release/diesel seaq/target/release/seaq` from that directory. The table's ranges are two such runs.
