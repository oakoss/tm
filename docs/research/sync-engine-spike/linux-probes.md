# Linux sandbox probes

Run 2026-10-16 in a throwaway Proxmox VM (clone of an Ubuntu 24.04 cloud image, kernel 6.8.0-147-generic, active LSMs `lockdown,capability,landlock,yama,apparmor`), as an unprivileged user, with Ubuntu's sqlite3 3.45.1, bubblewrap 0.9.0 and codex-cli 0.163.0 (official musl release binary, no login). Scripts: `scripts/linux/probe.sh` and `scripts/linux/conc_test.sh`. The VM was destroyed afterwards.

## Setup finding

With Ubuntu 24.04's default `kernel.apparmor_restrict_unprivileged_userns=1`, neither sandbox starts: `bwrap` fails with `setting up uid map: Permission denied` and `codex sandbox` fails with `bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted`. Setting the sysctl to 0 made both work. Codex users on stock Ubuntu 24.04 hit the same wall, so whatever they do to make Codex work applies here too.

## Read-only opens

Both `bwrap --ro-bind / /` and `codex sandbox -c 'sandbox_mode="read-only"'` refused writes (`touch: cannot touch 'probe_file': Read-only file system`) and gave identical SQLite results:

| Database state | default open / `mode=ro` | `immutable=1` |
|---|---|---|
| Rollback journal | 2 rows | 2 rows |
| WAL, cleanly closed (no `-wal`/`-shm`) | `unable to open database file (14)` | 2 rows |
| WAL, chmod 555 dir + 444 db | `unable to open database file (14)` | 2 rows |
| WAL, live writer (3 committed, 1 only in `-wal`) | 3 rows (reads through the writer's `-shm`) | 2 rows, stale |
| Snapshot (`VACUUM INTO` + rename, rollback mode) | 2 rows | 2 rows |

No sandboxed open created files. Unsandboxed, a read-only open of a cleanly closed WAL database created `-wal`/`-shm` and left them behind, as on macOS.

Unix socket connect: allowed under bwrap, `PermissionError: [Errno 1] Operation not permitted` under Codex, matching macOS.

## Concurrency

50 parallel shells × 10 short `sqlite3` read-then-write transactions, busy timeout 5 s, 3 runs: `BEGIN IMMEDIATE` 500/500 rows and 0 errors in every run. `BEGIN DEFERRED` was also clean here (0 errors in 3 runs), unlike macOS (187 errors); with 2 vCPUs the contention was lower, so this does not make DEFERRED safe.

## Verdict

Linux matches macOS on every point the storage design depends on: a live WAL store cannot be opened from a write-denied sandbox, a rollback-mode snapshot opened with `immutable=1` can, Codex blocks local sockets, and `BEGIN IMMEDIATE` with a busy timeout gives lossless concurrent writes. The snapshot design in ADR 0001 holds on both platforms.
