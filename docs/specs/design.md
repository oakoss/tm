# tm design

`tm` is a local-first issue tracker for one human and their AI agents: Jira-style planning for the human, a complete `--json` CLI for agents, and code awareness (files, branches, releases, review findings) that neither camp of existing trackers offers. Terms are defined in [GLOSSARY.md](../../GLOSSARY.md); the reasons behind the hardest-to-reverse choices are in [docs/decisions/](../decisions/). The research behind it is in [docs/research/](../research/).

## Audience and scope

- Built for the author's own use first, published under oakoss: `@oakoss/tm` on npm, `tm-cli` on crates.io, command `tm`, MIT licensed.
- Single human per machine. Multi-user accounts and permissions wait for the hosted option.
- macOS and Linux. Windows is deferred; WSL2 is the documented route, with the Workspace on the Linux filesystem.

### v1 and later

| v1 | Before v1, behind an experimental flag | After v1 |
|---|---|---|
| Local store, CLI, TUI, sync-ready data model | `tm serve` and sync, for testing | Stable sync, hosted option, Postgres server backend |
| Import and export | | MCP server (`tm mcp`) |
| Git integration from local history | | Agent dispatch from `@mentions`, two-way sync with other trackers |
| Web UI (`tm ui`), built after the TUI | | Commands run by automation rules |

## Architecture

- **One Rust binary** for the CLI, the Ratatui TUI and `tm serve` ([ADR 0002](../decisions/0002-rust-core-with-typescript-web-ui.md)). One monorepo holds a Cargo workspace (`crates/tm-core`, `tm-store`, `tm-sync`, `tm-server`, `tm-tui`, and `tm-cli`, the published crate that builds the `tm` binary), the web app (`web/`, pnpm) and the npm wrapper (`npm/`). Internal crates are `publish = false`; splitting them keeps incremental builds fast. mise pins Rust, Node and pnpm and runs the tasks; git hooks are lefthook, configured by hand.
- **Web UI** in TypeScript, React and TanStack, using the author's UI component library. Built assets are embedded in the binary; it talks to `tm serve` over JSON plus a server-sent change stream feeding a TanStack DB custom collection. TypeScript types are generated from the Rust types.
- **Local store**: plain SQLite (bundled build), written directly by each `tm` process with `BEGIN IMMEDIATE` and a busy timeout; no daemon ([ADR 0001](../decisions/0001-local-store-is-plain-sqlite-without-a-daemon.md)). All database access sits in one storage module.
- **Read-only snapshot**: after write batches the CLI publishes a never-modified copy that sandboxed agents open with `immutable=1`.
- **Operation log is the source of truth** ([ADR 0005](../decisions/0005-operation-log-is-the-source-of-truth.md)): every change is an append-only operation from v1, written in the same transaction as the tables it updates. The tables are projections that `tm rebuild` regenerates from the log; undo, history and sync all read the log.
- **Invariants**: every operation has a unique ID; a take succeeds only on an unclaimed Issue, never on "already has that value"; links have deterministic keys; a Key is never reused, even after `tm purge`.
- **`tm undo <batch>`** reverts one command's writes, named by the batch id every write reports or a unique start or end of it, by appending operations that restore their prior state ([ADR 0010](../decisions/0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md)). A human's bare `tm undo` reverts their own last command, and repeating it steps back ([ADR 0012](../decisions/0012-a-human-undoes-their-last-command-without-naming-it.md)).
- **Sync** (experimental before v1): Replicas exchange operations with `tm serve`, which orders them and assigns Keys. Synced operations form a hash chain, so `tm sync status` reports synced, pending, behind or forked by comparing chain heads with the server; merge, ordering and versioning rules are in [ADR 0006](../decisions/0006-operation-merge-ordering-and-versioning.md). The server runs on SQLite first; Postgres comes later for hosted or multi-user use.
- **Releases**: oakum bump files and changelogs, with cargo-dist building the binaries for crates.io, Homebrew and the npm wrapper, as oakum itself ships.
- **Schema-version guard**: an older binary refuses a store written by a newer one.
- **Watch list**: Turso DB, re-tested at each minor release against the multi-process and sync tests in [turso-deep-dive.md](../research/sync-engine-spike/turso-deep-dive.md) (scripts in `docs/research/sync-engine-spike/scripts/`).

