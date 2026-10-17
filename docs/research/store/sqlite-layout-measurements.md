# SQLite projection layout measurements

- Date: 2026-10-17
- Author: Claude (research agent), for tm-u8g.3
- Scope: Which projection layout and PRAGMAs the local store should use, measured at 10k, 50k and 200k Issues, with an operation log of 5 operations per Issue (1M operations at 200k).

## Question

The store keeps an append-only operation log plus projection tables in one SQLite file, written by short-lived `tm` processes under `BEGIN IMMEDIATE` ([ADR 0001](../../decisions/0001-local-store-is-plain-sqlite-without-a-daemon.md), [ADR 0005](../../decisions/0005-operation-log-is-the-source-of-truth.md), [ADR 0006](../../decisions/0006-operation-merge-ordering-and-versioning.md)). Which of these projection layouts should hold Issues, and what does each cost to read, write, rebuild and store?

- **A, EAV**: `issue_fields(issue_id, field, value)`, one row per field.
- **B, hybrid**: one row per Issue, typed built-in columns, Scheme (custom) fields in one JSON column with expression indexes on the filtered ones.
- **C, typed**: one row per Issue, every field a typed column.

ADR 0005 says "Every write costs an extra append; at tracker scale this is assumed negligible but has not been measured." This note measures it.

## Sources

