# Sync engine spike: how a local-first issue tracker should sync, state as of 2026-10-16

Method note: engine status (versions, licenses, last push, releases) came from `gh api`, `npm view`, crates.io and each project's docs on 2026-10-16, gathered by three research passes whose sources are cited inline. Nothing in the engine sections was installed or run; behaviour claims there are cited or labeled **inferred**. The SQLite sections are different: every behaviour claim there quotes a command run on this machine (macOS, Homebrew `sqlite3 3.54.0 2026-10-09`, Codex CLI 0.163.0). The experiment scripts are in `scripts/sqlite_exp/`.

## Takeaway

No off-the-shelf engine meets the full requirement set. The constraint that eliminates most of them is "a self-hostable authority, ideally the same binary, that assigns `CPL-n` at sync". Electric, PowerSync and Zero all need Postgres plus a separate service; sqlite-sync needs SQLite Cloud's coordinator and its license forbids a custom transport; CRDT libraries are leaderless by design; LiteFS/Litestream are single-writer replication. Turso DB sync comes closest (MIT, ships a sync server in the same `tursodb` binary) but its own docs say "No multi-process access", which breaks the parallel-agents requirement.

**Recommendation: the custom design.** Plain SQLite (WAL) as the local store, an append-only operation log plus materialized tables in the same file, HTTP push/pull to a small server that is the same binary in `serve` mode, orders ops, assigns `CPL-n` in the push transaction and returns it in the ack. Sandboxed agents read a published rollback-mode snapshot opened with `immutable=1`. This pushes the core toward **Rust** (single static binary for CLI + TUI + web UI + server, ~6 ms cold start class). Details and risks are at the end.

## Requirements used as the scoring rubric

1. Full local copy; offline reads and writes; fast cold start (agents run the CLI hundreds of times).
2. One optional, self-hostable authority server, ideally the same binary, that assigns sequential per-project numbers; offline-created issues carry a provisional ID until sync.
3. No git remote as transport.
4. Sync status verifiable against the server.
5. Sandboxed read-only agents (Codex `--sandbox read-only`) read the local store without writing or locking.
6. Many concurrent local writers without "database is locked".

## SQLite under a write-denying sandbox

### What the docs say

