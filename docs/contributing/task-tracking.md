# Task tracking

Work is tracked in `tm` itself, in Project `TM`, which `.config/tm.toml` maps this repo to. Beads tracked it until the switch milestone in [build-plan.md](build-plan.md) and is frozen as a backup.

## Setup

Install the `tm` you track work with from `main`, apart from the build you develop: `cargo install --locked --path crates/tm-cli`. A Workspace lives in tm's home on one machine; there is no sync yet, so this repo's Workspace is on the machine that created it.

Agents start from `tm prime`, which the `SessionStart` hook in `.claude/settings.json` runs.

## Shape

- One epic per build phase; only the current phase is filed. Later phases stay in the build plan until they are next.
- Tasks are ordered by `blocks` Links, so `tm ready` gives the next task in order, and carry acceptance criteria from the build plan's checks in their description.
- No labels yet, apart from the `beads-*` ones the import added. Add them only when a real need appears.

## Flow

- Starting work: `tm move TM-n in_progress`.
- At ship: `tm move TM-n in_review`; at merge: `tm move TM-n done`. The commit link is a `Closes TM-n` trailer, or `Refs TM-n` for part of the work.
- Changing a field without changing the Status: `tm move TM-n <its Status> --set field=value`.

## Keeping the data safe

- Until the store has gone a month without data problems, from 2026-10-20, write `tm export` daily to a dated file under `exports/` in tm's home.
- Until v1 the log format changes in place without a version bump, so a build with a changed format can misread this store rather than refuse it ([ADR 0010](../decisions/0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md)). A pull request that changes the format moves this store across: the installed `tm` runs `tm export`; with `.config/tm.toml` moved aside, since `tm init` otherwise reuses the Workspace it names, the new build runs `tm init --key TM` and `tm import --records`; the pull request commits the new `.config/tm.toml`; and only then is the installed `tm` updated.

## Beads, frozen

Beads' data stays in `.beads/` and its Dolt remote; don't write to it here. The final `bd export`, `bd list --json --all --limit 0` and a table of each bead's id and its Key in tm are under `beads-backup/` in tm's home; commits and docs from before the switch name beads ids such as `tm-u8g.11`.

If `tm` loses or corrupts data, go back to beads, fix the cause, and import again. Beads' own quirks, for then:

- **It commits on its own.** `bd init` and `bd dolt remote add` each make a commit. Undo it with `git reset HEAD~1`, then review and commit the result yourself.
- **It writes the private host into `config.yaml`.** `bd dolt remote add` adds a `sync:` block with the remote URL. Delete it before committing; the URL belongs only in `.beads/.env`.
- **`git push` does not sync tasks.** Run `bd dolt push`, and check `bd dolt remote list`, since a push with no remote exits 0.
- **Don't run `bd setup codex` or `bd init` without `--skip-agents --skip-hooks`.** They edit AGENTS.md and set `core.hooksPath`, which silently disables lefthook.
