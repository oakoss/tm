# Local store is plain SQLite without a daemon

- Status: accepted
- Date: 2026-10-16

Each machine keeps its Issues in a plain SQLite file outside any code repo, written directly by every short-lived `tm` process with `BEGIN IMMEDIATE` and a busy timeout, so parallel agents can write without a background process. Sandboxed agents that cannot write any file read a separately published, never-modified snapshot opened with `immutable=1`, because a live WAL database cannot be opened at all under a write-denying sandbox. Sync between machines is an append-only operation log exchanged with `tm serve`, which orders operations and assigns Keys; it runs on SQLite first, with Postgres added later for hosted or multi-user setups.

## Considered Options

- **Dolt (as beads uses)**: single writer in embedded mode, unreadable from a read-only sandbox (beads #7445), and sync over a git remote that can corrupt under concurrent pushes.
- **Turso DB**: multi-process writes are experimental; in our tests on v0.8.3 and v0.9.0-pre.1, 16 of 20 runs had a process panic and 3 of 20 lost acknowledged rows. A sync replica cannot be opened by a second process, and the sync server has no auth. Re-test at each minor release.
- **Postgres or PGlite**: both need a long-running process for multiple writers, and a sandboxed agent cannot reach it over a socket.
- **Electric, PowerSync, Zero**: need Postgres plus a separate service; Electric syncs reads only.
- **LMDB, or plain files with SQLite as a rebuildable index**: workable without a daemon but give up queries or add a second representation to maintain.

## Consequences

- Sandboxed agents see data as of the last published snapshot.
- Snapshot publishing relies on rename-over, which Windows does not allow on open files; Windows is deferred (WSL2 works).
- The sandbox and concurrency probes give the same results on macOS and on Ubuntu 24.04 under both bubblewrap and Codex's sandbox ([linux-probes.md](../research/sync-engine-spike/linux-probes.md)).
- All database access stays in one storage module so a later engine swap (for example to Turso) does not reach the sync protocol.