- sqlite.org pages fetched 2026-10-17: [pragma.html](https://www.sqlite.org/pragma.html), [wal.html](https://www.sqlite.org/wal.html), [lang_transaction.html](https://www.sqlite.org/lang_transaction.html), [foreignkeys.html](https://www.sqlite.org/foreignkeys.html), [expridx.html](https://www.sqlite.org/expridx.html), [gencol.html](https://www.sqlite.org/gencol.html), [json1.html](https://www.sqlite.org/json1.html), [stricttables.html](https://www.sqlite.org/stricttables.html), [fts5.html](https://www.sqlite.org/fts5.html).
- Benchmarks run 2026-10-17 on an Apple M2 Max, 96 GiB RAM, macOS 27.0.1, internal SSD (APFS). Scripts were in a session scratchpad (`store-bench/`: `common.py`, `gen.py`, `rebuild.py`, `queries.py`, `writes.py`, `coldstart.py`, and a Rust crate `coldstart/` with binaries `coldstart` and `replay`). The scratchpad is not kept; the DDL, queries and loop shapes are reproduced under [Method](#method) so the runs can be repeated.

### SQLite versions

| Runner | SQLite | How measured |
|---|---|---|
| `sqlite3` CLI | 3.54.0 | `sqlite3 --version` → `3.54.0 2026-04-09 ...` |
| Python 3.15.0 `sqlite3` | 3.54.0 | `sqlite3.sqlite_version` → `3.54.0` |
| rusqlite 0.40.2 `bundled` (the repo's pin, via libsqlite3-sys 0.38.2) | 3.53.2 | `coldstart version` → `sqlite_version=3.53.2 foreign_keys=1 journal_mode(mem)=memory synchronous=2 busy_timeout=5000 fts5=1` |

Python and the CLI run 3.54.0; `tm` will ship 3.53.2. Every feature tested exists in both: STRICT from 3.37.0 (stricttables.html), generated columns from 3.31.0 (gencol.html), JSONB from 3.45.0 (json1.html), `->>` (version not checked; it ran on 3.54.0), FTS5 (`fts5=1` above). Speed differences between the two versions were not measured (inferred to be small).

## Findings

### Reads

Warm latency in one Python connection (`mode=ro`), median of ≥ 30 runs (up to 2000 or 1 s), after `ANALYZE`. Times include Python's per-row fetch cost, which dominates queries that return thousands of rows; the `LIMIT 50` and `count(*)` rows isolate SQLite's own cost. Median µs:

| Query (rows at 200k) | A 10k | B 10k | C 10k | A 50k | B 50k | C 50k | A 200k | B 200k | C 200k |
|---|---|---|---|---|---|---|---|---|---|
| `status=active AND priority<=2 ORDER BY priority`, all rows (15,938) | 3,250 | 1,004 | 997 | 18,682 | 6,715 | 6,666 | 84,730 | 32,771 | 29,479 |
| same, `LIMIT 50` | 2,643 | 42 | 42 | 14,611 | 41 | 42 | 71,595 | 42 | 43 |
| same, `count(*)` | 2,661 | 41 | 41 | 15,240 | 206 | 212 | 78,796 | 870 | 805 |
| `area=cli`, all rows (57,382; 29%) | 4,583 | 3,289 | 3,356 | 26,619 | 17,667 | 17,726 | 113,448 | 80,410 | 81,806 |
| `area=cli LIMIT 50` | 51 | 35 | 36 | 58 | 34 | 36 | 62 | 35 | 35 |
| `area=area20` (2,015; 1%) | 113 | 74 | 74 | 1,506 | 628 | 685 | 7,942 | 3,281 | 3,283 |
| `label=label10`, all rows (8,529) | 934 | 307 | 307 | 5,657 | 2,952 | 2,738 | 25,255 | 16,172 | 16,062 |
| `label=label10 LIMIT 50` | 51 | 36 | 36 | 61 | 35 | 37 | 64 | 37 | 36 |
| `path=src/watch.ts` (1,363) | 83 | 55 | 55 | 1,038 | 220 | 215 | 5,388 | 2,760 | 2,274 |
| full Issue by id (row + labels + paths) | 33 | 22 | 23 | 36 | 22 | 25 | 39 | 26 | 26 |
| list page: 50 newest not-done | 305 | 67 | 92 | 333 | 71 | 94 | 329 | 67 | 94 |

STRICT and JSONB variants at 200k (median µs):

| Query | C | C STRICT | B STRICT text JSON | B STRICT JSONB |
|---|---|---|---|---|
| status/priority `LIMIT 50` | 43 | 42 | 42 | 43 |
| `area=cli` all rows | 81,806 | 72,315 | 75,654 | 73,068 |
| full Issue by id | 26 | 27 | 24 | 25 |
| list page of 50 | 94 | 92 | 70 | 76 |

- **A loses every multi-field filter.** `status=active AND priority<=2` needs a self-join per field, and EXPLAIN shows `USE TEMP B-TREE FOR ORDER BY`, so even `LIMIT 50` reads all 39,865 active Issues: 71.6 ms at 200k against 42 µs for B and C, which walk `issues_status_prio (status=? AND priority<?)` in order. Its list page needs a pivot with `USE TEMP B-TREE FOR GROUP BY` (329 µs against 67–94 µs). Plans are in [Raw data](#raw-data).
- **B and C read at the same speed** wherever an index applies; the difference is within run-to-run noise (C's `area=cli` varies 72–82 ms across runs). C's list page is ~25 µs slower only because it returns six custom columns against B's one JSON string.
- **STRICT costs nothing measurable** on reads or rebuild (C 38.8 s vs C STRICT 37.2 s).
- **JSONB is neutral on indexed reads** and slightly slower where the JSON is returned as text: `json(custom)` on the list page is 76 against 70 µs. In-memory microbenchmark over 200k rows (Python, 3.54.0): unindexed `json_extract` scan 17.8 ms JSONB vs 65.2 ms text; rendering all rows to text 107.5 ms (`json(b)`) vs 76.7 ms; `jsonb_set` 0.25 µs/row vs `json_set` 0.65 µs/row; 15.2 MB vs 21.2 MB stored.
- **Unindexed custom-field filters** (`estimate >= 8`, 40,189 rows, `count(*)`, 200k): C 49.1 ms, B JSONB 62.1 ms, B text 95.5 ms, all `SCAN issues`. A answers it from `issue_fields_fv` in 0.7 ms, its one advantage: every field is indexed.
- **All three layouts end in the same state**, a check on the replay: status counts at 200k are identical in A and C (`active 39865, backlog 59754, done 60257, review 10123, todo 30001`), and every query returns the same row count in A, B and C.

### Expression indexes, `->>` and generated columns

Measured at 200k (CLI 3.54.0 `EXPLAIN QUERY PLAN`) on B with `CREATE INDEX issues_c_area ON issues(json_extract(custom, '$.area'))`:

| WHERE clause | Plan |
|---|---|
| `json_extract(custom, '$.area')='cli'` | `SEARCH ... USING COVERING INDEX issues_c_area (<expr>=?)` |
| `json_extract(custom,'$.area')='cli'` (no space) | same, index used |
| `custom->>'area'='cli'` | `SCAN issues` |
| `custom->>'$.area'='cli'` | `SCAN issues` |
| `json_extract(custom, '$."area"')='cli'` | `SCAN issues` |
| generated column `g_area TEXT AS (json_extract(custom, '$.area')) VIRTUAL` + index, `WHERE g_area='cli'` | `SEARCH ... USING INDEX issues_g_area (g_area=?)` |

`->>` is 2× slower than the indexed form in the timings above (159.5 ms vs 80.4 ms at 200k). This matches the docs: the planner uses an expression index only when the query's expression is the same "exactly as it is written in the CREATE INDEX statement ... except for minor syntactic differences such as white-space changes" ([expridx.html](https://www.sqlite.org/expridx.html)). A VIRTUAL generated column removes the matching problem because queries name the column, and "an index that uses one or more VIRTUAL generated columns is an expression index" ([gencol.html](https://www.sqlite.org/gencol.html)). Its cost at 200k: `ALTER TABLE ... ADD COLUMN g_area ... VIRTUAL` 8 ms, `CREATE INDEX` 0.98 s (CLI `.timer`). STORED columns cannot be added by `ALTER TABLE ADD COLUMN`; VIRTUAL can (gencol.html).

STRICT enforces a generated column's declared type, even with no index (CLI 3.54.0, `:memory:`):

| Statement on `t(id INTEGER PRIMARY KEY, custom TEXT NOT NULL) STRICT` | Result |
|---|---|
| `ALTER TABLE t ADD COLUMN g_est INTEGER AS (json_extract(custom,'$.estimate')) VIRTUAL`, then insert `{"estimate":5}`, `{"estimate":"7"}`, `{}` | stored; `g_est` reads `integer 5`, `integer 7`, `null` |
| insert `{"estimate":"abc"}` | `cannot store TEXT value in INTEGER column t.g_est` |
| the same `ALTER TABLE` when a row with `"abc"` already exists | `constraint failed`, column not added |
| `g_est ANY AS (...) VIRTUAL` + index, insert `"abc"` and `5` | stored as `text abc` and `integer 5`; `WHERE g_est>=5` uses `SEARCH t USING INDEX ti (g_est>?)` |

So a typed generated column turns one malformed Scheme value into an unwritable Issue row, and blocks adding the column at all. Declaring generated columns `ANY` keeps the index without that coupling.

For C, adding a Scheme field is `ALTER TABLE issues ADD COLUMN risk TEXT` in 7 ms plus a 0.85 s index; `DROP COLUMN` rewrote the table in 0.64 s, and failed first with `error in index issues_c_risk after drop column` until the index was dropped.

### Writes

One `set_field` per `BEGIN IMMEDIATE ... COMMIT`, 1000 transactions on a copy of the 200k database (900 MB, WAL). Each transaction reads and advances the HLC row in a `meta` table (ADR 0006), inserts an operation row (16-byte `op_id`, 32-byte `op_hash`, JSON payload), resolves the Issue UUID through its unique index, updates the projection and upserts a `field_versions` row. Python, µs:

| Layout and setting | Field | Median | p95 | p99 | Mean |
|---|---|---|---|---|---|
| C, `synchronous=NORMAL` | status | 124 | 208 | 11,252 | 321 |
| C, `synchronous=FULL` | status | 158 | 266 | 10,404 | 342 |
| C, `FULL` + `fullfsync=ON` | status | 4,197 | 5,248 | 19,178 | 4,612 |
| C, `NORMAL` + `fullfsync=ON` | status | 123 | 207 | 19,932 | 538 |
| A, `NORMAL` | status | 128 | 213 | 11,455 | 327 |
| B, `NORMAL` | status | 124 | 241 | 11,354 | 332 |
| B, `NORMAL`, 0 JSON expression indexes | area | 118 | 191 | 10,930 | 299 |
| B, `NORMAL`, 3 JSON expression indexes | area | 131 | 216 | 11,482 | 329 |
| B, `NORMAL`, 6 JSON expression indexes | area | 141 | 240 | 12,049 | 354 |
| B JSONB, `NORMAL`, 3 indexes | area | 132 | 225 | 11,773 | 335 |
| B JSONB, `NORMAL`, 6 indexes | area | 140 | 235 | 11,671 | 350 |
| C, `NORMAL`, 3 column indexes | area | 126 | 220 | 11,736 | 337 |
| A, `NORMAL` | area | 132 | 236 | 11,747 | 341 |
| C, `NORMAL`, no FTS | title | 114 | 204 | 10,310 | 274 |
| C, `NORMAL`, FTS5 external content + triggers | title | 172 | 358 | 9,821 | 403 |
| C, `NORMAL`, FTS5 (trigger is `AFTER UPDATE OF title, description`) | status | 136 | 304 | 13,048 | 389 |

- **Layout barely moves write cost.** A, B and C are within 10 µs of each other at the median. Each extra JSON expression index adds about 3–4 µs (118 → 131 → 141 µs for 0 → 3 → 6), whether the JSON is text or JSONB; `json_set` itself is under 1 µs (microbenchmark above).
- **The durability setting dominates.** On macOS, `synchronous=FULL` alone adds ~34 µs. A plain `fsync` there does not reach permanent storage; `man fsync` on this machine says "The F_FULLFSYNC fcntl asks the drive to flush all buffered data to permanent storage." With `fullfsync=ON` (`F_FULLFSYNC`) a commit costs 4.2 ms, 34× more. Linux was not measured; there `fsync` is a real flush and FULL's cost is expected to sit between these (inferred).
- **p99 is the WAL checkpoint** (~10–12 ms; ~20 ms with `fullfsync` or `checkpoint_fullfsync`), paid by whichever commit pushes the WAL past 1000 pages. Inferred from the docs ("SQLite will automatically checkpoint whenever a COMMIT occurs that causes the WAL file to be 1000 pages or more", [wal.html](https://www.sqlite.org/wal.html)); the run did not trace which commits checkpointed.
- **FTS5 adds ~58 µs per title write** and ~12 µs to writes that do not touch title or description. Python's per-statement overhead is included in all of these.

The append ADR 0005 calls "assumed negligible": the operation insert, HLC update and `field_versions` upsert together are the whole transaction above, about 120 µs median at `NORMAL`, under 1 ms in every configuration except `fullfsync`.

### Rebuild (`tm rebuild`)

Replay of the operation log into empty projection tables in one transaction. Operation mix (seed 42): 200,000 `create`, 600,020 `set_field`, 159,953 `add`, 40,027 `remove`. Seconds:

| Engine | Layout | How | 10k / 50k ops | 50k / 250k ops | 200k / 1M ops |
|---|---|---|---|---|---|
| Python, default 2 MB cache | A | in place: WAL, `NORMAL`, indexes kept | 1.96 | 12.06 | 58.8 |
| Python, default cache | B | in place | 1.28 | 8.39 | 40.0 |
| Python, default cache | C | in place | 1.21 | 8.58 | 38.8 |
| Python, default cache | C STRICT | in place | | | 37.2 |
| Python, default cache | B STRICT JSONB | in place | | | 39.9 |
| Python, default cache | C + FTS5 triggers | in place | | | 62.0 |
| Python, default cache | A | fresh file, `journal_mode=OFF`, `synchronous=OFF`, indexes after | | | 28.0 |
| Python, default cache | B | fresh file | | | 21.9 |
| Python, default cache | C | fresh file | | | 22.0 |
| Python, default cache | C + FTS5 `'rebuild'` after | fresh file | | | 22.6 |
| Rust 3.53.2, 2 MB cache | C | in place, indexes kept | | | 33.6 |
| Rust, 64 MB cache | C | in place, indexes kept | | | 20.4 |
| Rust, 1 GB cache | C | in place, indexes kept | | | 14.1 |
| Rust, 1 GB cache | C | in place, drop indexes, replay, recreate | | | 12.0 |
| Rust, 1 GB cache | B | in place, drop and recreate indexes | | | 12.6 |
| Rust, 64 MB cache | C + FTS5 | in place, triggers firing | | | 41.6 |
| Rust, 1 GB cache | C + FTS5 | drop triggers and indexes, replay, recreate, `'rebuild'` (1.22 s) | | | 14.5 |
| Rust, 64 MB cache | C + FTS5 | same | | | 18.8 |

Copying 1M operations into a fresh file took 1.1–1.3 s, included in neither column. The Rust replay checks out: on the 10k log it produced the same counts as the Python replay (`10000|24952|10000|1945`, 26,094 labels, 179,682 `field_versions` rows), and FTS5 `integrity-check` passed after the drop-and-rebuild run.

- **Page cache size, not language, is the first lever.** Rust with SQLite's default 2 MB cache took 33.6 s, only 13% faster than Python's 38.8 s; the same Rust replay with a 1 GB cache took 14.1 s. Rebuild time grows faster than the log (Python C: 1.2 s → 8.6 s → 38.8 s for 5× and 4× more operations) once the working set leaves the cache.
- **Defer secondary indexes and FTS triggers**, then rebuild them: 14.1 → 12.0 s without FTS, 41.6 → 14.5 s with it. FTS5's own `'rebuild'` over 200k titles and descriptions is 1.2 s.
- **A rebuilds 1.3–1.5× slower** than B and C (14 rows per Issue).
- **In-place rebuild blocks writers for its whole duration.** During a Rust rebuild on a copy of the 200k database, a reader got `rows 39865 ms 1.6` (the pre-rebuild snapshot), and a writer with `busy_timeout=5000` failed with `OperationalError database is locked after s 5.22`. The rebuild took 11.7 s.

### Database size

From `dbstat` after `ANALYZE` and a TRUNCATE checkpoint. MB:

| Issues | Operation log + its indexes | A projection | B projection | C projection | C file |
|---|---|---|---|---|---|
| 10k | 26.3 | 32.2 | 18.3 | 17.8 | 44.2 |
| 50k | 132.6 | 162.8 | 93.1 | 90.6 | 223.3 |
| 200k | 531.8 | 659.4 | 378.1 | 367.8 | 899.7 |

At 200k: `issues` 158.6 MB (descriptions average ~550 bytes, stored inline), `field_versions` 116.1 MB (3.6M rows: one per field per Issue plus one per set element ever touched, about 32% of C's projection), FTS5 adds 46.0 MB (`issues_fts_data`). A's `issue_fields` plus its value index is 460.7 MB. The operation log is ~59% of the file, mostly create payloads carrying descriptions.

### Cold start

Spawn, open, set pragmas, read `user_version`, run the indexed status/priority query with `LIMIT 50`, exit. Median of 60 runs after 5 warm-up runs, warm OS page cache (a truly cold cache was not measured). ms:

| Command | 10k A | 10k B | 10k C | 200k A | 200k B | 200k C |
|---|---|---|---|---|---|---|
| Rust binary, no-op (process baseline) | 3.32 | | | | | |
| `sqlite3 -init /dev/null :memory: 'select 1'` (CLI baseline) | 4.56 | | | | | |
| Rust: open + `busy_timeout`, `synchronous`, `foreign_keys`, `user_version` | 4.12 | 4.20 | 4.41 | 4.24 | 4.31 | 4.38 |
| Rust: the same + query | 7.81 | 4.64 | 4.42 | 75.79 | 4.48 | 4.56 |
| `sqlite3` CLI: open + query | 10.05 | 5.66 | 5.78 | 80.39 | 5.85 | 5.63 |

In-process timing (`coldstart report`, 200k C): `open+pragmas+user_version=0 715.792µs total 804.792µs rows 50` (1.58 ms on the first of three runs). Opening costs ~1 ms on top of process start, independent of database size; B and C add ~0.2 ms for the query, and A's join costs 72 ms at 200k.

### Projection schema stamp

- `PRAGMA user_version` is "the user-version integer at offset 60 in the database header" and "SQLite makes no use of the user-version itself" ([pragma.html](https://www.sqlite.org/pragma.html)). Reading it took 2.14 µs per call; `SELECT v FROM meta WHERE k='projection_version'` took 2.22 µs (Python, 20,000 calls each). Both are free next to the ~1 ms open.
- `VACUUM INTO` keeps it: after `PRAGMA user_version=7; PRAGMA application_id=0x746d0001; VACUUM INTO 'uv2.db'`, the copy reported `user_version` 7, `application_id` 1953300481 (0x746d0001) and `journal_mode` `delete`. So ADR 0001's snapshot carries the stamp and comes out in rollback mode, which a read-only sandbox can open.
- A rebuild into a fresh file must set the stamp itself; nothing copies it.

### PRAGMAs and defaults

What the shipped library does without configuration, from `coldstart version` on rusqlite 0.40.2 bundled: `foreign_keys=1` (libsqlite3-sys's build passes `-DSQLITE_DEFAULT_FOREIGN_KEYS=1`), `synchronous=2` (FULL), `busy_timeout=5000`. The `sqlite3` CLI reports `DEFAULT_FOREIGN_KEYS` as 0, so a debugging session in the CLI does not enforce foreign keys unless asked to.

| PRAGMA | Documented behavior (fetched 2026-10-17) | Measured here |
|---|---|---|
| `journal_mode=WAL` | "is persistent. If a process sets WAL mode, then closes and reopens the database, the database will come back in WAL mode"; "readers do not block writers and a writer does not block readers"; "WAL does not work over a network filesystem" ([wal.html](https://www.sqlite.org/wal.html)) | Readers ran during an 11.7 s rebuild |
| `synchronous=NORMAL` | "WAL mode is always consistent with synchronous=NORMAL, but WAL mode does lose durability. A transaction committed in WAL mode with synchronous=NORMAL might roll back following a power loss or system crash." ([pragma.html](https://www.sqlite.org/pragma.html)) | 124 µs median commit |
| `synchronous=FULL` | "FULL is atomic, consistent, isolated, and durable (ACID) in WAL mode"; "an additional sync operation of the WAL file happens after each transaction commit" (pragma.html) | 158 µs on macOS without `fullfsync` |
| `fullfsync`, `checkpoint_fullfsync` | Default off; "Only Mac OS X supports F_FULLFSYNC" (pragma.html) | 4.2 ms commit with `FULL` + `fullfsync` |
| `foreign_keys` | "disabled by default (for backwards compatibility), so must be enabled separately for each database connection" ([foreignkeys.html](https://www.sqlite.org/foreignkeys.html)); "a no-op within a transaction" (pragma.html) | On by default in the bundled build |
| `busy_timeout` | Sets the connection's busy handler (pragma.html). `BEGIN IMMEDIATE` "might fail with SQLITE_BUSY if another write transaction is already active"; a deferred transaction's upgrade to a write fails with SQLITE_BUSY when another connection has written ([lang_transaction.html](https://www.sqlite.org/lang_transaction.html)) | Writer gave up after 5.22 s against a rebuild |
| `optimize` | "Applications with short-lived database connections should run "PRAGMA optimize;" once, just prior to closing each database connection" and after `CREATE INDEX` (pragma.html) | Not measured |

## Conclusions

1. **EAV (A) is out.** It is 1,700× slower on the core multi-field filter at 200k with `LIMIT 50` (71.6 ms vs 42 µs), 3.5–5× slower on list pages, 1.8× larger, and 1.3–1.5× slower to rebuild. Its one win, indexed filters on any field, does not need EAV (below).
2. **B and C perform the same.** Reads, writes, size and rebuild time differ by less than the noise between runs. The choice between them is how Scheme fields evolve, not speed: C turns every Scheme change into DDL (instant `ADD COLUMN`, table-rewriting `DROP COLUMN` that fails while an index references the column); B holds Scheme fields as data, and gets typed, indexed access to any filtered field from a VIRTUAL generated column, which `ALTER TABLE` can add.
3. **Durability, not layout, sets write latency.** Every layout commits a `set_field` in ~120–140 µs at `NORMAL`. The ADR 0005 append is measured: the whole transaction stays under 1 ms except with `fullfsync`.
4. **Rebuild of 1M operations is 12–15 s** in Rust with a large page cache and deferred indexes, FTS5 included. It holds the write lock throughout, longer than the 5 s busy timeout.

## Implications / actions

Recommendation for tm-u8g.3 (a recommendation, not a decision):

- **Layout B, built as follows:**
  - `issues` as a STRICT rowid table with typed built-in columns and `custom TEXT NOT NULL` holding Scheme field values as text JSON.
  - Each filterable Scheme field gets a VIRTUAL generated column (`json_extract(custom, '$.<field>')`) declared `ANY`, plus an index, created when the Scheme declares it. Type checking stays in Rust: a typed generated column in a STRICT table rejects the whole row on one bad value and refuses `ADD COLUMN` over existing bad data (measured above). The filter compiler targets the column name, so neither `->>` nor a re-spelled path can silently miss the index.
  - Sets (labels, multi-selects, file paths, Links) go in `WITHOUT ROWID` tables keyed `(issue_id, value)`, with a reverse `(value, issue_id)` index.
  - Built-in multi-field filters get composite indexes, such as `(status, priority)` and `(updated_ms)`.
  - Prefer text JSON over JSONB: JSONB wins only on unindexed custom-field scans (62 vs 96 ms at 200k), is slower to render (`json()`), and is opaque in the `sqlite3` CLI; json1.html says "Applications should not use JSONB outside of SQLite". Revisit if unindexed custom filters turn out to be common.
- **PRAGMAs per connection:** `journal_mode=WAL` (persistent, set at creation), `synchronous=NORMAL`, `foreign_keys=ON`, set explicitly although the bundled build defaults to on, `busy_timeout` explicit, and `PRAGMA optimize` before close. `NORMAL` loses at most the last commits on power loss, never consistency (pragma.html), and every operation is also in the log, so a lost tail is a lost recent edit, not a broken projection. It also skips the `fullfsync` cost, which `FULL` needs on macOS to be truly durable. Decide this deliberately: a user who needs every acknowledged edit to survive power loss needs `FULL` plus `fullfsync` on macOS, at ~4 ms per write.
- **Schema stamp:** `PRAGMA user_version` holds the projection schema version, and `PRAGMA application_id` marks the file as tm's. On open, a mismatch triggers `tm rebuild`. This costs nothing measurable, survives `VACUUM INTO`, and needs no table.
- **`tm rebuild`:** one `BEGIN IMMEDIATE`, `cache_size` raised for the rebuild only (e.g. −1048576 KiB), drop secondary indexes and FTS triggers, replay, recreate them, FTS5 `'rebuild'`, `ANALYZE`, then bump `user_version` in the same transaction. Rebuild in place, not into a fresh file renamed over the live one: a process holding the old file open keeps writing to the unlinked inode, and those writes are lost (inferred from POSIX rename semantics, not tested here). The fresh-file path measured faster in Python (22 s vs 39 s), but the cache-size and deferred-index changes recover that in place.
- **FTS5:** an external-content table over `issues(title, description)` with the documented triggers ([fts5.html](https://www.sqlite.org/fts5.html)), so it updates in the same transaction (+58 µs per title write); the trigger must be `AFTER UPDATE OF title, description` so other writes pay only ~12 µs. Comments need their own FTS table (not measured).
- **ADR 0005's "has not been measured"** can cite this note.

## Open questions

- **Busy timeout vs rebuild:** a 12–15 s in-place rebuild at 200k Issues exceeds the 5 s `busy_timeout`, so concurrent agents fail during it. Options: a long timeout only while a rebuild marker is set, or a rebuild in batches (which would expose a half-built projection unless guarded).
- **`field_versions` is 32% of the projection** (116 MB at 200k) because set elements get a row each. Alternatives, unmeasured: per-element versions in the set tables themselves, or versions only for fields with a pending conflict window.
- **Inline descriptions slow scans:** unindexed custom-field scans (49–96 ms at 200k) read every description. Moving long text (description, plan) to a side table should shrink `issues` and speed scans (inferred, not measured).
- **Linux and cold page cache:** fsync cost on Linux ext4 and cold-cache open times were not measured; the sandbox VM from [linux-probes.md](../sync-engine-spike/linux-probes.md) could run `writes.py` unchanged.
- **Operation log size:** 532 MB at 1M operations, mostly create payloads with full descriptions. Compaction (ADR 0006) or payload compression may matter before projection layout does.
- **Foreign keys were not modeled.** The benchmark schema has no FK constraints, so the cost of `foreign_keys=ON` on writes is unmeasured, and an in-place rebuild that empties `issues` while child tables reference it needs a delete order or `PRAGMA defer_foreign_keys`.
- **The `->>` trap applies to any hand-written SQL** (the web UI, ad hoc `tm` queries). A test asserting `EXPLAIN QUERY PLAN` for each generated filter would catch a regression.

## Method

Op log, one `ops` table per ADR 0006 (`seq INTEGER PRIMARY KEY, op_id BLOB UNIQUE, v, replica_id, actor, project, issue_uuid, kind, hlc, base_seq, utc_ms, offset_min, server_seq, server_received_ms, payload BLOB, op_hash BLOB, chain_hash BLOB`, index `(issue_uuid, seq)`), payload as JSON text. Distributions (seed 42):

- Status: backlog 30, todo 15, active 20, review 5, done 30.
- Priority 0–4 (none, urgent, high, medium, low): weights 15, 5, 20, 35, 25.
- Type: 7 types.
- Assignee: 20 users, 70% assigned.
- Title: 4–10 words. Description: 20–160 words.
- Parent: 20% of Issues have one.
- Scheme fields:
  - `area`: 30 values, Zipf s=1.1, so `cli` ≈ 29% and `area20` ≈ 1%.
  - `severity`: Bugs only.
  - `estimate`: 60% set.
  - `due`: 30% set.
  - `sprint`: 50% set.
  - `component`: 20 values, 70% set.
- Labels: 0–5 per Issue from 200, Zipf.
- Paths: 0–6 per Issue, mostly distinct, plus `src/watch.ts` at 0.2% of draws.

Follow-up operations:

| Operation | Share |
|---|---|
| set status | 35% |
| set priority | 10% |
| set Assignee | 10% |
| set a Scheme field | 15% |
| set title | 5% |
| add label | 10% |
| remove label | 5% |
| add path | 10% |

Projection DDL (STRICT variants add `STRICT` and type `custom` as `TEXT` or `BLOB`):

```sql
-- shared by all layouts
CREATE TABLE issue_labels(issue_id INTEGER NOT NULL, label TEXT NOT NULL, PRIMARY KEY(issue_id, label)) WITHOUT ROWID;
CREATE INDEX issue_labels_label ON issue_labels(label, issue_id);
CREATE TABLE issue_paths(issue_id INTEGER NOT NULL, path TEXT NOT NULL, PRIMARY KEY(issue_id, path)) WITHOUT ROWID;
CREATE INDEX issue_paths_path ON issue_paths(path, issue_id);
CREATE TABLE field_versions(issue_id INTEGER NOT NULL, field TEXT NOT NULL, hlc INTEGER NOT NULL, seq INTEGER NOT NULL,
  PRIMARY KEY(issue_id, field)) WITHOUT ROWID;

-- A
CREATE TABLE issues(id INTEGER PRIMARY KEY, uuid BLOB NOT NULL UNIQUE, key TEXT NOT NULL UNIQUE);
CREATE TABLE issue_fields(issue_id INTEGER NOT NULL, field TEXT NOT NULL, value, PRIMARY KEY(issue_id, field)) WITHOUT ROWID;
CREATE INDEX issue_fields_fv ON issue_fields(field, value, issue_id);

-- B (C replaces `custom` with area, severity, estimate, due, sprint, component columns and plain indexes)
CREATE TABLE issues(id INTEGER PRIMARY KEY, uuid BLOB NOT NULL UNIQUE, key TEXT NOT NULL UNIQUE,
  type TEXT NOT NULL, title TEXT NOT NULL, description TEXT, status TEXT NOT NULL, assignee TEXT,
  priority INTEGER NOT NULL, parent_id INTEGER, created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
  archived INTEGER NOT NULL DEFAULT 0, custom NOT NULL);
CREATE INDEX issues_status_prio ON issues(status, priority);
CREATE INDEX issues_updated ON issues(updated_ms);
CREATE INDEX issues_c_area ON issues(json_extract(custom, '$.area'));      -- plus component, sprint
```

Replay applies `create` as one `issues` insert (A: one `issue_fields` row per non-null field) plus set rows and a `field_versions` row per field. `set_field` is `UPDATE issues SET <col>=?, updated_ms=?` (B Scheme fields: `custom=json_set(custom, '$.<f>', ?)`, or `json_remove` for null; A: upsert into `issue_fields`) plus a `field_versions` upsert. `add`/`remove` are `INSERT OR IGNORE`/`DELETE` on the set table, a `field_versions` row keyed `l:<value>` or `p:<value>`, and an `updated_ms` bump.

Queries (B/C form; A uses one self-join on `issue_fields` per field):

```sql
SELECT id, key, title, priority FROM issues WHERE status='active' AND priority<=2 ORDER BY priority;
SELECT id, key, title FROM issues WHERE json_extract(custom, '$.area')='cli';  -- C: area='cli'
SELECT i.id, i.key, i.title FROM issue_labels l JOIN issues i ON i.id=l.issue_id WHERE l.label='label10';
SELECT i.id, i.key, i.title FROM issue_paths x JOIN issues i ON i.id=x.issue_id WHERE x.path='src/watch.ts';
SELECT id, key, title, status, priority, assignee, custom FROM issues WHERE status<>'done' ORDER BY updated_ms DESC LIMIT 50;
-- full read: the issues row by id, then labels and paths by issue_id
```

Write transaction (Python, `autocommit=True`, explicit statements):

```sql
BEGIN IMMEDIATE;
SELECT v FROM meta WHERE k='hlc';            -- then hlc = max(hlc+1, now_ms << 16)
UPDATE meta SET v=? WHERE k='hlc';
INSERT INTO ops(op_id, v, replica_id, actor, project, issue_uuid, kind, hlc, base_seq, utc_ms, offset_min, payload, op_hash) VALUES(...);
SELECT id FROM issues WHERE uuid=?;
UPDATE issues SET status=?, updated_ms=? WHERE id=?;
INSERT INTO field_versions VALUES(?,?,?,?) ON CONFLICT(issue_id, field) DO UPDATE SET hlc=excluded.hlc, seq=excluded.seq;
COMMIT;
```

Commands behind each table, run from the scratch directory:

```sh
python3 gen.py 200000                                   # op log; also 10000, 50000
python3 rebuild.py 200000 C inplace                     # Python rebuild; A|B|C, inplace|fresh, [strict] [jsonb] [fts]
python3 queries.py db_200000_C_inplace.db C             # reads; B ... jsonb for the JSONB file
python3 writes.py db_200000_C_inplace.db C FULL status fullfsync   # writes; [idx=K] [jsonb] [fts]
CACHE_KIB=1048576 ./coldstart/target/release/replay rr.db C dropidx  # Rust rebuild on a copy; inplace|dropidx
python3 coldstart.py db_10000_A_inplace.db ... db_200000_C_inplace.db
./coldstart/target/release/coldstart report db_200000_C_inplace.db "<status/priority query> LIMIT 50"
sqlite3 -init /dev/null uv.db "PRAGMA user_version=7; PRAGMA application_id=0x746d0001; VACUUM INTO 'uv2.db';"
```

Rust probes: a scratch crate depending on `rusqlite = "=0.40.2"` (`bundled`) and `serde_json`, built with `RUSTUP_TOOLCHAIN=1.99.1 ~/.cargo/bin/cargo build --release --offline`.

## Raw data

`EXPLAIN QUERY PLAN` at 200k:

```text
A status_prio: SEARCH s USING COVERING INDEX issue_fields_fv (field=? AND value=?) | SEARCH t USING PRIMARY KEY (issue_id=? AND field=?)
               | SEARCH p USING PRIMARY KEY (issue_id=? AND field=?) | SEARCH i USING INTEGER PRIMARY KEY (rowid=?) | USE TEMP B-TREE FOR ORDER BY
A list50:      CO-ROUTINE page | SEARCH u USING COVERING INDEX issue_fields_fv (field=?) | SEARCH s USING PRIMARY KEY (issue_id=? AND field=?)
               | SCAN page | SEARCH f USING PRIMARY KEY (issue_id=?) | USE TEMP B-TREE FOR GROUP BY | USE TEMP B-TREE FOR ORDER BY
B status_prio: SEARCH issues USING INDEX issues_status_prio (status=? AND priority<?)
B area:        SEARCH issues USING INDEX issues_c_area (<expr>=?)
B area (->>):  SCAN issues
B list50:      SCAN issues USING INDEX issues_updated
C area:        SEARCH issues USING INDEX issues_c_area (area=?)
label:         SEARCH l USING COVERING INDEX issue_labels_label (label=?) | SEARCH i USING INTEGER PRIMARY KEY (rowid=?)
path:          SEARCH x USING COVERING INDEX issue_paths_path (path=?) | SEARCH i USING INTEGER PRIMARY KEY (rowid=?)
read by id:    SEARCH issues USING INTEGER PRIMARY KEY (rowid=?) | SEARCH issue_labels USING PRIMARY KEY (issue_id=?)
               | SEARCH issue_paths USING PRIMARY KEY (issue_id=?)
```

200k size breakdown (`SELECT name, sum(pgsize) FROM dbstat GROUP BY name`), MB: `ops` 477.5, `ops_issue` 29.1, `sqlite_autoindex_ops_1` 25.2; C: `issues` 158.6, `field_versions` 116.1, `issue_paths_path` 24.4, `issue_paths` 24.2, `issue_labels_label` 8.9; A: `issue_fields_fv` 243.7, `issue_fields` 217.0; FTS: `issues_fts_data` 46.0.
