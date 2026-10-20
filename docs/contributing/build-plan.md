# tm build plan

Ordered phases for building what [design.md](../specs/design.md) describes. Each phase ends in something usable and is done when its checks pass. Work is tracked in beads until the switch milestone, then in `tm` itself.

## Phase 1: core store and CLI

- Monorepo scaffold: Cargo workspace with `tm-core`, `tm-store`, `tm-sync`, `tm-server`, `tm-tui` (internal, `publish = false`) and `tm-cli` (empty where not yet needed); bundled SQLite; the repo infrastructure copied from oakum (mise tasks, pinned toolchain, lints, cargo-deny, formatters, lefthook, cog, CI with a CI Summary gate on macOS and Linux, Renovate, contributing pages). The full list is in the design notes of bead `tm-u8g.1`.
- Snapshot tests (insta) for `--json` output, and a `clippy.toml` I/O tripwire keeping `tm-core` free of I/O, once those crates have code.
- Operation log as the source of truth, table projections in the same transaction, `tm rebuild` ([ADR 0005](../decisions/0005-operation-log-is-the-source-of-truth.md)). Settle and document the per-field merge rule and operation versioning now, though sync comes later.
- Invariants: unique operation IDs, deterministic link keys, Keys never reused, take-only-if-unclaimed.
- Workspaces, repo mapping through `.config/tm.toml`, Key assignment by the local authority ([ADR 0004](../decisions/0004-one-authority-assigns-keys.md)).
- Built-in fields and a minimal default Scheme and Workflow.
- `create`, `list`, `show`, `move`, `comment`, `ready`, `claim`, `undo`; `--json` on all of them; structured errors.
- A first cut of the filter language: field qualifiers, negation, lists, `@me`, `sort:`.
- `tm prime`, `tm export`, `tm import` with the beads adapter.
- Read-only snapshot publishing.
- Bare `tm` without a terminal prints help.

Done when:

- a rebuild-from-log property test shows the tables match the log
- an export → import round trip loses nothing
- the 50-writer test (`BEGIN IMMEDIATE`, 5 s busy timeout) writes 500 of 500 rows on macOS and Linux CI
- a sandboxed reader opens the snapshot under bubblewrap on Linux
- read commands meet the cold-start budget (target to be set in this phase and measured in CI)

### Switch milestone: move this project from beads to tm

When Phase 1's checks pass:

1. Run `bd export | tm import --from beads - --dry-run` to see what tm does not keep, then `bd export | tm import --from beads -`, and check counts, dependencies and statuses against `bd list --json --all --limit 0`, since plain `bd list` hides closed Issues.
2. Freeze the beads data as a backup; do not delete it.
3. Track all further work in `tm`, with agents using `tm prime` at session start.
4. Run `tm export` daily into a dated file until the store has gone a month without data problems.
5. If `tm` loses or corrupts data, fall back to beads, fix the cause, and re-import.

## Phase 2: TUI first screens

Issue list with filters, Issue detail with history, Triage, Board, ready queue. Views defined in config.

Done when daily triage and planning for this project happen in the TUI.

## Phase 3: agent workflow

- "On behalf of" in history; Actor detection and recording it are Phase 1 ([ADR 0010](../decisions/0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md)).
- Full Claims: protected fields, `--force`, version-checked writes, expiry and heartbeat, `tm start`, `tm handoff`, `ready --claim`, Path Claims.
- Notifications and `@mentions`, `tm remember`, `tm human`, `--watch`.
- `tm setup claude` and `tm setup codex`, showing a diff before writing.

Done when:

- the agent-usability evaluation passes: Claude and Codex, given only `tm --help`, complete a scripted set of tasks (claim the next ready Issue, hand it off, comment, record a Finding once Phase 5 lands), and every stumble is either fixed or logged as an Issue
- two agents working at once never both hold a Claim

## Phase 4: Schemes and Workflows in full

Custom issue types and field types, requirements per status, description templates, Definition of Done, acceptance criteria and plan snapshots, link types with readiness options and cycle detection, gates, snooze, `tm doctor` and `tm fix`, Scheme change previews, and the setup wizard with Presets.

Done when a Jira-style Preset and a Minimal Preset both install and validate, and a Scheme change shows its impact before it is applied.

## Phase 5: git integration and Findings

Branch and trailer linking, closing branches, the orphaned-branch and reopen rules, Development panel data, Findings with anchors, relocation, staleness, fingerprints and states, and the review-cycle ledger importer.

Done when this project's own commits and reviews flow into `tm` without manual copying.

## Phase 6: releases, automation and Checks

Components with path globs, Releases from tags, Ships in, Found in, Planned bump against bump files, release notes, Iterations, declarative automation rules with the shipped rule set, and Checks with Evidence.

Done when claude-plugins' tags and bump files produce correct Ships in values and bump-mismatch flags.

## Phase 7: search, attachments and the rest of the TUI

The search palette (fuzzy, FTS5, frecency), the full filter language, attachments, Swimlanes and per-column filters, the release view, the Development panel and the dependency graph.

## Phase 8: web UI

`tm ui` on localhost with a token, TanStack DB over `tm serve`'s change stream, built on the UI component library, with shareable View URLs. Adds a dependency-review workflow (from claude-plugins) covering npm.

## Phase 9: sync (experimental)

`tm serve` as the Key authority, operation exchange, provisional Keys, `tm sync status` verified against the server, and `detach`. Sync brings tm's first credentials, so this phase removes the `rust/cleartext-logging` exclusion from `.github/workflows/codeql.yml`.

## Release tooling (before the first published version)

oakum config generated with `oakum init` (do not copy `tool-version` from ui or claude-plugins; it is deprecated), cargo-dist pinned through mise with macOS and Linux targets only, and the oakum release workflow with a GitHub App token.

v1 is Phases 1 to 8 complete with sync still experimental.
