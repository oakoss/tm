# AGENTS.md

`tm` is a local-first issue tracker for one human and their AI agents: a Rust CLI and TUI with a later TypeScript web UI. The behavior it must have is in [docs/specs/design.md](docs/specs/design.md); the order it is built in is [docs/contributing/build-plan.md](docs/contributing/build-plan.md).

`mise run setup` bootstraps a fresh clone. `mise run check`, `mise run test` and `mise run audit` are the local gates; CI runs the same tasks. `mise run ci` runs every CI job's task in turn, the cold-start budget included, which CI holds only on Linux.

## Rules that override defaults

**tm writes only what it owns.** It never runs `git commit`, never edits `AGENTS.md` or `CLAUDE.md`, never changes git config (including `core.hooksPath`), never writes per-machine values (hosts, URLs, tokens) into a tracked file, and writes anything outside its Workspace only after a confirmed diff. Code that breaks this is a bug, whatever the convenience.

**Check decisions before proposing.** [docs/decisions/](docs/decisions/) records choices that took measurement to reach, such as plain SQLite without a daemon and the operation log as the source of truth. Contradicting one means writing a superseding decision, not diverging quietly.

**Use the glossary's words.** [GLOSSARY.md](GLOSSARY.md) defines Issue, Key, Workspace, Claim, Finding and the rest; use those terms in code, docs and commits, and avoid the listed alternatives.

## Project defaults

- Work is tracked in `tm` itself, Project `TM`; Phase 1 is epic `TM-1`. Run `tm ready` for the next task. Setup, flow and the frozen beads backup: [docs/contributing/task-tracking.md](docs/contributing/task-tracking.md).
- Commit messages: `type(scope): summary`. Branches: `<type>/tm-<n>-<theme>`, for Issue `TM-n`. Details: [docs/contributing/conventions.md](docs/contributing/conventions.md).

## More detail

- Contributor map (invariants, structure, conventions, pull requests): [docs/contributing/index.md](docs/contributing/index.md)
- Docs layout (decisions, specs, research, contributing): [docs/README.md](docs/README.md)
- Research behind the design, with sources and scripts: [docs/research/](docs/research/)
