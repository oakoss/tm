# Agent-first task trackers (beads, Trekker, Taskmaster, and others), state as of 2026-10-16

Method note: repo metadata (stars, open-issue counts, last push, licenses, releases) came from `gh api` on 2026-10-16. npm download counts are for 2026-09-13 to 2026-10-12, from `api.npmjs.org/downloads/point/last-month/<pkg>`. Beads docs were read from the repo at `gastownhall/beads@main`. The `gh api repos/steveyegge/beads` call resolves to `gastownhall/beads`, so the repo has been transferred. Each per-tool section uses bold labels that match the brief's question list.

## Beads (`bd`), by Steve Yegge, now at github.com/gastownhall/beads

### Takeaway
Beads is the most feature-complete agent tracker and among the most widely adopted (star and download counts are in the body; brew installs are not counted), with a hash-ID dependency graph, atomic claims, `bd ready`, gates, compaction, and a large community UI ecosystem. Since March 2026 it runs entirely on Dolt and syncs by publishing `refs/dolt/data` to the code repo's git remote. Its weak points are operational: the embedded store is single-writer behind a file lock, it cannot be read from a read-only sandbox, auto-push shipped on by default for about 7 weeks, push state fails silently, and releases are churny.

### Cited Findings
**History, storage, and sync model**
- First announced as "a coding agent memory system" in a Medium post dated 2025-10-13 (only the search snippet was read, not the full post) — [Yegge, Medium](https://steve-yegge.medium.com/introducing-beads-a-coding-agent-memory-system-637d7d92514a)
- Original "Beads Classic" stored data in SQLite plus git-tracked JSONL. In early February 2026 Yegge moved Beads and Gastown to be backed exclusively by Dolt. The move "added friction" for solo users, who now had to run an external Dolt server. DoltHub then restored a single-player experience: embedded Dolt became the default, behind a `DoltStorage` interface of about 86 methods, with "an exclusive lock" that queues concurrent callers. The restoration took about 120 commits — [DoltHub blog, 2026-04-02](https://www.dolthub.com/blog/2026-04-02-restoring-beads-classic/)
- The CHANGELOG for v0.58.0 (2026-03-02) says: "Beads Classic SQLite backend — the SQLite storage layer and all migration infrastructure have been removed. Dolt is the only backend." The daemon was removed in v0.59.0/v0.60.0. The CHANGELOG lists "Embedded mode by default" (PR #2842) under v1.0.0 (2026-04-02). The two sources disagree: the DoltHub post shows `bd init` already reporting "Mode: embedded" at v0.63.3 — [CHANGELOG](https://github.com/gastownhall/beads/blob/main/CHANGELOG.md)
- Two storage modes. **Embedded** (default) keeps data in `.beads/embeddeddolt/` with a single writer. **Server** (`bd init --server`) connects to `dolt sql-server` for multiple concurrent writers. `.beads/issues.jsonl` is "an export for viewers and interchange, not the source of truth or a backup" — [README](https://github.com/gastownhall/beads/blob/main/README.md)
- Cross-machine sync uses `bd dolt push` / `bd dolt pull`. "On new projects, `bd init` auto-detects `git remote get-url origin` and configures a Dolt remote named `origin`. The first `bd dolt push` publishes `refs/dolt/data`." Fresh clones run `bd bootstrap`. JSONL import "is upsert-only; it cannot infer that records absent from an export were deleted" — [sync-concepts.md](https://github.com/gastownhall/beads/blob/main/docs/core-concepts/sync-concepts.md)
- Every write creates a Dolt commit in embedded mode (`dolt.auto-commit` defaults to `on`). Auto-commit is off in server mode because "Firing DOLT_COMMIT after every write under concurrent load causes 'database is read only' errors" — [dolt.md](https://github.com/gastownhall/beads/blob/main/docs/architecture/dolt.md)
- An experimental HTTP backend (`bd connect https://…` to a `bd serve` server) is documented with a "divergence ledger" of operations that behave differently over the wire — [README](https://github.com/gastownhall/beads/blob/main/README.md)
- `bd init --stealth` / `BEADS_DIR` gives git-free use (`no-git-ops: true`). `bd init --contributor` routes planning issues to a separate repo such as `~/.beads-planning` — [README](https://github.com/gastownhall/beads/blob/main/README.md)

**Sync defaults and unintended publishing**
- Auto-push to the Dolt remote, "automatic push with 5-minute debounce after Dolt commits", was added in v0.57.0 (2026-03-01) — [CHANGELOG](https://github.com/gastownhall/beads/blob/main/CHANGELOG.md)
- PR #3446, "fix(dolt): disable auto-push by default; require explicit opt-in", merged 2026-04-24. A `gh api compare` run shows tag v1.0.3 (2026-04-24) contains the merge commit — [PR #3446](https://github.com/gastownhall/beads/pull/3446)
- The current docs say: "`dolt.auto-push` … default `false` … Auto-push is explicit opt-in because concurrent pushes to git-protocol Dolt remotes can corrupt or strand remote history when multiple writers race." Push state is tracked in a per-machine `.beads/push-state.json`, and "Push failures are warnings only (non-fatal)" — [configuration.md](https://github.com/gastownhall/beads/blob/main/docs/reference/configuration.md)
- `export.auto` (the pre-commit hook writes `.beads/issues.jsonl` so it "lands in the same commit") defaults to `false`. `export.git-add` is also off unless configured — [configuration.md](https://github.com/gastownhall/beads/blob/main/docs/reference/configuration.md); [git-integration.md](https://github.com/gastownhall/beads/blob/main/docs/reference/git-integration.md)
- Issue #3463 (open, 2026-04-24): with `no-git-ops: true` or `--stealth`, bd "still configures a Dolt origin remote" in `repo_state.json`, and "Every write operation … triggers a git push attempt." None of `no-git-ops`, `sync.remote ""`, or `export.auto false` stops it. The workaround is to hand-edit `repo_state.json` — [#3463](https://github.com/gastownhall/beads/issues/3463)
- Issue #6289 (open): on a failed or timed-out auto-push, `push-state.json` `last_push` still advances, and `bd doctor` "Sync Staleness" reports green. The reporter says the only reliable check is `git ls-remote <remote> refs/dolt/data` — [#6289](https://github.com/gastownhall/beads/issues/6289)
- Issue #3370 (open): auto-push without a timeout "hangs indefinitely when remote is unreachable" and holds the embedded `.lock`, which blocks every other `bd` process — [#3370](https://github.com/gastownhall/beads/issues/3370)
- Issue #5157 (open): with 9 concurrent sessions, per-write auto-push self-contends and produces "unknown push error; checksum error" bursts "that present as corruption" — [#5157](https://github.com/gastownhall/beads/issues/5157)
- Issue #5266 (open): `refs/dolt/data` "is destroyed by `git push --mirror`". Issue #6484: bd writes check out `__dolt_remote_info__` in the non-bare working repo, and failed writes can strip tracked files — [#5266](https://github.com/gastownhall/beads/issues/5266); [#6484](https://github.com/gastownhall/beads/issues/6484)

**Concurrency, locking, ID collisions, read-only access**
- "Embedded mode is single-writer (enforced via file lock)." The symptom is "database is locked" errors. The documented fix is to switch to server mode — [dolt.md](https://github.com/gastownhall/beads/blob/main/docs/architecture/dolt.md)
- `bd update <id> --claim` / `bd ready --claim` are atomic: "the first claim wins, and repeating a claim you already hold is idempotent." Merge slots serialize conflict-prone work — [coordination.md](https://github.com/gastownhall/beads/blob/main/docs/multi-agent/coordination.md)
- Hash IDs (`bd-a1b2`) avoid merge collisions. Length adapts to database size: 4 chars up to 500 issues, 5 chars up to 1500, then 6. A collision on import gets a disambiguator — [adaptive-ids.md](https://github.com/gastownhall/beads/blob/main/docs/core-concepts/adaptive-ids.md); [hash-ids.md](https://github.com/gastownhall/beads/blob/main/docs/core-concepts/hash-ids.md)
- Dependency primary keys were made deterministic (migration 0050) after random UUID keys caused unrecoverable `bd dolt pull` merge failures across clones — [CHANGELOG 1.1.0-rc.1](https://github.com/gastownhall/beads/blob/main/CHANGELOG.md)
- A remote-migrate gate refuses to auto-apply schema migrations to a database that has a remote. Exactly one designated clone migrates and pushes; the others run `bd bootstrap` — [CHANGELOG](https://github.com/gastownhall/beads/blob/main/CHANGELOG.md); [README](https://github.com/gastownhall/beads/blob/main/README.md)
- Read-only or sandboxed access does **not** work. Issue #7445 (open, v1.3.1): "`bd --readonly` … inside a sandbox that denies file writes, bd cannot open the store at all, even for a pure read," because the Dolt chunk journal is opened with write access. It reproduces under Codex CLI `--sandbox read-only`. It also references #5561, where strict `--readonly` writes a gate lock file. Issue #3582: sandboxed agents (Claude Code / bwrap) cannot reach a host-level shared Dolt server — [#7445](https://github.com/gastownhall/beads/issues/7445); [#3582](https://github.com/gastownhall/beads/issues/3582)
- Unix-socket server mode is suggested "in sandboxed environments (e.g., Claude Code)", but "Auto-start is not supported in socket mode" — [dolt.md](https://github.com/gastownhall/beads/blob/main/docs/architecture/dolt.md)

**Data model**
- Fields: id, title, description, type, status, priority (0–4), labels, timestamps, plus metadata (`--metadata`, `--set-metadata`, filterable in `bd ready`). Base types: bug, feature, task, epic, chore. Spike, story, and milestone types were added in v1.0.0 (PR #2923). Gate and message types also exist — [issues.md](https://github.com/gastownhall/beads/blob/main/docs/core-concepts/issues.md); [CHANGELOG](https://github.com/gastownhall/beads/blob/main/CHANGELOG.md)
- Dependency types that affect readiness: `blocks` (default), `parent-child` ("children blocked when parent blocked"), `conditional-blocks` ("B runs only if A fails"), and `waits-for` (B waits for all of A's children). Non-blocking types: `discovered-from` and `related`. Graph links: `relates-to`, `duplicates`, `supersedes`, `replies-to`. Includes cycle detection — [dependencies.md](https://github.com/gastownhall/beads/blob/main/docs/core-concepts/dependencies.md); [README](https://github.com/gastownhall/beads/blob/main/README.md)
- Hierarchical child IDs: `bd-a3f8` → `bd-a3f8.1` → `bd-a3f8.1.1` — [README](https://github.com/gastownhall/beads/blob/main/README.md)
- Cross-repo dependencies use `external:<project>:<capability>`, resolved by `bd ship <capability>`, which adds a `provides:` label to a closed issue — [ship.md](https://github.com/gastownhall/beads/blob/main/docs/cli-reference/ship.md)
- Validation is advisory. `bd lint` checks for "missing recommended sections based on issue type": bugs need Steps to Reproduce and Acceptance Criteria, tasks and features need Acceptance Criteria, epics need Success Criteria — [lint.md](https://github.com/gastownhall/beads/blob/main/docs/cli-reference/lint.md)
- `bd defer [--until=tomorrow]` takes items out of `bd ready` while keeping them in `bd list` — [defer.md](https://github.com/gastownhall/beads/blob/main/docs/cli-reference/defer.md)
- Issue #7451 (open): "External blocks edges ignored by ready/blocked/claim" — [#7451](https://github.com/gastownhall/beads/issues/7451)

**Git, branch, and PR integration**
- `bd init` installs git hooks by default (`pre-commit`, `post-merge`, `pre-push`, `post-checkout` as shims to `bd hooks run`). The install is worktree-aware and chains with lefthook, pre-commit, prek, hk, and others — [git-integration.md](https://github.com/gastownhall/beads/blob/main/docs/reference/git-integration.md)
- **Gates** link the graph to PRs and CI: `gh:pr` (resolves when `gh pr view` reports MERGED), `gh:run` (CI success), `timer`, `bead` (cross-rig), and `human` (manual `bd gate resolve`). The stated reason: with Dolt, "issue state is decoupled from code state", so closing a bead does not mean the code is on main — [dependencies.md § Gates](https://github.com/gastownhall/beads/blob/main/docs/core-concepts/dependencies.md)
- Commit linkage: `bd orphans` finds "issues that are referenced in commit messages but remain open or in_progress", and `--fix` closes them after confirmation. This is report-then-confirm, not automatic closing — [orphans.md](https://github.com/gastownhall/beads/blob/main/docs/cli-reference/orphans.md)
- Integrations with GitHub, GitLab (epics sync as GitLab milestones), Jira, Linear, ADO, and Notion — [cli-reference listing](https://github.com/gastownhall/beads/tree/main/docs/cli-reference); [CHANGELOG](https://github.com/gastownhall/beads/blob/main/CHANGELOG.md)

**Triage / inbox**
- `bd human list|respond|dismiss|stats` acts as an inbox of "human-needed beads (issues with 'human' label)". `bd human` with no subcommand prints about 15 essential commands, because "bd has 70+ commands - many for AI agents" — [human.md](https://github.com/gastownhall/beads/blob/main/docs/cli-reference/human.md)
- The `human` gate type blocks dependents until a person resolves it — [dependencies.md](https://github.com/gastownhall/beads/blob/main/docs/core-concepts/dependencies.md)

**Agent interface and context features**
- CLI with `--json` everywhere. `bd prime` prints workflow context and memories. `bd remember "insight"` stores project memory that `bd prime` injects. `bd init` writes or updates `AGENTS.md` and installs Claude Code, Codex, and Cursor integrations unless `--skip-agents` or `--stealth` is passed. `bd setup claude|codex|factory|cursor|mux` — [README](https://github.com/gastownhall/beads/blob/main/README.md)
- MCP server (`beads-mcp` on PyPI) exists, but the docs advise "Prefer CLI + hooks when shell is available." The docs estimate context overhead at about 1–2k tokens for CLI+hooks versus 10–50k for MCP — [mcp-server.md](https://github.com/gastownhall/beads/blob/main/docs/integrations/mcp-server.md)
- Compaction comes in two kinds. `bd admin compact` is semantic "memory decay": it summarizes old closed issues, and "original content is discarded". Modes are analyze, apply (an agent provides the summary), and a legacy auto mode that needs `ANTHROPIC_API_KEY`. `bd compact` squashes Dolt commits older than N days (default 30) and runs GC — [admin.md](https://github.com/gastownhall/beads/blob/main/docs/cli-reference/admin.md); [compact.md](https://github.com/gastownhall/beads/blob/main/docs/cli-reference/compact.md)
- Messaging: a message issue type with `--thread`, plus `bd mail` with pluggable delegates — [README](https://github.com/gastownhall/beads/blob/main/README.md); [community-tools.md](https://github.com/gastownhall/beads/blob/main/docs/community-tools.md)

**Human UI**
- No official UI. The community list includes TUIs (Mardi Gras, perles with BQL, beady-eye, LazyBeads, b9s, btui), web UIs (beads-ui via `npx beads-ui start`, bd-board, BeadBoard, beads-web, Maggie, beadside, which is "a local web reader and inbox … with a waiting-on-you section"), VS Code, Neovim, JetBrains, and Obsidian plugins, native macOS apps (Beadbox, Beadazzle), and a coordination server (BeadHub, whose `bdh` wrapper adds claiming and file reservations). Several older UIs are marked "Historical / Stale" — [community-tools.md](https://github.com/gastownhall/beads/blob/main/docs/community-tools.md)
- beads_viewer (`bv`, Jeffrey Emanuel; Go; 1,715 stars; v0.25.2 on 2026-10-01) is a TUI with graph view, PageRank, critical-path, and cycle insights, plus `--robot-triage` / `--robot-next` / `--robot-graph` agent modes. Its docs warn: "Never run bare `bv` in an agent context — it launches the interactive TUI" — [beads_viewer README](https://github.com/Dicklesworthstone/beads_viewer)

**Stack, license, version, activity, adoption**
- Go, MIT. Latest stable is v1.3.1 (2026-09-30), and v1.3.2-rc.2 was cut 2026-10-10. Last push 2026-10-16, 27,941 stars, 1,470 open issues (all retrieved 2026-10-16) — [repo](https://github.com/gastownhall/beads); [releases](https://github.com/gastownhall/beads/releases)
- Installed via Homebrew (`brew install beads`), `npm i -g @beads/bd` (34,233 downloads in the month to 2026-10-12), and an install script. Docs live at beads.gascity.com — [README](https://github.com/gastownhall/beads/blob/main/README.md); [npm downloads](https://api.npmjs.org/downloads/point/last-month/@beads%2Fbd)
- "v1.2.0 and v1.2.1 were published by accident on 2026-08-11, without release testing. v1.2.2 superseded them by re-releasing the tested 1.1 line." A recovery doc exists for databases migrated by v1.2.1 — [accidental-1-2-1-release.md](https://github.com/gastownhall/beads/blob/main/docs/recovery/accidental-1-2-1-release.md)
- A schema-version guard refuses to run an older binary against a newer schema, with an `BD_IGNORE_SCHEMA_SKEW=1` escape hatch. Upgrading a remote-backed database needs one designated migrator clone — [README](https://github.com/gastownhall/beads/blob/main/README.md)
- The Thoughtworks Technology Radar (April 2026) lists beads as a tool, per a search-result summary of the Portuguese page (not fetched directly) — [Thoughtworks Radar](https://www.thoughtworks.com/pt-br/radar/tools/beads)

**Complaints**
- Rufus Pollock (2026-02-23) wrote that it "runs a daemon, has many, many commands", is "quite invasive: it adds AGENTS.md it add git commit hooks", "got broken in version 50", and that the Dolt route also failed for him (quoted from a search-result summary) — [Pollock blog](https://rufuspollock.com/2026-02-23-issue-tracking-dilemma)
- Jeffrey Emanuel forked a Rust port, beads_rust (`br`), "frozen at the 'classic' SQLite + JSONL architecture", because "the hybrid SQLite + JSONL-git architecture … is being replaced with approaches better suited to Steve's vision" — [beads_rust README](https://github.com/Dicklesworthstone/beads_rust)
- The Trekker author says beads' "codebase has grown quickly without enough care", and cites "concerns about the future and security of that project". The ticket (`tk`) author describes the old beads as needing "a SQLite file in sync" and "a rogue background daemon" — [Trekker README](https://github.com/obsfx/trekker); [ticket README](https://github.com/wedow/ticket)
- The LWN thread "Dolt is LLM slop, though" (May 2026) criticizes Dolt and Gas Town, not beads directly. It quotes Yegge saying he and Claude designed the original Git+SQLite combination in about 15 minutes — [LWN comments](https://lwn.net/Articles/1070928/)

### Inferences
- Beads' sync channel is the user's code remote (`refs/dolt/data` on `origin`), and `bd init` wires that remote automatically. Today, publishing still needs a push (manual `bd dolt push` or opt-in auto-push). Between v0.57.0 (2026-03-01) and v1.0.3 (2026-04-24), auto-push was on by default, so any `bd init` in a repo with an origin could push the task graph to the code host after writes. Issue #3463 shows that even stealth mode kept attempting pushes. A new tracker should make remote publication explicit, per-remote, and visible.
- Every hidden ref, auto-push, and push-state flaw above fails as a warning or false green. The pattern is that sync status lives in local state the doctor does not check against the remote.
- The single-writer embedded lock, combined with opening the store read-write even for reads, rules out the "agent in a read-only sandbox can still query the graph" use case. A plain-file or WAL-reader design would avoid this.
- Beads is strongly agent-optimized (ready queue, atomic claim, gates, compaction, hash IDs) but human-hostile in surface area (70+ commands, so `bd human` exists). The human UX has been outsourced to about 30 community UIs of varying freshness.
- Gates are the most interesting mechanism for "closed in tracker ≠ merged to main". No other tool profiled here has an equivalent.

### Gaps
- No first-class release or fix-version field was found. Only a `milestone` issue type exists, and GitLab sync maps epics to milestones. No file-path field on issues was found in core docs; BeadHub, a community tool, adds file reservations. Both may exist in metadata conventions I did not see.
- I did not determine the exact current semantics of `bd remember`/`bd prime` memory decay beyond the README.
- Full text of Yegge's Medium posts was not fetched (Medium paywall/snippet only). No Hacker News thread on beads was found in search.
- Whether reads in embedded mode take the exclusive lock (as opposed to only failing on write-access to the journal) is not confirmed from code.

## Trekker, github.com/obsfx/trekker

### Takeaway
The agent tracker named "Trekker" is obsfx/trekker, a small Bun + SQLite CLI. Its author built it as a deliberately simpler reaction to beads. It is local-only with no sync model, uses sequential prefixed IDs, and adds full-text search and an audit history. Adoption is tiny and development has stalled since April 2026.

### Cited Findings
- **Identification:** described as a "Simple opinionated issue tracker for AI Agents" (repo description). Other GitHub "trekker" projects are unrelated: dmritrekker/trekker (dMRI fiber tracking) and assorted Star Trek and trekking apps — [GitHub search](https://github.com/obsfx/trekker)
- **Storage and sync:** "Stores tasks, epics, and dependencies in a local SQLite database. No server required." Data lives in `.trekker/trekker.db`, and the README says to "Add `.trekker` to your .gitignore if you do not want to track it". It requires the Bun runtime (`bun:sqlite`) — [README](https://github.com/obsfx/trekker)
- **IDs:** sequential and prefixed (`TREK-1`, `EPIC-1`, `CMT-1`), with prefixes configurable via `trekker init --issue-prefix` / `config set`. "Changing a prefix affects only newly created IDs" — [README](https://github.com/obsfx/trekker)
- **Data model:** epics → tasks → subtasks, and comments. Task statuses: `todo`, `in_progress`, `completed`, `wont_fix`, `archived`. Priority runs 0–5 (5 = "Someday"). `trekker dep add`. `trekker ready` lists top-level `todo` tasks whose dependencies are `completed`, `wont_fix`, or `archived`, and shows what each one unblocks. `epic complete` archives all its tasks — [README](https://github.com/obsfx/trekker)
- **Search and history:** FTS5 full-text search across epics, tasks, subtasks, and comments. `trekker history` is an audit log of creates, updates, and deletes — [README](https://github.com/obsfx/trekker)
- **Agent interface:** a `--toon` flag gives token-efficient TOON output. `trekker quickstart` prints an agent guide. The README does not mention a `--json` flag. A separate Claude Code plugin (obsfx/trekker-claude-code) provides "26 MCP tools, 13 slash commands, 7 skills, 5 lifecycle hooks", "Blocks internal TaskCreate/TodoWrite", and claims "Multi-instance safe with conflict handling". The Codex plugin is at a "very early stage" — [README](https://github.com/obsfx/trekker)
- **Git integration:** "No hook integrations. Just task management." This refers to git hooks; the "5 lifecycle hooks" belong to the Claude Code plugin — [README](https://github.com/obsfx/trekker)
- **Human UI:** a separate `@obsfx/trekker-dashboard` package provides a kanban web UI that reads the same database and auto-refreshes — [README](https://github.com/obsfx/trekker); [trekker-dashboard](https://github.com/obsfx/trekker-dashboard)
- **Motivation:** "I built this after using beads for a while. Beads does the job, but its codebase has grown quickly without enough care… My concerns about the future and security of that project led me here." — [README](https://github.com/obsfx/trekker)
- **Stack, license, activity:** TypeScript/Bun, MIT. Created 2026-01-19. npm `@obsfx/trekker` is at 1.11.0, published 2026-04-10, which is also the last push. 83 stars, 1 open issue, and 154 npm downloads in the month to 2026-10-12 — [repo](https://github.com/obsfx/trekker); [npm downloads](https://api.npmjs.org/downloads/point/last-month/@obsfx%2Ftrekker)

### Inferences
- Sequential IDs in a local, gitignored SQLite file mean there is no multi-machine story. Committing the binary `.db` would conflict on every merge, and two clones would mint the same `TREK-N`.
- Trekker covers the human-readable, simple-surface goal (short IDs, FTS, history, kanban) but gives up distributed agents.

### Gaps
- SQLite journal mode, locking, and concurrent-agent behavior are not documented, and the plugin's "multi-instance safe" claim is unverified. No validation of required fields, release, or file-path awareness, triage, or commit linkage was found.

## Claude Task Master (Taskmaster AI), github.com/eyaltoledano/claude-task-master

### Takeaway
Taskmaster is a PRD-to-tasks generator that calls LLM APIs to parse, expand, and complexity-score tasks, stored in a single `.taskmaster/tasks/tasks.json`. It is MCP-first (36 tools, about 21k tokens by default). Downloads are still high, but the OSS repo has been effectively idle since April 2026 while the project's docs and team features moved to the commercial Hamster product. The single JSON file has documented schema-drift and corruption bugs.

### Cited Findings
- **Storage:** tasks live in `.taskmaster/tasks/tasks.json` (legacy `tasks/tasks.json`) — [src/constants/paths.js](https://github.com/eyaltoledano/claude-task-master/blob/main/src/constants/paths.js)
- **Solo vs team:** solo mode uses "Local JSON files" and tags. Team mode stores tasks in "Hamster cloud" as "briefs" with "Real-time sync" via `tm login` (OAuth), "Requires internet", allows "up to 10 team members", and "Hamster handles all AI calls" — [Hamster docs: team overview](https://tryhamster.com/docs/taskmaster/team/overview)
- **Data model:** fields are `id` (integer), `title`, `description`, `status` (`pending`, `done`, `deferred`, plus in-progress per `next`), `dependencies` (array of IDs), `priority` (`high`/`medium`/`low`), `details`, `testStrategy`, `subtasks` (addressed as `taskId.subtaskId`), and optional `metadata`. `validate-dependencies` checks for invalid or circular dependencies, and `next` picks tasks whose dependencies are satisfied — [Hamster docs: task structure](https://tryhamster.com/docs/taskmaster/capabilities/task-structure)
- **Tags:** tags act as workstreams. `task-master move --from=5 --from-tag=backlog --to-tag=in-progress [--with-dependencies]` — [README](https://github.com/eyaltoledano/claude-task-master)
- **AI-driven commands:** `parse-prd`, `expand`, `analyze-complexity`, and `research`. Three model roles (main, research, fallback). Providers include Anthropic, OpenAI, Gemini, Perplexity, xAI, OpenRouter, MiniMax, and Claude Code / Codex CLI OAuth (no key needed) — [README](https://github.com/eyaltoledano/claude-task-master)
- **Agent interface:** MCP is the "Recommended" path. `TASK_MASTER_TOOLS` controls tool loading: `all` = 36 tools, about 21,000 tokens (the default); `standard` = 15, about 10,000; `core`/`lean` = 7, about 5,000. A CLI (`task-master`/`tm`) and `init --rules cursor,windsurf,vscode` rule files are also available — [README](https://github.com/eyaltoledano/claude-task-master)
- **Concurrency:** writes use `withFileLockSync`, but issue #1585 notes a remaining "read-outside-lock pattern" in `writeJSON` — [#1585](https://github.com/eyaltoledano/claude-task-master/issues/1585)
- **Corruption bugs:** #1708 (open, 2026-06-07): `set-status` converts every `task.id` from number to string and injects `"parentId": "undefined"`, giving "a mixed schema that drifts on every write" (reproduces 100% on 0.43.x). #1637: the MCP server's stale in-memory `currentTag` "causes cross-tag data corruption". #1641: about 370 MB RSS per stdio MCP process — [#1708](https://github.com/eyaltoledano/claude-task-master/issues/1708); [#1637](https://github.com/eyaltoledano/claude-task-master/issues/1637); [#1641](https://github.com/eyaltoledano/claude-task-master/issues/1641)
- **Git and worktrees:** worktree support is an open feature request (#1104, 2025-08). Tag isolation per session via `TM_ACTIVE_TAG` was requested in #1306 (closed). `--ready`/`--blocking` filters were added via #1532 (closed 2025-12) — [#1104](https://github.com/eyaltoledano/claude-task-master/issues/1104); [#1306](https://github.com/eyaltoledano/claude-task-master/issues/1306); [#1532](https://github.com/eyaltoledano/claude-task-master/issues/1532)
- **License:** "MIT License" plus a "'Commons Clause' License Condition v1.0" (GitHub reports NOASSERTION) — [LICENSE](https://github.com/eyaltoledano/claude-task-master/blob/main/LICENSE)
- **Version and activity:** latest release `task-master-ai@0.43.1` on 2026-03-31. The last commits are 2026-04-21/23 (docs links moved to tryhamster.com). Last push 2026-04-28. 28,235 stars, 214 open issues. 73,652 npm downloads in the month to 2026-10-12 — [repo](https://github.com/eyaltoledano/claude-task-master); [commits](https://github.com/eyaltoledano/claude-task-master/commits/main); [npm downloads](https://api.npmjs.org/downloads/point/last-month/task-master-ai)

### Inferences
- The OSS project appears to have pivoted to the commercial Hamster product. The repo has been inactive for about 6 months as of October 2026 while downloads remain high, likely from existing MCP configs that run `npx -y task-master-ai`.
- Sequential integer IDs in one shared JSON file make concurrent branches and agents collide (same IDs, whole-file merge conflicts). No hash-ID or merge strategy is documented, so this is inferred, not measured.
- The tool is strong at planning (PRD decomposition, complexity scoring) and weak as a durable system of record: no git or PR linkage, no audit history, no triage, and a schema that drifts.

### Gaps
- No documented human TUI or web UI in OSS beyond the CLI and `tm list --watch`. No release or file-path fields found. No triage or inbox concept. Exact Hamster pricing not found.

## Claude Code built-in task tools (TaskCreate / TaskGet / TaskList / TaskUpdate)

### Takeaway
Claude Code's task list is a session-scoped checklist that persists across compaction and can be shared across sessions through `~/.claude/tasks/`. It is deliberately minimal and outside the repo. As of v2.1.268 the tools are default-off on models newer than Opus 4.7 and Sonnet 4.6.

### Cited Findings
- The four tools are TaskCreate, TaskGet, TaskList, and TaskUpdate. TaskUpdate "Updates task status, dependencies, details, or deletes tasks". TodoWrite is "Disabled by default in favor of" the Task tools and comes back with `CLAUDE_CODE_ENABLE_TASKS=0` — [tools reference](https://code.claude.com/docs/en/tools-reference)
- Availability: "available by default only on Opus 4.7, Sonnet 4.6, Haiku 4.5, and earlier models… On newer models, Claude keeps track of multi-step work without a written checklist, and the tools' definitions and reminders take up context." Opt in with `CLAUDE_CODE_ENABLE_TODO_TOOLS=1`, `--allowedTools TaskCreate`, or `--tools`. Background and cloud sessions always have the tools. "The default set described here applies in Claude Code v2.1.268 and later" — [tools reference § Task tool availability](https://code.claude.com/docs/en/tools-reference)
- "Tasks persist across context compactions." To share across sessions: `CLAUDE_CODE_TASK_LIST_ID=my-project claude` uses "a named directory in `~/.claude/tasks/`". `Ctrl+T` toggles the list, which shows up to 5 tasks — [interactive mode § Task list](https://code.claude.com/docs/en/interactive-mode)
- Agent teams use a "shared task list" for assigning and claiming. Without the Task tools, agents "coordinate with … team through messages instead" — [tools reference](https://code.claude.com/docs/en/tools-reference)
- Third-party guides (unverified against official docs) describe dependencies set via `TaskUpdate(addBlockedBy=[…], addBlocks=[…])` and warn that reusing one list ID across repos risks cross-project contamination — [claudefa.st guide](https://claudefa.st/blog/guide/development/task-management); [cc.bruniaux.com](https://cc.bruniaux.com/guide/workflows/task-management/)
- A third-party article titles beads "The Agent Memory System That Inspired Claude Code Tasks" (not verified as Anthropic's statement) — [morphllm](https://www.morphllm.com/beads-agent-memory)

### Inferences
- Storage under the home directory (not the repo) means no git history, no team visibility, and no PR linkage. It is a working memory, not a tracker. The move to default-off on newer models suggests the platform expects a durable external tracker, reached by CLI or skill, to fill the gap.
- The Trekker plugin explicitly blocks TaskCreate/TodoWrite to force its own tracker. Competing task surfaces are a known agent failure mode (see Codex below).

### Gaps
- Official docs do not give the on-disk schema, locking behavior for concurrent sessions sharing a `CLAUDE_CODE_TASK_LIST_ID`, or the exact dependency parameter names.

## Codex CLI and Cursor task features

### Takeaway
Both have only ephemeral plan or to-do features, not trackers. Codex made its `update_plan` tool opt-in on 2026-08-31. One blog reports that part of the motive was competition with external trackers.

### Cited Findings
- openai/codex PR #41744, "Make the update_plan tool opt-in", merged 2026-08-31. This is verified via `gh search prs`, and the merged PR contradicts a 2026 guide claiming Codex planning is "on by default" — [PR #41744](https://github.com/openai/codex/pull/41744)
- A blog (secondary) reports that developers using Linear or Jira saw the model split progress updates between `update_plan` and the external tool — [Daniel Vaughan blog, 2026-08-31](https://codex.danielvaughan.com/2026/08/31/codex-cli-update-plan-tool-opt-in-external-planning-competing-surfaces/)
- Cursor plan mode produces a Markdown plan with agent to-dos. Forum reports include "Plan Mode: No Todo generated" for Codex-backed agents and to-do status not syncing when a task runs in a separate agent — [Cursor blog: plan mode](https://cursor.com/blog/plan-mode); [Cursor forum](https://forum.cursor.com/t/plan-mode-its-broken-for-codex-agent-not-generating-to-dos/150314)

### Gaps
- No primary Cursor docs were fetched on to-do persistence or storage. Treat Cursor details as community-reported.

## Other agent-oriented trackers found

### Takeaway
The most relevant comparators are Backlog.md (markdown files in git, a real human UI, and cross-branch ID checks), beads_rust (frozen classic beads, non-invasive by design), and ticket/`tk` (a single bash script, markdown + YAML frontmatter). A long tail of tiny 0–2-star "issue tracker for agents" repos shows the category is crowded but unconsolidated.

### Cited Findings
**Backlog.md (MrLesk/Backlog.md)**
- "Markdown-native Task Manager & Kanban visualizer for any Git repository". Each task is a plain `.md` file under `backlog/` (or `.backlog/`, or a configured path). Includes acceptance criteria and a project-wide Definition of Done checklist, milestones and dependencies, a terminal board (`backlog board`), a web UI (`backlog browser`, drag-and-drop), fuzzy search, and an optional MCP connector for Claude Code, Codex, Gemini CLI, Kiro, and Cursor. "no server, no account, no telemetry" — [README](https://github.com/MrLesk/Backlog.md)
- Cross-branch config keys: `checkActiveBranches`, `remoteOperations`, `activeBranchDays`, `autoCommit`, `bypassGitHooks`, and `zeroPaddedIds`. With `--no-git`, the saved config forces `checkActiveBranches=false`, `remoteOperations=false`, and `autoCommit=false`. The README says to prefer CLI, MCP, or web over hand-editing "so field types and metadata" stay valid — [README § Configuration](https://github.com/MrLesk/Backlog.md)
- The repo has a `scripts/smoke-parallel-task-locking.sh`, and its own backlog contains "back-513 - Investigate stale worktree branch discovery" — [repo code search](https://github.com/MrLesk/Backlog.md)
- TypeScript, MIT, v1.54.0 released 2026-10-16, 7,055 stars, 80 open issues, 87,203 npm downloads in the month to 2026-10-12 — [repo](https://github.com/MrLesk/Backlog.md); [npm downloads](https://api.npmjs.org/downloads/point/last-month/backlog.md)

**beads_rust (`br`), Dicklesworthstone/beads_rust**
- A Rust port "frozen at the 'classic' SQLite + JSONL architecture". Mutations auto-flush `.beads/issues.jsonl`. "It never commits, pushes, pulls, installs hooks, or runs as a background service." Offers `--json` on every command and `--robot` envelopes, plus `br sync --reconcile-additive` and `--expect-plan-sha256` for safe merges — [README](https://github.com/Dicklesworthstone/beads_rust)
- v0.7.6 released 2026-10-15, 1,127 stars. License is "MIT License (with OpenAI/Anthropic Rider)" (GitHub reports NOASSERTION) — [repo](https://github.com/Dicklesworthstone/beads_rust); [LICENSE](https://github.com/Dicklesworthstone/beads_rust/blob/main/LICENSE)

**ticket (`tk`), wedow/ticket**
- "The git-backed issue tracker for AI agents… written as a full replacement for beads." Tickets are "markdown files with YAML frontmatter in `.tickets/`", so agents can search them "without dumping ten thousand character JSONL lines into their context". IDs such as `nw-5c46` are filenames, which makes them clickable in IDEs. A single portable bash script; `query` needs `jq`. Agent setup is one line in `AGENTS.md` — [README](https://github.com/wedow/ticket)
- Shell, MIT, 910 stars, last push 2026-03-16 — [repo](https://github.com/wedow/ticket)

**Long tail (all 0–2 stars, retrieved 2026-10-16)**
- ariesworx/aries (Go CLI plus MCP on Dolt, "shared memory and coordination for AI coding agents"), evangstav/fmw ("one JSONL per project, ready computation, no daemon"), kylesnowschwartz/beads-lite, khughitt/tasks ("one markdown file per task, one binary as the only writer, JSON by default"), codesweep-ai/ledger ("AI agents write JSON records, humans read a generated ledger"), npatten/Clew, cniska/agent-task, and mlimotte/monocle (a beads web UI) — [GitHub repo search](https://github.com/search?q=issue+tracker+for+agents&type=repositories)

### Inferences
- Three storage camps have emerged: (1) a database plus a sync engine (beads/Dolt, aries); (2) a database plus a git-friendly export (beads_rust, Trekker with no export); (3) one plain-text file per item in git (Backlog.md, ticket, khughitt/tasks). The file-per-item camp gives the best human readability and the cleanest git diffs and PR review. It handles concurrent ID minting with random IDs (`tk`) or branch scanning (Backlog.md).
- "One binary as the only writer" (khughitt/tasks) and "never commits/pushes" (beads_rust) are explicit reactions to beads' invasiveness and silent sync. Several authors cite beads' complexity as their reason for building something new.

### Gaps
- No standalone agent-tracker crates of note were found on crates.io besides beads_rust. Download and usage data for ticket and the long-tail repos was not collected. Backlog.md's exact ID-collision algorithm and locking were not read from code.