## Workspaces, Projects and config

- A **Workspace** is a store plus its config. The global one lives under `~/.tm/`, as does tm's own config, never `~/.config/tm.toml`, which is the repo file of a git repo at `$HOME`; others can be created for isolation. Issue data never lives in a code repo ([ADR 0003](../decisions/0003-issue-data-lives-outside-code-repos.md)).
- **Repo scope** works like `bd init`: `tm init` in a repo with no Workspace creates a Workspace dedicated to it.
- **Projects** nest freely; a repo maps to a Project through one file, `.config/tm.toml` (or `.tm.toml` at the root, never both). Without the file, the Workspace maps repos by git remote URL. Monorepos map path prefixes to Projects.
- **Per-machine values never go in the repo file**: sync server URLs, hostnames, tokens and paths live in the Workspace or user config, and no command writes them into a tracked file.
- **Config layers**: Workspace defaults, then Project overrides from the repo file. Config can be one file or split with `include`, kept under `.config/tm/` in a repo.
- **Setup wizard**: `tm init` offers **Presets** (for example Minimal, Solo + agents, Jira-style, Release-managed). A Preset only writes config, so everything stays editable.

## Issues

### Identity

- Every Issue has a permanent UUID and a sequential **Key** (`CPL-42`) assigned by one authority ([ADR 0004](../decisions/0004-one-authority-assigns-keys.md)). Provisional Keys (`CPL-~k3f`) exist only while a synced machine is offline and remain aliases forever.
- Ambiguous Key references are refused, never guessed.
- Merging two Workspaces with the same Project key is an import that renumbers incoming Issues and keeps old Keys as source-tagged aliases.

### Built-in fields

Key, type, title, description, status, Assignee, reporter, Agent and Claim, parent, labels, priority, archived, created and updated times, and history. Priority uses five levels (none, urgent, high, medium, low) with renameable labels. Labels compare without case and keep the spelling they were added with. Everything else is a Scheme field.

### Schemes, types and fields

- A **Scheme** defines issue types, their fields, their Workflows and which types may be parents of which. Projects inherit their parent's Scheme unless they override it.
- **Default types**: Epic, Story, Task, Bug, Sub-task, Spike (covers research and investigation) and Design. Any can be removed; hierarchy limits are off unless configured.
- **Field types**: text, rich text, number, date, boolean, single select, multi select, person, version, URL, Issue reference, file paths and code anchor.
- Behaviour comes from fields, not type names: any type can carry a code anchor or file paths.
- **Description templates** per type (for example Steps, Expected and Actual for Bugs), fully configurable.
- **Acceptance criteria** (a hand-ticked checklist) and a **plan** field are available per type. `tm plan CPL-42 --from <file>` snapshots a plan file's contents with history, so it survives after the file is deleted; a hook can capture plans automatically.
- **Schema as data**: `tm scheme show <type> --json` lists every field, allowed value and per-status requirement.

### Workflows

- Each type has a **Workflow**: statuses, allowed transitions and a configurable starting status.
- Every status belongs to one **Status category**: triage, todo, active, done or cancelled. Cross-type features (Triage, ready queue, release notes) read categories, never status names, so cancelled work never counts as shipped. Categories come from the Scheme when read; a status the Scheme does not define reads as triage ([ADR 0007](../decisions/0007-the-projection-does-not-depend-on-the-scheme.md)).
- Workflow rules and required fields are checked when this Replica writes, never on replay: a merge can produce a state no single write allowed, and it is kept rather than refused.
- **Requirements attach to statuses**: "required to enter X", with creation counting as entering the starting status. Agents can file quickly into Triage while work cannot start until fields are complete.
- **Definition of Done**: a Project can define acceptance criteria added to every new Issue, on top of per-type templates.
- Triage Issues are left out of default Views.

### Links and gates

