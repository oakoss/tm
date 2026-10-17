# Task tracking

Work is tracked in beads (`bd`, embedded Dolt) until the switch milestone in [build-plan.md](build-plan.md), after which it moves to `tm` itself.

## Shape

- One epic per build phase; only the current phase is filed. Later phases stay in the build plan until they are next.
- Tasks carry dependencies, so `bd ready` gives the next task in order, and acceptance criteria copied from the build plan's checks.
- No labels yet. Add them only when a real need appears.
- Close a bead at ship, not at merge; the commit link is a `Closes <id>` trailer.

## Fresh clone

`bd bootstrap` creates the local database. Prefer it to `bd init`, which this repo has already had.

To sync with the private Dolt remote, put its URL in `.beads/.env` (gitignored), never in `config.yaml`:

```
BD_SYNC_REMOTE=ssh://user@host/srv/dolt/task-management
```

## Beads quirks and cleanup

Running beads commands here is fine; clean up after them.

- **It commits on its own.** `bd init` and `bd dolt remote add` each make a commit (`bd init: initialize beads issue tracking`, `bd: update sync.remote`). Undo it with `git reset HEAD~1`, then review and commit the result yourself.
- **It writes the private host into `config.yaml`.** `bd dolt remote add` adds a `sync:` block with the remote URL. Delete it before committing; the URL belongs only in `.beads/.env`.
- **`git push` does not sync tasks.** Run `bd dolt push` explicitly.
- **A push with no remote still succeeds.** `bd dolt push` prints "No remote is configured — skipping." and exits 0, so check `bd dolt remote list` rather than the exit code.
- **Don't run `bd setup codex` or `bd init` without `--skip-agents --skip-hooks`.** They write agent files, edit AGENTS.md and install hooks that set `core.hooksPath`, which silently disables lefthook.
- **Add a git origin only after `bd init`.** With an origin present, `bd init` derives a Dolt remote from it and a later push publishes the task graph to the code host.
