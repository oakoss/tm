# Turso DB and Turso Sync: deep dive (2026-10-16)

Follow-up to [sync-engines.md](sync-engines.md). That pass ruled Turso out on "No multi-process access" and a dev-grade sync server. This pass checks those claims against current primary sources, then measures them.

**Scope of measurement.** macOS 27.0.1 (build 26A434), Apple Silicon arm64, APFS. `tursodb` CLI v0.8.3 (latest) and v0.9.0-pre.1 (prerelease) from the GitHub release tarballs (SHA-256 matched the published `.sha256` files). Sync client `@tursodatabase/sync` 0.8.3 on Node 24.22.0. Control runs used Apple's `/usr/bin/sqlite3` 3.54.0. **Not tested:** Linux, Windows, the `turso` Rust crate, MVCC / `BEGIN CONCURRENT`, partial sync, `remoteWritesExperimental`. Scripts are in `scripts/turso_exp/` (`scripts/`, `node/`); raw run output was not kept.

## TL;DR

- **The earlier "No multi-process access" claim is outdated, but not in a useful way.** Multi-process access shipped in 0.6.0 (2026-05-14) behind `--experimental-multiprocess-wal` / `.tshm`, and it is still labelled experimental and "not production ready". Under load on macOS it **panicked in 16 of 20 stress runs and lost acknowledged commits in 3 of 20, while `PRAGMA integrity_check` reported `ok`**. Even with only two processes, 4–5 of 10 runs hit a spurious open rejection. The same harness on stock `sqlite3` was clean.
- **Without the flag (the default), the database is still locked to a single process.** A second process gets `Locking error: Failed locking file 't.db'. File is locked by another process`.
- **`--readonly` works well in default mode.** It takes no lock, works in a read-only directory and under `sandbox-exec (deny file-write*)` against a live writer, and creates no sidecar files. It has two caveats: about 1 open in 300 fails with a short WAL read during a checkpoint (retry fixes it), and a long-lived read-only connection never sees new commits.
- **Sync server:** it still has no auth or TLS. Anyone who can reach the port can run arbitrary SQL on the server DB via `POST /v2/pipeline`, and I did exactly that. The docs call it a server "for development and testing".
- **A server-side trigger can assign sequence numbers.** I measured this: the server numbers issues in push-arrival order and clients converge on the server's numbers after pull. A trigger created by a client is **not** pushed to the server.
- **Conflict handling merged disjoint-column edits to one row.** When both replicas changed the same column, the last push won silently. Neither case reports a conflict.
- **Sync replicas are single-process even with `multiprocess_wal`.** A second process opening the replica fails with a lock error. This blocks the parallel-agents requirement more decisively than the `.tshm` bugs.
- **Verdict:** Turso is not usable as the tracker's multi-writer local store yet, and not as its sync engine without forking the server. The earlier recommendation (custom op log on plain SQLite) stands. Details in [Verdict](#verdict).

## What changed since the earlier pass

