# AGENTS.md

`tm` is a local-first issue tracker for one human and their AI agents: a Rust CLI and TUI with a later TypeScript web UI. The behavior it must have is in [docs/specs/design.md](docs/specs/design.md); the order it is built in is [docs/contributing/build-plan.md](docs/contributing/build-plan.md).

No code exists yet. Phase 1's first task scaffolds the Cargo workspace and adds `mise run check` and `mise run test` as the local gates.

## Rules that override defaults

**tm writes only what it owns.** It never runs `git commit`, never edits `AGENTS.md` or `CLAUDE.md`, never changes git config (including `core.hooksPath`), never writes per-machine values (hosts, URLs, tokens) into a tracked file, and writes anything outside its Workspace only after a confirmed diff. Code that breaks this is a bug, whatever the convenience.

**Check decisions before proposing.** [docs/decisions/](docs/decisions/) records choices that took measurement to reach, such as plain SQLite without a daemon and the operation log as the source of truth. Contradicting one means writing a superseding decision, not diverging quietly.

**Use the glossary's words.** [GLOSSARY.md](GLOSSARY.md) defines Issue, Key, Workspace, Claim, Finding and the rest; use those terms in code, docs and commits, and avoid the listed alternatives.

## Project defaults

- Work is tracked in beads until the switch milestone in the build plan; Phase 1 is epic `tm-u8g`. Run `bd ready` for the next task. Beads quirks and cleanup steps: [docs/contributing/task-tracking.md](docs/contributing/task-tracking.md).
- Commit messages: `type(scope): summary`.

## More detail

- Docs layout (decisions, specs, research, contributing): [docs/README.md](docs/README.md)
- Research behind the design, with sources and scripts: [docs/research/](docs/research/)