- WAL read-only access, [wal.html §5](https://www.sqlite.org/wal.html): since 3.22.0 (2018-01-22) a WAL database without write permission "can still be read as long as one or more of the following conditions are met: 1. The -shm and -wal files already exist and are readable. 2. There is write permission on the directory containing the database so that the -shm and -wal files can be created. 3. The database connection is opened using the immutable query parameter." The same page recommends converting to `journal_mode=DELETE` before putting a database on read-only media.
- `-wal`/`-shm` lifecycle, [walformat.html §1.4](https://www.sqlite.org/walformat.html): "if the last client did not call sqlite3_close() before it shut down, or if the last client to disconnect was a read-only client, then the final cleanup operation does not occur and the shm and wal files may still exist." `SQLITE_FCNTL_PERSIST_WAL` keeps them deliberately "to allow subsequent read-only clients to connect" ([walformat.html §1.5](https://www.sqlite.org/walformat.html)). PERSIST_WAL was not exercised here (the CLI has no switch for it); documented, not measured.
- Read-only WAL readers still lock: "All connections hold SQLITE_LOCK_SHARED continuously while attached to a WAL-mode database. This is true for both read/write connections and read-only connections" ([walformat.html §2.3.1](https://www.sqlite.org/walformat.html)). If `-shm` is not writable, the reader builds a "private heap-memory ersatz WAL-index" ([walformat.html §3](https://www.sqlite.org/walformat.html); `SQLITE_READONLY_CANTINIT` in [rescode.html](https://www.sqlite.org/rescode.html)).
- `immutable=1`, [uri.html](https://www.sqlite.org/uri.html): SQLite "skips all file locking and change detection on immutable database files. If [it] asserts that a database file is immutable and that file changes anyhow, then SQLite might return incorrect query results and/or SQLITE_CORRUPT errors." `mode=ro` only opens the file read-only; it does not stop SQLite creating sidecar files.
- Rollback mode: a hot journal left by a crashed writer gives `SQLITE_READONLY_ROLLBACK`, "a hot journal that needs to be rolled back but cannot because the database is readonly" ([rescode.html](https://www.sqlite.org/rescode.html)).
- `SQLITE_OPEN_READONLY` ([c3ref/open.html](https://www.sqlite.org/c3ref/open.html)) is assumed to be what `mode=ro` and the CLI's `-readonly` pass down (**inferred**, page not re-read for this note). In the probes below, `-readonly` alone and `-readonly` plus `mode=ro` gave identical results in every case.

### What the experiments printed

Fixture: table `i` with 2 committed rows (3 where noted). Two sandboxes: a Seatbelt profile `(allow default) (deny file-write*)` via `sandbox-exec` (`ro_test.sh`, `ro_test2.sh`), and Codex's own read-only sandbox, `codex sandbox -c 'sandbox_mode="read-only"' --` (`codex_test.sh`, `codex_test2.sh`). Sanity check under Codex: `touch: probe_file: Operation not permitted`.

| Store state | Sandbox (script) | `-readonly` / `mode=ro` | `immutable=1` | No flags (`sqlite3 t.db`, Python `sqlite3.connect`) |
|---|---|---|---|---|
| Rollback journal (`journal_mode=DELETE`), clean | Seatbelt (`ro_test.sh`) and Codex (`codex_test.sh`, `codex_test2.sh`) | `exit=0 -> 2` | `exit=0 -> 2` | Codex: `2` / `(2,)` |
| WAL, cleanly closed (Homebrew build deletes `-wal`/`-shm`) | Seatbelt and Codex (same scripts) | `unable to open database file (14)` | `exit=0 -> 2` | Codex: `unable to open database file (14)` / `sqlite3.OperationalError: unable to open database file` |
| WAL, `-wal`/`-shm` left by an earlier read-only open | Codex (`codex_test.sh`, `codex_test2.sh`) | `exit=0 -> 2` | `exit=0 -> 2` | Codex: `2` / `(2,)` |
| WAL, live writer holding 1 uncheckpointed row (3 committed) | Seatbelt (`ro_test.sh`) | `exit=0 -> 3` | `exit=0 -> 2` (stale, wrong) | not run |
| WAL, writer killed with `-9` (3 committed, 1 only in `-wal`) | Seatbelt (`ro_test2.sh`) | `exit=0 -> 3` | `exit=0 -> 2` (stale, wrong) | not run |
| WAL, writer mid-`BEGIN IMMEDIATE` (uncommitted row) | Seatbelt (`ro_test2.sh`) | `exit=0 -> 1` (correct snapshot) | not run | not run |
| Rollback, writer killed `-9` mid-txn (hot journal present) | Codex (`codex_test2.sh`) | `attempt to write a readonly database (8)` | `exit=0 -> 2` (correct here, not guaranteed: immutable ignores the journal, and uri.html warns of wrong results if the file changed) | not run |
| `chmod 555` dir + `444` db, WAL, no sidecars | Seatbelt (`ro_test.sh`) | `unable to open database file (14)` | `exit=0 -> 2` | not run |

One more measured detail, unsandboxed (`ro_test2.sh`): `sqlite3 "file:d_rofiles/t.db?mode=ro" ...` on a cleanly closed WAL db printed `before: t.db` and `after close: t.db t.db-shm t.db-wal`. A "read-only" open creates and leaves sidecar files when it can.

Linux (Codex uses Landlock/seccomp or bubblewrap there, per [developers.openai.com/codex/security](https://developers.openai.com/codex/security)) was not tested. Whether Landlock lets a read-only fd take the `fcntl` read locks SQLite uses on `-shm` is **inferred** to work but unverified.

### What this implies

- A live WAL store is readable by a sandboxed agent **only if `-wal` and `-shm` already exist**, and the reader still takes shared locks. Whether they exist depends on who closed last. That is fragile and does not meet "no lock".
- `immutable=1` on a live file is wrong: it returned 2 rows when 3 were committed, and the docs warn of `SQLITE_CORRUPT`.
- Rollback mode reads cleanly under the sandbox but is a poor live store: a crashed writer's hot journal locks sandboxed readers out (`attempt to write a readonly database (8)`), and it gives up WAL's "readers do not block writers and a writer does not block readers" ([wal.html §1](https://www.sqlite.org/wal.html)), so readers and committing writers contend (the reverse is **inferred** from that advantage list; not measured here).
- The only configuration that is write-free **and** lock-free: a **published snapshot** in rollback mode that is never modified in place, opened with `immutable=1`. `VACUUM INTO 'snap.tmp.db'` then `mv` to `snap.db` produced a `journal_mode` of `delete`, and took `real 0.06` on a 41 MB, 20,000-issue database (warm page cache, measured once). Readers that already have the old file open keep reading the old inode after the rename (POSIX semantics, **inferred** for this use, not tested).

Recommended layout (`.tracker/` or `$XDG_DATA_HOME/...`, name TBD):

```
store.db            live store, WAL; writers only
store.db-wal/-shm   managed by SQLite
snapshot.db         rollback-mode copy, replaced atomically by rename after each write batch
snapshot.meta       op sequence + server cursor the snapshot reflects (lets agents report staleness)
```

The CLI detects a read-only environment (open of `store.db` for write fails, or an explicit `--read-only`/env var) and falls back to `file:snapshot.db?immutable=1`. Writers keep `PERSIST_WAL` on as a second line, so a reader that does open `store.db` usually succeeds. (rusqlite has no first-class setter for it that this pass confirmed; it may need raw `sqlite3_file_control` through `rusqlite::ffi`, **inferred**.)

## Concurrency: many short-lived CLI writers

Docs: `BEGIN DEFERRED` starts a read transaction; "Subsequent write statements will upgrade the transaction to a write transaction if possible, or return SQLITE_BUSY" ([lang_transaction.html](https://www.sqlite.org/lang_transaction.html)). In WAL, a stale snapshot gives `SQLITE_BUSY_SNAPSHOT`, and "if [BEGIN IMMEDIATE] succeeds, then SQLite guarantees that no subsequent operations on the same database through the next COMMIT will return SQLITE_BUSY" ([rescode.html](https://www.sqlite.org/rescode.html)).

Experiment (`conc_test.sh`, `conc_test_split.sh`): 50 parallel shells × 10 `sqlite3` process launches each = 500 short transactions on one WAL db.

Single-statement transaction (`INSERT ... SELECT max(n)+1`):

```
BEGIN DEFERRED, busy_timeout=0    : rows=286 of 500, errors=214 e.g. 'database is locked'
BEGIN DEFERRED, busy_timeout=5000 : rows=500 of 500, errors=0
BEGIN IMMEDIATE, busy_timeout=0   : rows=228 of 500, errors=272
BEGIN IMMEDIATE, busy_timeout=5000: rows=500 of 500, errors=0
```

Read-then-write transaction (`SELECT max(n)` then `INSERT`), the shape of "read issue, then update it":

```
BEGIN DEFERRED, busy_timeout=0    : rows=289 of 500, errors=211
BEGIN DEFERRED, busy_timeout=5000 : rows=313 of 500, errors=187 e.g. 'database is locked'
BEGIN IMMEDIATE, busy_timeout=0   : rows=232 of 500, errors=268
BEGIN IMMEDIATE, busy_timeout=5000: rows=500 of 500, errors=0
```

`busy_timeout` alone cannot rescue a deferred read-to-write upgrade (187/500 failures). `BEGIN IMMEDIATE` + `busy_timeout` had 0 failures. The first script's four runs (2,000 process launches total) finished in 1.72 s wall on this machine, so serialized writes are not the bottleneck at agent scale. Rules for the core: every write transaction is `BEGIN IMMEDIATE`, `busy_timeout` ≥ 5 s, short transactions, no network I/O inside a write transaction (beads' auto-push held its lock while the network hung, per the sibling note `Task tracker landscape/agent_first_trackers.md`).

Daemon vs no daemon: a local daemon could serialize writes and own the sync loop, but Codex's read-only sandbox blocks Unix sockets by default (`codex sandbox --help` lists an opt-in `--allow-unix-socket`). Measured: with `nc -lU s.sock` listening, `echo hello | codex sandbox -c 'sandbox_mode="read-only"' -- nc -U s.sock` printed `sandboxed nc exit=1` and the listener received nothing, while the same command unsandboxed printed `unsandboxed nc exit=0` and the listener got `hello-unsandboxed-check`. A daemon-only read path would lock sandboxed agents out. So: the plain file is mandatory for reads, direct SQLite writes from each CLI process are sufficient (measured above), and a daemon is optional, useful only for background sync and snapshot publishing. The sync step itself can run in-process (`tracker sync`) or from the web UI server when it is running.

Cold start, measured (`bench.sh`, mean of 20 runs of a read-only `count(*)` on the 41 MB db, including ~1 ms loop overhead): `sqlite3: 6.3 ms/run`, `node: 28.5 ms/run` (`node:sqlite`, Node v24.22.0), `bun: 9.9 ms/run` (`bun:sqlite`, Bun 1.4.3). A Rust binary is **inferred** to land near the `sqlite3` figure; a TS CLI with real dependencies loaded will be slower than this bare Node number.

Which SQLite each runtime actually uses (`select sqlite_source_id()`): Bun printed `2026-04-09 12:25:13 8fa8248e…aapl`, identical to `/usr/bin/sqlite3`, so `bun:sqlite` on macOS links Apple's system library; Node printed `2026-07-24 19:02:57 bf7c7f30…` (its own bundled copy). Apple's build left `-wal`/`-shm` behind after a normal close in this session (first fixture, `ls` showed `t.db t.db-shm t.db-wal`), which Homebrew's did not. The sidecar lifecycle that the read-only story depends on therefore varies by runtime unless the app ships its own SQLite. `rusqlite` 0.40.2 has a `bundled` feature (`cargo info rusqlite -v`: `bundled = [libsqlite3-sys?/bundled, modern_sqlite]`; [README](https://github.com/rusqlite/rusqlite): "`bundled` uses a bundled version of SQLite").

## Footprint

| Option | Size | Source |
|---|---|---|
| Turso DB CLI (`tursodb`, includes sync server) | 5.5 MB compressed, macOS arm64 | [releases](https://github.com/tursodatabase/turso/releases) |
| sqlite-sync extension | ~270 KB compressed, macOS arm64 | [releases](https://github.com/sqliteai/sqlite-sync/releases) |
| cr-sqlite extension | 0.4–0.6 MB zip | [releases](https://github.com/vlcn-io/cr-sqlite/releases) |
| Litestream binary | ~13 MB compressed | [releases](https://github.com/benbjohnson/litestream/releases) |
| Automerge wasm | 3,644,225 B (1,136,563 B gzipped), measured by `npm pack` | research pass |
| Loro wasm / experimental pure-TS `loro.js` | 3,397,123 B (1,124,329 B gz) / 648,421 B (132,841 B gz), measured by `npm pack` | research pass |
| Yjs / `ywasm` | 300,059 B (62,692 B gz) / 998,270 B (340,948 B gz), measured by `npm pack` | research pass |
| Electric, PowerSync, Zero (server side) | Postgres plus a service container (Zero suggests 1 vCPU / 2 GB / 20Gi to start, [self-host](https://zero.rocicorp.dev/docs/self-host)); PowerSync adds bucket storage | docs |
| Custom design | Your binary; bundled SQLite adds roughly 1–2 MB (**inferred**, not measured) | — |

## Engine comparison matrix

| | Plain SQLite file stock `sqlite3` can read | Offline writes | Server assigns `CPL-n` at ingest and client gets it | Self-host / same binary | Many local writer processes | Verifiable sync status | Rust SDK | TS SDK | License |
|---|---|---|---|---|---|---|---|---|---|
| Turso DB + Turso Sync | Format is SQLite, but sidecars (`-info`, `-changes`, `-wal-revert`, MVCC `-log`) and mixed SQLite/Turso multi-process is unsupported | Yes | Not documented; server trigger + page-level pull (**inferred**) | `tursodb --sync-server`, same binary, but dev-grade (no auth/TLS) | No ("No multi-process access"; `.tshm` experimental) | `stats().revision`, push/pull times | `turso` crate 0.8.3 | `@tursodatabase/sync` 0.8.3 | MIT |
| libSQL embedded replica | Yes, but "Do not open the local database while ... syncing" | Opt-in `offline: true`, maturity unstated | Writes forwarded to primary when online (**inferred**) | `sqld` last release 2025-02-14 | WAL, but corruption warning during sync | Frame number (**inferred**) | `libsql` 0.10.0 | `@libsql/client` 0.18.0 | MIT |
| sqlite.ai sqlite-sync | Yes for reads; writes need the extension (**inferred**) | Yes (CRDT) | No; docs say use UUIDs, avoid autoincrement | No: SQLite Cloud coordinator required | If each process loads the extension (**inferred**) | `localVersion`/`serverVersion` | None (load C ext via rusqlite, **inferred**) | `@sqliteai/sqlite-sync` 1.2.0 | Elastic 2.0 + network-layer restriction |
| cr-sqlite | Yes for reads; writers need the extension (**inferred**) | Yes (CRDT) | You write the server, so yes | Build your own | If each loads the extension (**inferred**) | `db_version` | Core is Rust/C | `@vlcn.io/crsqlite` | MIT; dormant (last release 2024-01-17) |
| Electric | Not by itself (TS client is in-memory) | Via TanStack DB `offline-transactions` | Your write API (**inferred**) | Postgres 14+ + Elixir service | n/a | Not documented | None | `@electric-sql/client` 1.5.28 | Apache-2.0 |
| PowerSync | Likely for reads (JSON in `ps_data__*` behind views, **inferred**) | Yes (upload queue) | Yes, via your `uploadData` backend; syncs back at checkpoint | Source DB + bucket DB + Node service | Docs: "only instantiate one instance per file" | Checkpoint requests alpha, Swift-only | `powersync` 0.1.1 beta | `@powersync/node` 1.1.1 GA | Service FSL-1.1-ALv2; SDKs Apache-2.0 |
| Rocicorp Zero | No (KV chunk store) | **No** ("doesn't support offline writes") | Yes (server mutators may return different values) | Postgres + zero-cache | n/a | Not covered | None | `@rocicorp/zero` 1.11.0 | Apache-2.0 |
| LiteFS / Litestream | Yes | No (single writer) | n/a | Replication/backup only | n/a | `replicated_txid` | n/a | n/a | Apache-2.0 |
| Automerge / Loro / Yjs | No (binary docs); needs a SQLite index you build | Yes | No; needs a separate transactional endpoint | Sync servers exist (automerge-repo-sync-server, loro protocol, Hocuspocus) | Automerge FS storage documented multi-process safe; others DIY | Heads / version vectors, your endpoint | Native (all three) | Native/wasm | MIT |
| **Custom op log** | **Yes, by construction** | Yes | **Yes, in the push transaction, returned in the ack** | **Same binary, `serve` mode** | Yes (measured: `BEGIN IMMEDIATE` + timeout) | Server cursor + op hash in ack | your code | your code | yours |

## Per-engine notes

### Turso DB (Rust rewrite, formerly Limbo) and Turso Sync

- Version: v0.8.3 (2026-10-12), v0.9.0-pre.1 (2026-10-15); crates `turso`, `turso_sync_engine` 0.8.3; npm `@tursodatabase/sync` 0.8.3 ([releases](https://github.com/tursodatabase/turso/releases), [crates.io](https://crates.io/crates/turso_sync_engine), [npm](https://www.npmjs.com/package/@tursodatabase/sync)). MIT. About 24.8k stars, pushed 2026-10-16. Pre-1.0: "we have not yet reached 1.0 … some features are explicitly marked experimental" ([README](https://github.com/tursodatabase/turso/blob/main/README.md)).
- Sync model: push sends row-level logical changes; pull sends physical pages so the replica becomes byte-identical; unpushed local changes are rolled back, remote applied, then replayed ([blog 2025-10-08](https://turso.tech/blog/introducing-databases-anywhere-with-turso-sync), [conflict resolution](https://docs.turso.tech/sync/conflict-resolution)). Last push wins. A client-side `transform` hook can rewrite changes. Last explicit label is "Beta" (2025-10-08); no GA announcement found.
- Self-host: `tursodb --sync-server ADDR [--sync-dir PATH]`, implemented in [cli/sync_server.rs](https://github.com/tursodatabase/turso/blob/main/cli/sync_server.rs) with only `POST /v2/pipeline` and `/pull-updates`, no auth or TLS (the research pass searched the file for auth/token/bearer/TLS and found nothing). Turso Cloud's server is closed source ([2025-01-21 post](https://turso.tech/blog/upcoming-changes-to-the-turso-platform-and-roadmap)).
- Blockers: the [manual](https://github.com/tursodatabase/turso/blob/main/docs/manual.md) lists "No multi-process access" and labels `mvcc` "not production ready"; [COMPAT.md](https://github.com/tursodatabase/turso/blob/main/COMPAT.md): "We don't support mixed SQLite and Turso in multi-process scenarios." That kills requirement 6 and makes requirement 5 rely on a snapshot anyway.
- Worth tracking: if multi-process access stabilizes, Turso DB plus a forked, hardened sync server with a server-side numbering trigger is the closest thing to a product that matches. Sequence assignment via trigger is **inferred**, untested.

### libSQL embedded replicas

`libsql` crate 0.10.0 (2026-10-13), `@libsql/client` 0.18.0, MIT; `sqld` last released v0.24.32 on 2025-02-14 ([releases](https://github.com/tursodatabase/libsql/releases)). Offline writes require `offline: true`; the docs warn "Do not open the local database while the embedded replica is syncing. This can lead to data corruption," and steer new projects to Turso Sync ([embedded replicas](https://docs.turso.tech/features/embedded-replicas/introduction)). The 2025-03-31 offline beta said conflict "resolution is not yet implemented" ([post](https://turso.tech/blog/turso-offline-sync-public-beta)). Superseded; not a candidate.

### sqlite.ai sqlite-sync

1.2.0 (2026-09-28), active ([releases](https://github.com/sqliteai/sqlite-sync/releases)). CRDT extension (`cloudsync`) with causal-length sets, delete-wins/add-wins, block-level LWW text ([README](https://github.com/sqliteai/sqlite-sync/blob/main/README.md)). Server is SQLite Cloud or your Postgres with the `cloudsync` extension, still registered in SQLite Cloud's dashboard and routed by their microservices ([postgres quickstart](https://github.com/sqliteai/sqlite-sync/blob/main/docs/postgresql/quickstarts/postgres.md)). [LICENSE.md](https://github.com/sqliteai/sqlite-sync/blob/main/LICENSE.md): Elastic 2.0 with a "Network Layer Restriction" against any alternative transport or server protocol, and commercial license needed for non-open-source production use. [schema.md](https://github.com/sqliteai/sqlite-sync/blob/main/docs/schema.md) says to use `cloudsync_uuid()` and avoid autoincrement. Fails requirement 2 on license and architecture.

### cr-sqlite (vlcn)

Last release v0.16.3 on 2024-01-17; a few 2024 fixes, then community build fixes in August 2026 with no release ([commits](https://github.com/vlcn-io/cr-sqlite/commits/main)). MIT. Column-level LWW CRRs exchanged through `crsql_changes` ([README](https://github.com/vlcn-io/cr-sqlite/blob/main/README.md)); transport is DIY ([guide](https://vlcn.io/docs/cr-sqlite/networking/whole-crr-sync)). Good ideas to borrow (per-column clocks, `db_version` cursors), but dormant; adopting it means owning a C/Rust extension.

### Electric SQL

Read-path only: "Electric does not do write-path sync" ([writes](https://electric.ax/docs/guides/writes)). Needs Postgres 14+ with logical replication plus an Elixir service ([deployment](https://electric.ax/docs/guides/deployment)). `@core/sync-service` 1.8.1 (2026-09-07), Apache-2.0. Offline writes require TanStack DB persistence (0.6, 2026-03-25) and `@tanstack/offline-transactions` ([TanStack blog](https://tanstack.com/blog/tanstack-db-0.6-app-ready-with-persistence-and-includes)). Team joined Neon at Databricks on 2026-08-11; Electric Cloud "is winding down" ([blog](https://electric.ax/blog/2026/08/11/electric-joining-databricks)). No Rust client. Too heavy and wrong shape.

### PowerSync

Best-designed of the Postgres-backed options for this use: local SQLite, FIFO upload queue to your backend, server state applied only at checkpoints so the client "never has to resolve conflicts locally" ([consistency](https://docs.powersync.com/architecture/consistency)), and [client ID docs](https://docs.powersync.com/sync/advanced/client-id) cover server-assigned sequential IDs. Against it: service is FSL-1.1-ALv2 ([FSL](https://www.powersync.com/legal/fsl)); self-hosting needs a source DB, bucket storage (Mongo replica set or Postgres) and the Node service ([setup](https://docs.powersync.com/self-hosting/installation/powersync-service-setup)); the Node SDK says "only instantiate one instance per file" ([Node reference](https://docs.powersync.com/client-sdks/reference/node)); Rust crate `powersync` 0.1.1 is beta ([Rust reference](https://docs.powersync.com/client-sdks/reference/rust)); verifying sync via checkpoint requests is alpha and Swift-only ([checkpoint requests](https://docs.powersync.com/client-sdks/advanced/checkpoint-requests)).

### Rocicorp Zero

1.0 GA 2026-03-24, latest 1.11.0 (2026-10-16), Apache-2.0 ([status](https://zero.rocicorp.dev/docs/status)). Server mutators can return values that differ from the client run, which is the right pattern for numbering ([mutators](https://zero.rocicorp.dev/docs/mutators)), but Zero "doesn't support offline writes" and "is not local-first" ([when to use](https://zero.rocicorp.dev/docs/when-to-use)), client store is a KV/IndexedDB cache, TS only. Eliminated by requirement 1.

### LiteFS / Litestream

LiteFS v0.5.14 (2025-04-22), Fly says "We are not able to provide support or guidance for this product" ([docs](https://docs.fly.io/litefs)). Litestream v0.5.17 (2026-08-31), Apache-2.0, active; its VFS write mode "assumes a single writer" ([guide](https://litestream.io/guides/vfs-write-mode/)). Neither does offline writes or merging. Litestream is a good fit for **backing up the authority server's database** to object storage.

### Automerge / Loro / Yjs

- Automerge: `@automerge/automerge` 3.5.0 (2026-09-16), Rust `automerge` 0.12.0, MIT; 3.0 cut memory more than 10x ([blog](https://automerge.org/blog/automerge-3/)). Concurrent field sets pick a deterministic winner but keep losers visible via `getConflicts()`. The filesystem storage adapter is "safe for multiple processes to use the same data directory" ([storage docs](https://automerge.org/docs/reference/repositories/storage/)). The repo layer is mid-transition (npm `latest` points at 2.6.0-alpha; a 3.0.0-experimental line started 2026-10-05; Subduction sync is nightly). Rust repo `samod` 0.15.0 still says "don't use this anywhere serious yet" ([samod](https://github.com/alexjg/samod)).
- Loro: `loro` 1.16.2 crate / `loro-crdt` 1.16.4, MIT, stable encoding since 1.0; movable list/tree and shallow snapshots; no storage layer; its own docs say uniqueness needs a "transactional authority" ([when not CRDT](https://github.com/loro-dev/loro-docs/blob/main/pages/docs/concepts/when_not_crdt.mdx)).
- Yjs: `yjs` 13.6.33, v14 in RC; `yrs` effectively single-maintainer; map conflicts decided by client ID ("From your point of view, it is random", [yjs#520](https://github.com/yjs/yjs/issues/520)). Weakest fit.
- Common gap: none is a query engine, so a SQLite index is still needed for `list --status open --assignee me`; numbering needs a separate server endpoint. An issue tracker's fields are mostly scalar (status, assignee, labels), where CRDT merge buys little over per-field LWW. Where CRDTs do earn their place is **long text** (issue descriptions edited concurrently); a CRDT text field could be embedded later as an opaque blob column inside the custom design (**inferred** design option).

## Custom design: op log in SQLite + HTTP sync

### Data model

- `ops` table, append-only: `op_id` (UUIDv7, client-generated), `actor` (device/agent id), `project`, `entity_id` (issue UUID), `kind` (`create`, `set_field`, `add_label`, `remove_label`, `comment`, `link`), `payload` (JSON), `hlc` (hybrid logical clock), `server_seq` (NULL until acked), `status` (`pending` / `acked` / `rejected`).
- Materialized tables (`issues`, `labels`, `comments`, `links`) updated in the **same** `BEGIN IMMEDIATE` transaction as the op insert, so reads never replay.
- Issues are keyed by UUID. `number` is NULL until the server assigns it; the CLI shows a provisional handle (e.g. `CPL-~7f3a`, from the UUID) and accepts either form. Store a `aliases` table so provisional handles keep resolving after numbering (agents may have written the provisional ID into commits or notes).

### Sync protocol (sketch)

- `POST /v1/projects/{p}/push` with pending ops (idempotent on `op_id`). The server, in one transaction: dedupes, assigns `server_seq` (global per project), allocates `CPL-n` from a per-project counter row for each `create`, applies ops to its own materialized state, and returns `{acked: [{op_id, server_seq}], numbers: [{entity_id, number}], cursor, state_hash}`.
- `GET /v1/projects/{p}/pull?after=cursor` returns ops in `server_seq` order. The client applies remote ops, then rebases its still-pending ops on top (same approach as Turso Sync's rollback-and-replay, but at the op level).
- Conflict rule: per-field last-writer-wins by `server_seq` (server order is the truth); set-like fields (labels) as add/remove ops; comments are append-only. The server may reject ops (deleted project, permission) and say so in the ack, giving a visible `rejected` state instead of a silent drop.
- Verifiability (requirement 4): `tracker sync status` compares local `cursor` and `state_hash` (hash over materialized rows or over the op sequence) against `GET /status`. "Synced" means the server returned the same cursor and hash, not that a push call returned 200. No git remote anywhere.

### Same binary as the server

`tracker serve` runs the HTTP server against a server-side SQLite db (WAL, single process, so concurrency is trivial), plus auth tokens. Litestream backs it up. Offline-only users never run it.

### Costs the custom design takes on

- Writing and testing the rebase logic, schema migrations across client/server versions (op payloads need versioning), auth, and a compaction/snapshot story for the op log.
- Every op type needs a well-defined apply function on both sides; sharing one core library between client and server is what makes this tractable, which argues for one language.

## Recommendation

1. **Build the custom op-log design** on plain SQLite. It is the only option that satisfies all six requirements, and the expensive parts (SQLite, HTTP) are commodity. Borrow ideas: PowerSync's "client never resolves conflicts locally; server state lands at checkpoints" and Turso Sync's rollback-and-replay; Zero's server-mutator return values for numbering.
2. **Local store layout:** `store.db` in WAL for all writers (`BEGIN IMMEDIATE`, `busy_timeout` ≥ 5000, short transactions, no network inside); `snapshot.db` in rollback mode, republished by `VACUUM INTO` + rename after write batches, opened by sandboxed agents with `immutable=1`; `PERSIST_WAL` on as a fallback.
3. **Language: Rust.** Reasons in order of weight: one static binary that is CLI, TUI, local web UI server, and `serve` authority (same-binary requirement); cold start in the `sqlite3` class (6.3 ms measured for `sqlite3`; Node measured 28.5 ms before loading any app code, Bun 9.9 ms); `rusqlite`'s `bundled` feature pins the SQLite build, so sidecar lifecycle and PERSIST_WAL behaviour are under your control (measured above: Apple's system build leaves `-wal`/`-shm` after close, Homebrew's does not). The web UI can still be TypeScript, served as static assets embedded in the binary. Bun is a credible second choice if the team strongly prefers TS: `bun build --compile` gives a single binary and its measured cold start was within ~4 ms of `sqlite3`. Caveat: `bun:sqlite` on macOS used Apple's system SQLite (`…aapl` source id), so a Bun build would inherit Apple's sidecar behaviour unless it loads a custom SQLite (`Database.setCustomSQLite`, **inferred**, not tested). Single-binary packaging of Bun was not tested here.
4. Keep Turso DB on a watch list. If it ships stable multi-process access and a production sync server, re-evaluate; its sync engine is close to what the custom design builds.

## Open risks

- **Linux sandbox behaviour untested.** All read-only probes ran on macOS (Seatbelt, Codex's sandbox). Landlock/bubblewrap handling of SQLite's `fcntl` locks on a read-only fd, and of `immutable=1`, needs the same probes on Linux before committing.
- **Snapshot staleness.** Sandboxed agents see the state as of the last publish. Publishing after every write batch keeps the lag small, but a writer that crashes between commit and publish leaves the snapshot stale until the next write. Include the op cursor in `snapshot.meta` so agents can report "as of".
- **Snapshot cost at scale.** `VACUUM INTO` was 0.06 s on 41 MB with a warm cache, measured once. Large attachments in the db would change that; keep blobs out of the main store.
- **Rename semantics.** Readers holding the old `snapshot.db` open across a rename are assumed to keep a consistent old inode (standard POSIX; not tested here). Windows behaves differently (rename over an open file fails), so Windows support needs a versioned-filename scheme instead (**inferred**).
- **Provisional ID leakage.** Agents will paste provisional IDs into commits and PRs before sync. The alias table must be permanent and synced, or those references rot.
- **Rebase correctness.** Op-level rebase with per-field LWW is simple, but edge cases (edit to a deleted issue, label rename, project move) need explicit rules and property tests.
- **Op log growth and compaction.** Needs a snapshot-plus-tail scheme so a new machine does not replay years of ops; not designed here.
- **Clock skew.** HLC handles ordering of local, unsynced ops; server order decides after sync. Users offline for weeks will see their edits lose to later server edits on the same field; the UI should surface overwritten values (Automerge's `getConflicts()` is the model).
- **Engine statuses move quickly.** Turso DB (pre-1.0, weekly releases) and Automerge's repo layer (mid-transition) were the most volatile; re-check before any later decision.
