# Publishing the Snapshot faster than VACUUM INTO

- Date: 2026-11-01
- Author: Claude, for Jace Babin (TM-38)
- Scope: how each write copies the store to the Snapshot, and what a faster copy would cost.

## Question

Every write publishes the Snapshot by `VACUUM INTO` a file beside it and a rename ([ADR 0001](../../decisions/0001-local-store-is-plain-sqlite-without-a-daemon.md)), so sandboxed Agents that may not open the store still read current data. How much of a write is that copy, and would SQLite's backup API, a file copy after a checkpoint, or publishing less often be faster while keeping the guarantee that a reader finds the old copy or the new one, never part of either?

## Sources

- SQLite's documentation of [`VACUUM INTO`](https://sqlite.org/lang_vacuum.html#vacuuminto), the [online backup API](https://sqlite.org/backup.html) and [`wal_checkpoint`](https://sqlite.org/pragma.html#pragma_wal_checkpoint).
- [`crates/tm-store/src/snapshot.rs`](../../../crates/tm-store/src/snapshot.rs): `publish_snapshot` copies with `VACUUM INTO` under a lock file, renames the copy over the Snapshot, then checkpoints `PASSIVE`.
- Measurements from [scripts/snapshot](scripts/snapshot): seeds a store as the cold-start test does (one batch; a comment on every tenth Issue and a `blocks` Link on every fifth), then times each copy seven times and compares the copy's `issues` rowids with the source's.

## Findings

### How long each copy takes

Medians of seven runs; the range spans two runs, except the single run at 100,000 Issues. "Backup, then rollback" is the backup API followed by `PRAGMA journal_mode = DELETE` on the copy (see below).

| Store | `VACUUM INTO` | Backup API | Backup, then rollback | Checkpoint, then copy the file |
|---|---|---|---|---|
| macOS (Apple M2 Max, APFS), 10,000 Issues, 45.2 MB | 110.8–112.3 ms | 72.0–75.9 ms | 75.4–77.6 ms | 0.1 ms |
| macOS, 100,000 Issues, 452.8 MB | 1,081 ms | 736 ms | 731 ms | 0.1 ms |
| Linux (Docker on the same Mac, aarch64, overlay filesystem), 10,000 Issues, 44.6 MB | 74.9–76.1 ms | 48.4–51.4 ms | 50.6–54.6 ms | 5.4–5.9 ms |

- **The copy is most of a write at scale.** The cold-start test's writes at 10,000 Issues take 131–138 ms (`mise run coldstart`, measured during TM-34), of which about 110 ms is this copy. At 100,000 Issues each write would spend over a second publishing.
- **The backup API is about a third faster** on both systems, and still about 30% faster once the copy is switched to a rollback journal, which costs about 3 ms at 10,000 Issues and was within noise at 100,000. It copies pages rather than rebuilding every table and index as `VACUUM INTO` does.
- **A file copy after a checkpoint is near-free.** On APFS `std::fs::copy` clones the file; on the Linux overlay filesystem, which cannot clone, it is a plain copy from the page cache. A filesystem that can clone, such as btrfs or XFS, was not measured.
- **Every method kept the rowids of `issues`.** `VACUUM INTO` may renumber rows of tables without an `INTEGER PRIMARY KEY`, but on these stores, where no Issue row is ever deleted, it did not. The backup API and a file copy keep rowids by construction.

### What each keeps of the Snapshot's guarantee

- **A copy of the pages keeps the WAL mark.** Header bytes 18 and 19 are 2 in a WAL database; the backup API and a file copy carry them over, while `VACUUM INTO` writes a rollback-journal file (`marked WAL` in the benchmark's output). tm opens the Snapshot with `immutable=1`, which reads either. A reader denied every write that opens it with only `mode=ro`, such as an Agent running `sqlite3` in a sandbox, cannot open a WAL-marked copy: in a directory of mode 555, `?mode=ro` failed with `attempt to write a readonly database` on the backup copy and opened the `VACUUM INTO` copy (run with Python's sqlite3, 3.54.1). Switching the copy to a rollback journal before the rename keeps today's file format.
- **The backup API, with that switch,** reads one consistent snapshot of the source inside the writer's connection, as `VACUUM INTO` does, and writes to a separate file that is renamed over the Snapshot. The lock, the partial file and the rename stay as they are.
- **A checkpoint and a file copy** keeps the WAL mark too, and is consistent only while nothing changes the main database file during the copy. A `TRUNCATE` checkpoint cannot run inside the writer's own transaction: `BEGIN IMMEDIATE; …; PRAGMA wal_checkpoint(TRUNCATE)` fails with `database table is locked` (run with the system sqlite3, 3.54.0; tm bundles 3.53.2). So the writer cannot hold its write lock across the checkpoint and the copy on one connection, and another process's checkpoint could change the file mid-copy. A safe protocol needs a second connection and a lock order that has not been worked out, and it also needs readers gone for `TRUNCATE` to succeed.
- **Publishing less often** leaves the Snapshot stale until a later write. The sandboxed reader that needs it cannot write anything, so it cannot ask for a fresh copy; only a writer can know to publish, and it would have to publish every time.

## Conclusions

The backup API, with the copy switched to a rollback journal, is the safe improvement: about 30% off every write's publish on macOS and Linux, with the same publish protocol, the same file format, consistent copies and rowids kept. A checkpoint and a file copy would make the publish nearly free, but only with a locking protocol nobody has shown to be correct. Publishing less often breaks what the Snapshot is for.

## Implications / actions

- Publish the Snapshot with the backup API: in `publish_snapshot`, copy with `rusqlite::backup` into the same partial file and set `journal_mode = DELETE` on it before the rename, and update design.md's "by `VACUUM INTO` and rename". A test that opens the Snapshot with `mode=ro` alone, in a directory it may not write, keeps the format from drifting back to WAL. No ADR changes, since ADR 0001 decides a published, never-modified copy, not how it is made.
- A store at 100,000 Issues still spends about 0.7 s per write publishing with the backup API. If stores that size matter, the checkpoint-and-copy protocol is worth its own spike.

## Open questions

- A correct lock order for a checkpoint and a file copy, and how it behaves while readers hold the log.
- Filesystems that can clone on Linux (btrfs, XFS) were not measured.
- Publishing off the write's latency path, or skipping a publish when another writer's is already waiting on the lock, was not considered; either keeps every write published while hiding some of the cost.
- Seeding 100,000 Issues in one batch took 619 s, against 11 s for 10,000: the full-text index's update scans by Issue id, which TM-124 addresses.

## Raw data

To rerun: `cargo build --release` in [scripts/snapshot](scripts/snapshot), then `target/release/snapbench DIR 10000` (and `100000`); a first run seeds `DIR/store-N.db` and later runs reuse it. The Linux rows ran the same program in `rust:1-slim` with the store under `/tmp`.
