# Task tracking

Work is tracked in `tm` itself, in Project `TM`, which `.config/tm.toml` maps this repo to. Beads tracked it until the switch milestone in [build-plan.md](build-plan.md) and is frozen as a backup.

## Setup

Install the `tm` you track work with from `main`, apart from the build you develop: `cargo install --locked --path crates/tm-cli`. A Workspace lives in tm's home on one machine; there is no sync yet, so this repo's Workspace is on the machine that created it.

Agents start from `tm prime`, which the `SessionStart` hook in `.claude/settings.json` runs. Its other hooks, from `tm hook print claude`, renew the session's Claims after each tool use and release them when the session ends.

## Shape

- One epic per build phase; only the current phase is filed. Later phases stay in the build plan until they are next.
- Tasks are ordered by `blocks` Links, so `tm ready` gives the next task in order, and carry acceptance criteria from the build plan's checks in their description.
- No labels yet, apart from the `beads-*` ones the import added. Add them only when a real need appears.

## Flow

`tm guide` prints the general habits; this repo's own steps are:

- Starting work: `tm start TM-n`, which claims the Issue, moves it to in_progress and, if it has no Assignee, makes the human the work is for its Assignee, as one undo. The Claim lasts 2 hours and the session's writes renew it; `tm release TM-n` gives it up, and moving the Issue to done releases it.
- At ship: `tm move TM-n in_review`; at merge: `tm move TM-n done`. The commit link is a `Closes TM-n` trailer, or `Refs TM-n` for part of the work.
- Changing fields, labels or the parent: `tm edit TM-n --set field=value`, with `--set field=-` to read long text from stdin, `--unset`, `--label`, `--remove-label`, `--parent` and `--no-parent`.
- The description holds the current plan; progress, what was learned and changes of plan go in `tm comment TM-n`, which keeps a dated trail an edit would overwrite.

## Keeping the data safe

- Until the store has gone a month without data problems, from 2026-10-20, run `tm export --dated` daily. It writes `exports/<Workspace id>/<date>.jsonl` in tm's home, and writes nothing when the log has not changed. To run it on every commit instead, add `tm export --dated >/dev/null` to your own `.git/hooks/post-commit`; tm never installs a hook.
- Until v1 the log format changes in place without a version bump, so a build with a changed format can misread this store rather than refuse it ([ADR 0010](../decisions/0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md)). A pull request that changes the format moves this store across: the installed `tm` runs `tm export`; with `.config/tm.toml` moved aside, since `tm init` otherwise reuses the Workspace it names, the new build runs `tm init --key TM` and `tm import --records`; the pull request commits the new `.config/tm.toml`; and only then is the installed `tm` updated.

## Beads, frozen

Beads' data stays in `.beads/` and its Dolt remote; don't write to it here. The final `bd export`, `bd list --json --all --limit 0` and a table of each bead's id and its Key in tm are under `beads-backup/` in tm's home; commits and docs from before the switch name beads ids such as `tm-u8g.11`.

If `tm` loses or corrupts data, go back to beads, fix the cause, and import again. Beads' own quirks, for then:

- **It commits on its own.** `bd init` and `bd dolt remote add` each make a commit. Undo it with `git reset HEAD~1`, then review and commit the result yourself.
- **It writes the private host into `config.yaml`.** `bd dolt remote add` adds a `sync:` block with the remote URL. Delete it before committing; the URL belongs only in `.beads/.env`.
- **`git push` does not sync tasks.** Run `bd dolt push`, and check `bd dolt remote list`, since a push with no remote exits 0.
- **Don't run `bd setup codex` or `bd init` without `--skip-agents --skip-hooks`.** They edit AGENTS.md and set `core.hooksPath`, which silently disables lefthook.