- **Default link types**: blocks, relates to, duplicates, derived from, causes, supersedes. Custom types have an outward name, an inward name and readiness behaviour (blocks, waits for all children, conditional on failure, or none).
- **Gates** wait on outside events: a PR merging, CI passing, a timer, or a human resolving it.
- Parent and child is separate from links and governed by the Scheme.
- **Snooze**: `tm defer CPL-42 --until <date>` or `--until-activity` hides an Issue from the ready queue until the date or the next comment or change.
- Links that would create a blocking cycle are refused, since a cycle silently empties the ready queue.
- The ready queue shows what each Issue would unblock.

### Releases

- Opt-in per Project. A **Component** has its own version line and optional include and exclude path globs (for example `packages/ui/**` excluding `packages/ui/storybook/**`), so an Issue's file paths can infer its Component. Nothing is inferred unless globs are configured; a file in no Component infers nothing, and a file matching two is refused as ambiguous.
- A **Release** is one version, matched from git tags (default pattern `{component}@{version}`, which matches `review-cycle@0.31.0`). Versions come only from tags, never from branch names, so a patch tagged `1.2.1` on a `release/1.2.0` branch ships in `1.2.1`.
- Release notes are generated per Release from the Issues it shipped.
- **Ships in** starts as a plan and is confirmed when the closing commit lands in a tagged Release. **Found in** records where a Bug was observed.
- **Planned bump** (patch, minor or major per Component) is compared with the bump files the Issue's branch adds (oakum/changesets format), and mismatches are flagged.

### Iterations

Optional time-boxed periods (sprint, cycle or any name), off by default, usable as a filter (`iteration:@current`) or Swimlane.

### Comments, mentions and attachments

- Comment threads in Markdown, writable by humans and agents, recorded in the operation log.
- `@mentions` resolve to Actors and create **Notifications**, shown in a personal list separate from Triage. Mentioning an agent only notifies for now.
- Attachments are stored once per content hash in the Workspace, with a configurable size limit (25 MB default). The web UI previews images; the TUI shows them inline in terminals that support it and otherwise opens them externally.

### Deletion

Everything is soft-deleted: archived, hidden and restorable. `tm purge` is the only permanent removal; agents cannot purge by default, which is configurable. Nothing is ever summarised away.

## Agents

### Interface

- Everything a human can do, an agent can do through the CLI. Every command supports `--json` (or `TM_JSON=1`) with a stable, versioned output schema.
- Errors are structured and carry the fix, for example `{"error":"missing_fields","target":"in_progress","fields":[{"name":"area","allowed":["cli","tui","web"]}],"fix":"tm move CPL-42 in_progress --set area=<value>"}`. In a terminal, humans are prompted for the missing values instead.
- Bare `tm` opens the TUI only on a terminal; without one it prints help, so an agent never hangs in an interactive screen. `tm human` prints the short list of commands people need.
- List commands accept `--watch` to stream changes instead of polling.
- `tm prime` prints a compact context block for SessionStart hooks: the Scheme, the caller's Claims, the ready queue, Notifications, and open Findings or Path Claims touching files in the current diff. It warns when an agent session has no identity set.
- `tm remember "<insight>"` stores project memory that `tm prime` includes.
- `tm setup <agent>` adds the hooks for prime, heartbeat and identity for Claude Code, Codex and others, and turns off the agent's built-in todo list so progress is tracked in one place. It shows the exact diff and writes nothing until confirmed; `tm setup <agent> --undo` removes exactly what it added.
- Agent guidance ships as a skill inside the package (`tanstack-intent` convention) rather than edits to AGENTS.md or CLAUDE.md.
- **Guarantees**: `tm` never runs `git commit`, never edits AGENTS.md or CLAUDE.md, never changes git config (including `core.hooksPath`), and writes files outside its Workspace only after a confirmed diff.
- MCP is optional and comes after v1.

### Identity

- The human is set once (`tm whoami`), defaulting to git `user.email`.
- Agents are detected from the variables their harness sets (Claude Code, Codex, Gemini CLI, OpenCode, Cursor), or named with `TM_AGENT`; otherwise the writer is the human. `TM_ACTOR` overrides the human's name and never names an Agent ([ADR 0010](../decisions/0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md)).
- Agent writes are recorded as "agent, on behalf of human". Sub-agents get distinct names, from `tm setup claude`'s hook or from `TM_AGENT`, so history shows which agent did what.

### Claims