| Earlier claim | Status now | Evidence |
|---|---|---|
| v0.8.3 (2026-10-12), v0.9.0-pre.1 (2026-10-15) | Unchanged; still latest. Release notes for both are install boilerplate only; `CHANGELOG.md` on main stops at 0.8.0 (2026-09-28) | `gh release list -R tursodatabase/turso` |
| manual.md: "No multi-process access" | Still present, but it sits in a stale Limitations list (see below). Multi-process access exists as an experimental flag | [manual.md](https://github.com/tursodatabase/turso/blob/849e59960d662bf4d6fbc1756e04486043c89375/docs/manual.md) |
| COMPAT.md: mixed SQLite/Turso multi-process unsupported | Unchanged: "4. We don't support mixed SQLite and Turso in multi-process scenarios." | [COMPAT.md](https://github.com/tursodatabase/turso/blob/849e59960d662bf4d6fbc1756e04486043c89375/COMPAT.md) |
| `.tshm` experimental | Unchanged, and measured as unsafe under load (below) | multiprocess-access.mdx, experiments |
| MVCC "not production ready" | Sources disagree. A 2026-09-22 commit removed the "experimental" wording from the MVCC section, but the journal-mode table still says "not production ready". MVCC cannot be combined with multi-process WAL | see below |
| `--sync-server` has no auth/TLS | Unchanged. It now also has `--sync-dir` (2026-09-10) to serve many databases | `cli/sync_server.rs`, docs |
| Turso Sync "Beta" (2025-10-08) | No GA announcement found. The 2026-07-27 local-first guide gives no status label; docs pages give none | blog, docs |
| Last push wins, row level | Last push wins, but disjoint-column edits to one row merged in my test | experiment 4 |
| (new) | Turso is joining Supabase (2026-10-02): "Turso Database remains open source and actively developed." | [blog](https://turso.tech/blog/turso-is-joining-supabase) |

All repo links are pinned to main at `849e5996` (committed 2026-10-16T17:21:11Z).

## Current primary-source wording

### Multi-process access

README, "experimental features" list ([README.md](https://github.com/tursodatabase/turso/blob/849e59960d662bf4d6fbc1756e04486043c89375/README.md)):

> **Multi-process WAL coordination** via the `.tshm` sidecar for cross-process WAL readers and writers.

manual.md, CLI flags table:

> `--experimental-multiprocess-wal` | Enable experimental multi-process WAL coordination via the `.tshm` sidecar. Supported on 64-bit Unix, and on 64-bit Windows when used with `--vfs experimental_win_iocp`. **Note:** the feature is not production ready so do not use it for critical data right now.

manual.md, "Limitations" (still on main):

> - No multi-process access
> - No multi-threading
> - No savepoints
> - No triggers
> - No views
> - No vacuum

This list is stale as a capability description. COMPAT.md line 116 has `CREATE TRIGGER | ✅ Yes`, and triggers worked in experiment 4. The flag table in the same file documents multi-process access. So the "No multi-process access" line is outdated wording. The measurements below, not the doc, are the reason to still treat multi-process access as unavailable.

[docs/sql-reference/multiprocess-access.mdx](https://github.com/tursodatabase/turso/blob/849e59960d662bf4d6fbc1756e04486043c89375/docs/sql-reference/multiprocess-access.mdx) (added 2026-04-21):

> Multi-process access is **experimental**. The on-disk coordination format and the public API may change between releases. Do not rely on the format for long-term storage across Turso versions.
>
> By default, a Turso database file is opened by a single OS process. [...] opening the same file from a second process is rejected with a locking error.
>
> - A **single-writer** slot. At most one process across the cluster holds the writer lock at any time.
>
> The feature is not yet supported under MVCC (`BEGIN CONCURRENT`). Use either MVCC within a single process or multi-process WAL with the default isolation model, not both.
>
> Read-only opens are advisory: if `multiprocess_wal` is requested on a read-only open and no `.tshm` file exists, Turso falls back to the legacy read-only WAL path rather than failing.

CHANGELOG history: "core: Implement initial support for multi-process database access" (0.6.0, 2026-05-14); "core: reject co-enablement of MVCC and multiprocess WAL" (0.7.0, 2026-07-13); "core/wal: fix multiprocess database file reader race" (0.8.0, 2026-09-28).

Open issues labelled `multiprocess` (17 open, 2 closed, checked 2026-10-16). The ones I reproduced or that match my results:

- [#9362](https://github.com/tursodatabase/turso/issues/9362) (2026-09-25) "multiprocess_wal: a process opening without the flag is not rejected, and its committed writes are lost". Its body says: "Under concurrent load, the processes that *do* use the flag lose committed rows too." **Matches my measurement.**
- [#7213](https://github.com/tursodatabase/turso/issues/7213) (2026-05-21) "Multiprocess WAL panic: reader slot released by non-owner". I hit the closely related `shared owner slot released by non-owner` panic, from `core/storage/shared_wal_coordination.rs:1637`.
- [#9005](https://github.com/tursodatabase/turso/issues/9005) (2026-09-15) panic "shared WAL frame ids must increase monotonically". **Reproduced** on both versions.
- [#8536](https://github.com/tursodatabase/turso/issues/8536) (2026-08-27) "empty WAL + leftover -tshm fails open with short read on WAL frame". I saw this error once, but only after my own `sqlite3` check had opened those files read-write, so I don't count it as reproduced. Reopening untouched crashed files worked.
- [#7340](https://github.com/tursodatabase/turso/issues/7340) (2026-06-03) pyturso multiprocess fails with WAL lock on macOS. Not reproduced with the CLI.
- [#4627](https://github.com/tursodatabase/turso/issues/4627) "Queries in a readonly DB don't see concurrent updates to the DB". **Reproduced** (experiment 2b).
- [#6246](https://github.com/tursodatabase/turso/issues/6246) Rust SDK `Builder` cannot open read-only. The CLI's `--readonly` can, which is what I tested.
- [#1853](https://github.com/tursodatabase/turso/issues/1853) "Multi-process support" (2025-06-27) is still open.

Several open PRs are actively working on the shared WAL index: #8968, #8967, #9222, #8476, #9360, #8851 (all updated 2026-09-26 to 2026-10-16).

### MVCC / BEGIN CONCURRENT

Commit [`0325d327`](https://github.com/tursodatabase/turso/commit/0325d32770ae4f6bd21f3b54c61bc4d152a52d26) (2026-09-22), "docs: stop calling MVCC experimental":

> MVCC has been a supported journal mode since the experimental_mvcc flag was renamed to mvcc. [...] Only passive checkpointing is still behind an experimental flag

manual.md "MVCC limitations" now reads "MVCC is a supported journal mode." The journal-mode table in the same file still says:

> `mvcc` | [...] **Note:** the feature is not production ready so do not use it for critical data right now.

COMPAT.md still describes "experimental MVCC mode" with a durability gap: "a write that reported success is not durable while sibling statements remain active". The [0.8 blog post](https://turso.tech/blog/turso-0.8.0) (2026-09-29) is about `BEGIN CONCURRENT` performance and makes no production-readiness claim. There are many open MVCC issues, for example #8447 "split brain as we allow rollback to before a commit finishes" and #9505.

MVCC cannot be combined with multi-process WAL, so `BEGIN CONCURRENT` does nothing for the tracker's many-processes requirement. I did not test it.

### Sync server

[cli/sync_server.rs](https://github.com/tursodatabase/turso/blob/849e59960d662bf4d6fbc1756e04486043c89375/cli/sync_server.rs) is a raw `std::net::TcpListener`. `parse_route` accepts `OPTIONS *`, `POST /v2/pipeline`, `POST /pull-updates`, and the `/db/{name}/…` variants for `--sync-dir`. Searching the file for `auth`, `token`, `bearer` and `tls` finds nothing. `handle_pipeline` runs each Hrana `execute`/`batch` request on the server's connection.

[docs.turso.tech/sync/local-sync-server](https://docs.turso.tech/sync/local-sync-server.md):

> Use the Turso CLI as a local sync server for development and testing without Turso Cloud.
>
> Point your sync client at `http://localhost:8080`. No auth token is needed for the local server.

[Conflict resolution](https://docs.turso.tech/sync/conflict-resolution) says Turso Sync "uses a **last push wins** strategy" and does not state the granularity. The [local-first guide](https://turso.tech/blog/building-local-first-apps-the-complete-guide-to-offline-first-database-sync) (2026-07-27) says: "Last-Push-Wins strategy at the row level"; "Fully distributed sync between devices is not supported"; "The remote Turso database is always the source of truth".

## Experiments

### 1. Concurrent writers from separate OS processes

**1a. Default mode (no flag):** 30 processes, each inserting 20 rows.

```text
$ bash scripts/mp_write.sh tursodb-0.8.3 runs/v0.8.3-noflag 30 20 noflag
procs=30 rows_per_proc=20 mode=noflag busy=0 elapsed=0s nonzero_exit=22
  22 Error: Locking error: Failed locking file 't.db'. File is locked by another process
expected=600 actual(same flag)=160
integrity=ok
```

The lock is exclusive per process. Each loser fails loudly, and no rows are lost silently.

**1b. `--experimental-multiprocess-wal`, no busy timeout:** 29 of 30 processes got `database is busy` (349 failed statements), and 251 rows landed. 251 + 349 = 600, so every failure was reported.

**1c. Multiprocess with `PRAGMA busy_timeout=5000`:** 30×20, clean on both versions (`expected=600 actual=600 integrity=ok`). A `t.db-tshm` file (86 KB) appears next to the database.

**1d. Stress test with acknowledged writes:** 50 processes × 200 rows, `busy_timeout=10000`, multiprocess flag on every process. After each autocommitted `INSERT`, the same process runs a separate `SELECT … WHERE proc=? AND n=?`, and the shell prints `ACK:p:n` only if the row reads back. After all processes exit, the db/wal/tshm files are copied before anything else opens them. Rows are counted with `tursodb` on that copy, and the ACK set is diffed against the rows present. (An earlier batch counted with `sqlite3` on the live files. I discarded it because a `sqlite3` read-write open could have altered the WAL.)

```text
$ bash scripts/mp_ack.sh tursodb-<v> runs/ack2-<v>-<i> 50 200      # 10 runs per version
```

| Version | Runs | Runs with a panic | Total panics | Spurious mode rejections | Runs losing ACKed rows | ACKed rows lost |
|---|---|---|---|---|---|---|
| 0.8.3 | 10 | 7 | 59 | 4 | 2 (run 4: 545; run 6: 82) | 627 |
| 0.9.0-pre.1 | 10 | 9 | 69 | 3 | 1 (run 1: 15) | 15 |
| `sqlite3` 3.54.0 control (same workload, `.timeout 10000`) | 3 | 0 | 0 | n/a | 0 | 0 (10000/10000 each run, no errors) |

Error kinds observed (numbers normalized):

```text
thread 'main' panicked at core/storage/shared_wal_coordination.rs:1637:9:
shared owner slot released by non-owner
thread 'main' panicked at core/storage/wal.rs:N:N:
shared WAL frame ids must increase monotonically: new_frame_id=N, previous_frame_id=N, slot=N, shared_max_frame=N
connection WAL position must not be behind the committed high-water mark | local_max_frame=N, authority_max_frame=N   (0.9.0-pre.1)
Runtime error: I/O error: short read on page N: expected N bytes, got N
Error: Locking error: Failed opening database 't.db'. Database is already open without experimental multiprocess WAL in another process
```

The worst run was 0.8.3 run 4:

```text
files after run: t.db=118784 t.db-tshm=167936 t.db-wal=0
tursodb count on copy: 9449
tursodb view: acked_but_missing=545
expected=10000 acked=9994 in_db=9449 acked_but_missing=545 in_db_not_acked=0 nonzero_exit=2 ic=ok
```

In that run, two processes panicked with `shared owner slot released by non-owner`. The 545 lost rows came from 20 processes, and **19 of those exited 0**. Each lost row had been read back by its own process after commit, the WAL was 0 bytes afterwards (so the rows were not left uncheckpointed), and `PRAGMA integrity_check` returned `ok`. Run 4 is the unambiguous case. The other two lossy runs were different: a mass `shared WAL frame ids must increase monotonically` panic took down 49 of 50 processes (0.8.3 run 6: 49 of those; 0.9.0-pre.1 run 1: 48, plus one `connection WAL position must not be behind the committed high-water mark`). Those runs left a 4.4–4.8 MB WAL behind. "Lost" there means the row was ACKed by a process that later panicked and was not recovered on reopen. A default-mode open and a multiprocess-mode open of copies of the untouched files both returned the same counts (2061 and 1040). Every lossy run had at least one panic, and no run without a panic lost rows. My guess at the mechanism, a checkpoint or backfill working from a stale shared frame index, is **inferred** and unverified.

**1e. Lighter loads** (`scripts/light.sh`, 10 runs per shape):

```text
v0.8.3       procs=20 rows=5   expected_total=1000  acked_total=948  runs_with_failed_proc=8 panics=1 spurious_mode_rejections=10 acked_but_missing_total=0
v0.8.3       procs=5  rows=200 expected_total=10000 acked_total=9200 runs_with_failed_proc=4 panics=0 spurious_mode_rejections=4  acked_but_missing_total=0
v0.8.3       procs=2  rows=500 expected_total=10000 acked_total=7500 runs_with_failed_proc=5 panics=0 spurious_mode_rejections=5  acked_but_missing_total=0
v0.9.0-pre.1 procs=20 rows=5   expected_total=1000  acked_total=988  runs_with_failed_proc=3 panics=1 spurious_mode_rejections=2  acked_but_missing_total=0
v0.9.0-pre.1 procs=5  rows=200 expected_total=10000 acked_total=9200 runs_with_failed_proc=4 panics=0 spurious_mode_rejections=4  acked_but_missing_total=0
v0.9.0-pre.1 procs=2  rows=500 expected_total=10000 acked_total=8000 runs_with_failed_proc=4 panics=0 spurious_mode_rejections=4  acked_but_missing_total=0
```

Even **two processes that both pass the flag** hit `Database is already open without experimental multiprocess WAL in another process` in 4–5 of 10 runs. The losing process exits non-zero without writing anything, so nothing is lost, but every caller would need open-retry logic. Searching the issue tracker for that error string found nothing, so this looks unreported; it is a candidate issue to file.

### 2. Reader process while a writer process is active

A writer inserts 3000 rows over about 3 s. A separate process opens the DB 8 times during that window (`scripts/reader.sh`). Both versions behaved identically:

| Writer mode | Reader mode | Result |
|---|---|---|
| default | default | Every read: `Locking error: Failed locking file 't.db'. File is locked by another process` |
| default | `--readonly` | Works; counts rise during the write: `257 506 800 1100 1400 1700 2009 2300` |
| multiprocess | multiprocess | Works: `300 600 844 1100 …` |
| multiprocess | multiprocess + `--readonly` | Works |
| multiprocess | default `--readonly` | Every read: `Database is already open with experimental multiprocess WAL in another process` |

**2a. Stress for a short-lived `--readonly` reader in default mode.** The writer inserts 20000 rows with autocheckpoint on; a sandboxed reader (`deny file-write*`) opens, counts and closes, 300 times (`scripts/ro_stress.sh`):

```text
v0.8.3       run1: reads=300 ok=298 errors=2 regressions=0
v0.8.3       run2: reads=300 ok=299 errors=1 regressions=0
v0.9.0-pre.1 run1: reads=300 ok=299 errors=1 regressions=0
v0.9.0-pre.1 run2: reads=300 ok=299 errors=1 regressions=0
error: I/O error: short read on WAL frame at offset N: expected N bytes, got N
```

The failures are loud and retryable, and no count ever went backwards. I did not check row-content consistency beyond `count(*)`.

**2b. Long-lived `--readonly` connection.** One connection runs `SELECT count(*)` once a second for 5 s while the writer inserts 3000 rows:

```text
v0.8.3:       300 300 300 300 300
v0.9.0-pre.1: 300 300 300 300 300
```

The connection stays at its first snapshot (#4627), so readers must reopen to see new data.

**2c. Stock `sqlite3` against a live tursodb writer** (single observation; COMPAT.md says this is unsupported). `sqlite3 -readonly` got `database is locked (5)` on all 5 attempts. A `sqlite3` write with `.timeout 2000` returned rc=0, and the row persisted (`final tursodb: 3001|1`). I did not establish whether it ran concurrently or after the writer finished.

### 3. Read-only directory and sandboxed reader

The DB was created and closed with tursodb, then `chmod 555` on the directory and `444` on the files. I tested once with the 0-byte `-wal` present and once with it deleted:

```text
=== v0.8.3 keepwal before: t.db t.db-wal
  --readonly: 2
  default:    Error: I/O error (open): permission denied
  mp+ro:      2
  after: t.db t.db-wal          (no new files)
=== v0.8.3 nowal before: t.db
  --readonly: 2
  default:    Error: I/O error (open): permission denied
  mp+ro:      2
  after: t.db                   (no new files)
```

0.9.0-pre.1 gave identical results. `--readonly` creates no sidecar files. With a `.tshm` absent, multiprocess + readonly falls back to the legacy path, as the doc says. Passing a `file:…?immutable=1` URI to the CLI failed with `permission denied`. My guess is that the CLI does not parse URIs and treats the string as a filename (**inferred**).

With `sandbox-exec -p '(version 1)(allow default)(deny file-write*)'` and a live writer holding 500 uncheckpointed rows in a 2 MB WAL (`scripts/sandbox_read.sh`):

```text
default writer:      sandboxed reader (--readonly): 500
                     sandboxed sqlite3 -readonly: database is locked (5)
mp writer:           sandboxed reader (--experimental-multiprocess-wal --readonly): Error: I/O error (open): permission denied
                     sandboxed reader (--experimental-multiprocess-wal):            Error: I/O error (open): permission denied
```

A sandboxed agent can read a live Turso database only in **default mode with `--readonly`**. Multiprocess readers need to write `.tshm`, so they fail in the sandbox.

### 4. `tursodb --sync-server` with two replicas

`node/sync_test.mjs` and `node/sync_test2.mjs` start `tursodb --sync-server 127.0.0.1:<port> server.db`, connect replicas A and B through `@tursodatabase/sync` 0.8.3, kill the server, write on both replicas, restart the server, then push and pull.

**4a. Offline push fails loudly:**

```text
A push while offline: ERROR sync engine operation failed: database sync engine error: fetch error: TypeError: fetch failed
```

**4b. Both replicas change `status` on one row (B also changes `title`); B pushes last.** B's later push overwrote A's `status='A-closed'`, with no error or conflict signal:

```text
A final: [{"id":"i1","title":"first (edited by B)","status":"B-wip",...}, ...]
```

**4c. Disjoint columns: A sets `status`, B sets `title`.** Both edits survive on the server and on both replicas:

```text
server final: i1|retitled-by-B|closed-by-A|1|server
```

So "last push wins" did not act on whole rows here. My reading, that push replays only the columns each UPDATE touched, is **inferred**. I ran this once per push order.

**4d. A trigger created by a client is not pushed.** A client ran `CREATE TRIGGER assign_seq AFTER INSERT ON issues WHEN NEW.seq IS NULL BEGIN UPDATE issues SET seq=(SELECT coalesce(max(seq),0)+1 FROM issues) WHERE id=NEW.id; END`. Afterwards the server's `sqlite_schema` had only `turso_sync_last_change_id`, `issues` and their autoindexes. The `seq` values the trigger set on the client came back `NULL` everywhere after sync.

**4e. A trigger created on the server assigns sequence numbers in push-arrival order.** The table, the trigger and one seed row were created through the server's own `/v2/pipeline` with an unauthenticated `fetch` (HTTP 200). Rows written into `server.db` with the plain CLI *before* the server started were **not** served: clients bootstrapped an empty schema (`no such table: issues`). After bootstrap, the trigger existed on both replicas, because pull copies pages. While offline, A inserted iA1 and iA2 and B inserted iB1, and each replica showed provisional local `seq` values.

```text
A local: iA1 seq=2, iA2 seq=3        B local: iB1 seq=2
A pushes first  -> server final: iA1=2, iA2=3, iB1=4
B pushes first  -> server final: iB1=2, iA1=3, iA2=4   (0.8.3 and 0.9.0-pre.1 servers, same result)
```

After pull, both replicas held the server's numbers. Even though the client's own trigger had set `seq`, the server trigger fired on the pushed inserts, which suggests push replays the original `INSERT` without changes made by triggers. That mechanism is **inferred** and undocumented, and it held in these tests only. Consequences for the tracker:

- An offline `CPL-n` shown to the user before sync is provisional and can change. This is the same as the custom design, which shows nothing until the server acks.
- The number arrives through `pull`, not in the push response. Push returns only `ok`.

**4f. A sync replica is single-process, even with `multiprocess_wal`** (`node/sync_mp.mjs`):

```text
== default
  child: ERROR sync engine operation failed: database error: Locking error: Failed locking file '.../a/local.db'. File is locked by another process
== multiprocess_wal
  child: ERROR sync engine operation failed: database error: Locking error: Failed locking file '.../a/local.db'. File is locked by another process
```

The flag created `local.db-tshm`, but the sync engine still holds an exclusive lock. A replica also creates `local.db-changes`, `local.db-info`, `local.db-wal` and `local.db-wal-revert`.

**4g. Auth.** No credential was sent at any point, including the `CREATE TABLE` / `CREATE TRIGGER` / `INSERT` calls in 4e. In practice, anyone who can reach the port can execute arbitrary SQL on the server database (**measured** for DDL and inserts; destructive statements not tried).

## Verdict

**As the local store for many parallel agent processes: no, not now.** In default mode a second process cannot open the database at all. The opt-in multi-process mode is labelled experimental and not production ready, and on macOS it panicked under realistic contention, lost acknowledged commits without `integrity_check` noticing, and rejected even two well-behaved processes at open about half the time. Requirement 6 from sync-engines.md fails on measurement, not just on documentation.

**What does work today:** one long-lived writer process (a daemon that owns the file), with readers in other processes using default-mode `--readonly` short-lived connections that retry on a short WAL read. This also covers sandboxed agents, since it needs no writes and creates no sidecars. Agents' writes would then have to go through the daemon over IPC. That is a real architecture change, but the custom design already has a server-shaped component that could fill the role (**inferred**).

**As the sync engine: not without a fork.**

- Positive: the protocol does what the tracker's numbering needs. A server-side trigger assigned gap-free numbers in push order, and replicas converged on them.
- Blocking: sync replicas are strictly single-process. The server has no auth and executes arbitrary SQL from anyone who can reach it, so it would need at minimum an authenticating reverse proxy that also blocks `/v2/pipeline` for non-admin callers. The docs scope the server to "development and testing".
- Risky: conflict outcomes are silent. Same-column edits lose the earlier push with no signal, which conflicts with the "visible `rejected` state" goal. A `transform` hook exists to customise this; I did not test it.

**What would have to change for Turso to fit:**

1. Multi-process WAL fixed and graduated from experimental. Concretely: #9362, #7213, #9005 closed, plus the spurious open rejection fixed. Re-run `scripts/mp_ack.sh` and `scripts/light.sh` as the acceptance test, and get zero panics and zero lost ACKs across many runs, on Linux as well as macOS.
2. Sync replicas openable from more than one process, or an officially supported daemon pattern.
3. Auth and TLS on `--sync-server`, or a production self-hostable server.
4. Optionally, the server-assigned value returned in the push response instead of only via pull.

**Recommendation:** keep the custom op-log design from sync-engines.md. Keep Turso on the watch list. The shared-WAL PRs (#8968, #8967, #9222, #8476) show active work, and the Supabase acquisition says development continues. Re-run this harness at the next minor release.
