# The first write of each day backs up its Workspace

- Status: accepted
- Date: 2026-10-22
- Deciders: Jace Babin

## Context and Problem Statement

`tm export --dated` ([ADR 0013](0013-export-carries-the-log-and-issue-records.md)) writes a backup that restores with `tm import`, but only when someone runs it, and nothing removes old ones. When does tm back up a Workspace without being asked, and which backups does it keep?

## Decision Drivers

- No daemon ([ADR 0001](0001-local-store-is-plain-sqlite-without-a-daemon.md)): only a `tm` process a person or Agent runs can take a backup.
- A write must not fail, or slow down every time, because of a backup.
- Measured (`mise run coldstart`, release build, Apple Silicon): `tm export --dated` takes 27 ms at 200 Issues and 669 ms at 10,000, against 120 ms for an ordinary write at 10,000.
- Removing a backup is the one step here that cannot be undone.
- `.config/tm.toml` is tracked and holds nothing that differs between machines.

## Considered Options

- **When**: the first write of each local day; every N operations; after the command's output is printed; a detached child process.
- **What is kept**: the newest N; tiers of days, weeks and months.
- **Where the policy lives**: the repo file; a config file in tm's home.

## Decision Outcome

**The first write of each local day backs up its Workspace**, after the write commits and the Snapshot is published, as `exports/<Workspace id>/<local date>.jsonl` in tm's home: the file `tm export --dated` writes, read back with `tm import`'s check before it takes its name.

- A write whose command added to the log checks for today's file, one `stat`, and returns when it is there. Otherwise it takes the exports directory's lock, checks again, and writes the backup only if still missing, so two first writes of a day write one backup. A day with no writes takes none, and the writes after the first are in the next day's.
- Only a non-empty regular file counts as today's backup. An empty one is replaced; anything else at that name, such as a directory or a link, is left alone with a warning on every write until it is moved.
- `tm init` and `tm import`'s restore take none: a new Workspace has nothing to lose, and a restore's source is itself a backup.
- A failed backup or collection is a warning on the write, in its JSON envelope too ([ADR 0011](0011-the-cli-speaks-one-json-envelope-and-flat-coded-errors.md)); the write stands.
- `tm export --dated` still replaces the day's file when the log changed since.

**Old backups are collected by tiers**, after each backup, automatic or `tm export --dated`, under the same lock. Kept: every backup from the last `keep_days` days, the newest of each of the last `keep_weeks` ISO weeks, the newest of each of the last `keep_months` months, and the newest backup of all. Defaults: 14 days, 8 weeks, 12 months, so at most about 34 files.

- Only a regular file named `YYYY-MM-DD.jsonl` in that Workspace's exports directory is ever removed. A file dated after today, as from a clock that ran ahead, is kept. Nothing is removed when the exports directory, or the Workspace's directory in it, is a symbolic link, which may lead out of tm's home.
- A `.tm-import-*` staging directory there is removed too, whatever the policy: any export still writing holds the lock, so one found under it was left by a killed process.
- A file that cannot be removed is a warning naming it, and collection goes on with the rest.

**The policy is tm's first machine-level config**, `config.toml` in tm's home (`TM_HOME`, else `~/.tm`):

```toml
[backups]
enabled = true
keep_days = 14
keep_weeks = 8
keep_months = 12
```

- `enabled = false` stops writes taking backups; `tm export --dated` still writes one and collects.
- A missing file or key takes the default. The file is read only when a backup is due.
- A file tm cannot read, or a table or key it does not know, such as `[backup]`, is a warning: the backup is still written, and no backup is removed, since a policy misread could remove what was meant to be kept.

### Consequences

- Good, because a Workspace is backed up daily with no step to remember, and the manual daily `tm export --dated` ends.
- Good, because the other writes of a day pay one `stat`.
- Bad, because the first write of a day pays the whole export: about 0.7 s at 10,000 Issues.
- Bad, because a day's writes after its first are backed up only by the next day that has a write.
- Neutral, because the policy covers every Workspace on the machine; a per-Workspace table can join `[backups]` later without changing what is there, if one is asked for.

### Confirmation

A write under an empty tm home writes today's backup, which `tm import` restores; a second write that day leaves it untouched, and a write that changes nothing takes none. A write that cannot back up succeeds and warns. Collection over 400 days of backups keeps exactly the tiers' dates; files of other names, other Workspaces and later dates are kept, nothing is removed through a link, and a bad config removes no backup.

## Pros and Cons of the Options

### Every N operations

- Bad, because it pays the same export at a moment no one can predict, and a quiet day may take none.

### After the output is printed

- Bad, because a JSON write's warnings are inside its envelope, so a backup that fails after it cannot warn; and an Agent's harness waits for the process to exit either way.

### A detached child process

- Good, because no write waits for the export.
- Bad, because its failure reaches no one, and a harness that waits on an inherited pipe or the process group still waits.

### The newest N

- Bad, because N daily backups cover N days at most, so a problem found later has nothing from before it to restore.