- `tm start CPL-42` claims the Issue for the Agent and keeps the human as Assignee. Its side effects (moving to the first active status, setting the Assignee, creating a branch named after the Key) are each configurable and opt-in.
- A Claim protects the work: status, description, Checks and the Agent. Another Actor is refused with the claimant and expiry, unless it passes `--force`, which is recorded and notifies the claimant. Comments, links, Findings, attachments and fields such as priority stay open to anyone, and the claimant is notified of changes.
- Within a Replica, every write carries the version it was based on; a conflicting change to the same field is refused with the current value. Changes that meet during sync merge by [ADR 0006](../decisions/0006-operation-merge-ordering-and-versioning.md) instead.
- Claims expire after 2 hours by default (configurable, or disabled per Workspace). Any write renews them, as does `tm heartbeat` from a hook; `--for <duration>` and `--no-expiry` cover long jobs. Expiry clears the Agent, leaves the status alone and sends a Notification.
- `tm handoff CPL-42 --to <agent> "<note>"` transfers a Claim with context.
- **Path Claims** (`--paths`, or the Issue's file-path field) warn, not block, when another agent is about to edit the same files.
- `tm ready --claim` atomically takes the next ready Issue.

## Findings

- A Finding is its own kind of record attached to the Issue under review, not an Issue, following the model of code-scanning alerts rather than issues: most findings are fixed or rebutted within the same change, and making each one an Issue would flood Boards. Deferring a Finding creates a full Issue linked as *derived from* it. Findings show on the reviewed Issue and in their own Views.
- Each Finding has severity and category as Scheme fields, a source Actor (Claude, Codex, a human, a linter), a fingerprint for deduplication, one or more code anchors, optional suggested-fix text and a review-round field.
- A code anchor stores path, line range, commit, a hash of the lines and the snippet text.
- On `tm show`, in `tm doctor` and when a commit touches the anchored file, anchors are relocated: an exact match moves quietly, a close match asks for confirmation, and no match flags the Finding stale with a was-versus-now diff.
- States: open, fixed (links the commit), rebutted (needs a reason) and deferred (creates a follow-up Issue). Stale is a flag on top of any state.
- `tm list --path <file>` returns open Findings and Issues touching a file.
- The importer brings in the review-cycle ledger.

## Checks and Evidence

Off by default. An Issue can carry **Checks**: commands with an expected result. `tm verify CPL-42` runs them and records **Evidence**: command, exit code, output tail, commit, Actor and time. A Workflow can require passing Evidence at the current commit before a done status.

## Git and VCS integration

- **No git hooks needed**: each `tm` command first scans mapped repos for commits since it last looked and processes their trailers and closing branches, so commits from other machines, GUIs and rebases are seen without installing anything.
- **Optional hooks the user wires**: `tm hook prepare-commit-msg` (adds `Refs:` from a Key in the branch name), `tm hook commit-msg` (warns on unknown or ambiguous Keys) and `tm hook post-commit` (processes a commit immediately). `tm hook print lefthook` prints a snippet to paste; `tm` never writes hook config.
- Works from local git with no host required: a branch containing a Key links automatically, a `Refs: CPL-42` trailer links, and a `Closes: CPL-42` trailer closes the Issue once that commit reaches a closing branch (the default branch unless more patterns, such as `release/*`, are configured).
- An Issue with several linked PRs moves only when the last of them reaches the state.
- A GitHub provider (GitLab later) reuses `gh` auth and adds PR and CI state for gates and transitions. It refreshes on sync, on opening the TUI, or by webhook under `tm serve`.
- The **Development panel** collects an Issue's branches, commits, PRs and CI runs. It is shown in the TUI and web UI; agents fetch it on request (`tm show CPL-42 --dev --json`).
- PR descriptions can be filled with `Closes` lines from linked Issues.

## Automation rules

- Declarative trigger, condition and action entries in config. Triggers: created, transitioned, field changed, link resolved, git event, schedule. Actions: set field, transition, create Issue, add link, claim, notify.
- **Every rule is off by default**; Presets switch bundles on. Shipped rules:
  - a `Closes:` commit reaching a closing branch moves the Issue to done
  - a PR opening moves the Issue to review (with the GitHub provider)
  - all Sub-tasks done notifies the Assignee
  - a Finding going stale notifies the reviewed Issue's author
  - Claim expiry releases the Claim and records a comment
  - parent follows children (first child active moves the parent to active; all done moves it to review)
  - blocker cleared notifies the Assignee
  - stale in progress flags active Issues with no activity for N days
  - orphaned branch flags a merged branch whose Issue did not close
  - reopen suggestion on a new commit referencing a done Issue
  - bump mismatch flags a branch whose bump file differs from the Planned bump
  - release cut notifies Issues in ready for testing to verify in that Release
  - duplicate closes moves a duplicate to done with a pointer to the original

## Search and filters

- One filter language everywhere (CLI, Views, Board columns, Swimlanes, TUI and web UI), in the style of GitHub qualifiers: `key:value` for any built-in or Scheme field, lists (`status:review,testing`), negation (`-type:spike`), comparisons (`priority:>=high`, `created:>2026-10-01`), `OR` and parentheses when needed, functions (`@me`, `@agent`, `@next-release(review-cycle)`, `@today-7d`), `sort:` and bare words for full-text search. Unknown fields or values fail with the valid options.
- Qualifiers: a repeated qualifier means all of them and a comma list means any of them; `-field:a,b` means none of them and also matches Issues with no value. `no:field` and `has:field` test for a value (priority `none` counts as unset), `category:` filters by Status category, `@me` is the human the filter runs for, and `sort:field` takes the field's natural direction (priority highest first, dates newest first) unless suffixed `-asc` or `-desc`, with unset values last; with no `sort:`, results come most recently updated first. Archived Issues are left out unless the filter names `archived`. Names, enumerated values and people ignore case. Labels and people are free-form: one no Issue holds matches nothing rather than failing. A date field matches one exact day; text search and timestamp comparisons are not supported yet. Syntax not supported yet is refused with a structured error rather than read some other way, so nothing accepted today changes meaning later; error spans count bytes.
- The UIs offer filter chips that stay in sync with the text.
- A search-as-you-type palette (`/` or `Ctrl-K`, `tm find` in the CLI) covers Issues, comments, Findings, file paths, attachment names, Views and Projects. It combines fuzzy matching on Keys and titles (a Rust fuzzy matcher such as `nucleo`, to be evaluated), SQLite FTS5 on bodies and comments, and frecency ranking.

## Views and UIs

- A **View** is a saved, named filter and layout, picked from a sidebar. A **Board** is a View laid out as status columns, each column optionally with its own filter. **Swimlanes** are optional (off by default), group by any field and can have their own filter. Any number of Views per Project, defined in config and editable from the TUI and web UI, which write back to the same config.
- **First TUI screens**: Issue list with filters, Issue detail with history, Triage, Board, ready queue. A release view, the Development panel and a dependency graph with critical path follow.
- The web UI encodes a View's filter in its URL so it can be shared or bookmarked.
- **`tm ui`** binds to `127.0.0.1` only and opens a URL carrying a random token.

## Import and export

- `tm import` reads JSONL against a published schema and validates it like any other write. Adapters for beads, Jira, Linear and GitHub convert into that format; agents can write converters for anything else.
- `--dry-run` previews how source fields map to the Scheme; the TUI edits and saves the mapping.
- Re-runs are idempotent through stored `source:id` metadata.
- `tm export` writes everything that can be exported (Issues, history, comments, attachments, config) in the format `tm import` reads; a round-trip test runs in CI.

## Health

- Changing a live Scheme previews its impact before applying it ("makes 14 Issues non-conforming").
- Non-conforming Issues carry `violations[]` in JSON and a marker in lists, and stay editable; only moving into a status that needs the missing field enforces it.
- `tm doctor` groups violations by rule; `tm fix` walks through them or applies a bulk mapping. Field renames are explicit migrations.

## Open items and risks

- Sandboxed agents see data as of the last snapshot, and a crashed writer delays the next one.
- Replaying offline operations on top of the server's and compacting the operation log are not yet designed.
- Copilot and Cursor identity variables are unverified.
- Many users alias `tm` to `tmux`; ship a longer second binary name as a fallback.
