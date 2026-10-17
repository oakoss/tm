# Git-native and plain-file task trackers

Scope: Backlog.md, git-bug, git-issue, Fossil tickets, todo.txt, Taskwarrior/TaskChampion, Radicle issues (COBs), plus others found: wedow/ticket (`tk`), dstask, git-appraise (git notes), beads (metadata only). Research date 2026-10-16. Repo metadata (stars, license, last push, latest release) comes from `gh api repos/<owner>/<repo>` and `gh api repos/<owner>/<repo>/releases/latest`, run this session.

## Q1. Storage format, merge/conflict handling for concurrent writers, and ID generation

### Takeaway
The tools split into three storage families. Each one fails differently under concurrent writers:
1. **Snapshot files in the working tree** (Backlog.md, tk, todo.txt, dstask, git-issue). These depend on git's line merge, which breaks on frontmatter, and, where IDs are sequential, on IDs allocated as max+1.
2. **Operation logs as git commit DAGs under custom refs** (git-bug, Radicle COBs). These merge without conflicts using Lamport clocks or causal order with a deterministic tie-break.
3. **Operation logs synced through a server, outside git** (TaskChampion, Fossil tickets). These use a linear version chain with operational transform (TaskChampion) or timestamp-ordered change artifacts (Fossil).

The most useful single data point: Backlog.md, the most popular Markdown-frontmatter tracker in this set (beads has more stars but a different storage model), has been tested in production and shown to mint duplicate sequential IDs across clones. Its maintainer declined to fix this with random IDs and instead shipped detection and repair.

### Cited Findings

**Backlog.md**
- Each task is one Markdown file with YAML frontmatter, named `<prefix>-<n> - <Title>.md` under `backlog/tasks/` (or `.backlog/`, or a custom dir set in `backlog.config.yml`). Example frontmatter keys from a current task (BACK-769, 2026-10-16): `id, title, status, assignee[], created_date, updated_date, labels[], dependencies[], references[], modified_files[], priority, type, ordinal`. — [BACK-769 task file](https://github.com/MrLesk/Backlog.md/blob/main/backlog/tasks/back-769%20-%20Keep-generated-task-comments-as-plain-Markdown-text.md)
- The body uses machine-delimited sections: `<!-- SECTION:DESCRIPTION:BEGIN -->`, `<!-- AC:BEGIN -->` with numbered checkboxes `- [x] #1 ...`, `<!-- DOD:BEGIN -->`, and `<!-- SECTION:PLAN:BEGIN -->`. The tool can therefore round-trip structured fields that sit inside free Markdown. — same source
- IDs are sequential with a configurable prefix (`TASK-1` by default; this repo uses `BACK-`), with optional `zeroPaddedIds`. Subtasks use decimals (`task-4.1`). — [README](https://github.com/MrLesk/Backlog.md#readme); [ADVANCED-CONFIG.md](https://github.com/MrLesk/Backlog.md/blob/main/ADVANCED-CONFIG.md); [backlog/tasks/readme.md](https://github.com/MrLesk/Backlog.md/blob/main/backlog/tasks/readme.md)
- ID collisions (issue #711): "Sequential ID allocation (`max + 1`) cannot be made safe in Backlog.md's own target topology ... two writers creating tasks between one another's pushes deterministically mint the same ID." `withCreateLock()` (a `proper-lockfile` mutex) "only serializes writers on the same filesystem." The reporter "hit this in production twice" and proposed `idGeneration: random` (`<prefix>-<8 hex>`). Maintainer's answer: "We are keeping stable, incremental numeric task IDs. PR #749 shipped collision diagnosis and recovery, not collision prevention. A random or collision-free ID mode is not planned." — [Issue #711](https://github.com/MrLesk/Backlog.md/issues/711)
- A user pushed back on random IDs: "much more convenient to reference a few digits in a task than 8 hex digits." — [Issue #711 comments](https://github.com/MrLesk/Backlog.md/issues/711)
- Issue #997 (open): "task create reuses an id already claimed by an archived task." In v1.54.0, archive keeps the ID reserved, and `task delete` releases it only "once no other branch or worktree still holds the task." — [Issue #997](https://github.com/MrLesk/Backlog.md/issues/997); [v1.54.0 release](https://github.com/MrLesk/Backlog.md/releases/tag/v1.54.0)
- Ambiguous IDs "fail closed everywhere" (v1.50.0). Dependency graphs show "missing references, ambiguous IDs, and cycles ... as such rather than resolved to a guess" (v1.51.0). — [Releases](https://github.com/MrLesk/Backlog.md/releases)
- Design task BACK-760 (status To Do, created 2026-10-11) lists measured git facts that any design has to respect:
  - "Writing shared values into the task files of two branches at different times produces merge conflicts, also when the two branches changed different adjacent frontmatter lines."
  - "`git update-ref` with an expected old value lets exactly one of 20 parallel writers win. A push with an expected old value does the same between clones, with one trap: two writers that push byte-identical objects are both told they succeeded, so every write needs something unique to its operation."
  - "A ref outside `refs/heads` is not copied by a plain clone, and a fetch through a wildcard refspec with `--prune` deletes the local copy when the remote lacks it."
  - "With five writers sharing one assignee name, treating 'already has the requested value' as success reported 40 winners for 8 tasks."

  The planned design records "only task IDs and assignment" in "a Git ref outside the branches". Other metadata resolves last-writer-wins, and task files are never rewritten by sync. — [BACK-760](https://github.com/MrLesk/Backlog.md/blob/main/backlog/tasks/back-760%20-%20Share-task-IDs-assignment-and-status-across-branches-and-worktrees.md)
- Community fork with claims (issue #1063, open): a worker "acquires a claim: one atomic conditional write to `refs/claims/<ticket>`. It doesn't commit and leaves the task file alone." — [Issue #1063](https://github.com/MrLesk/Backlog.md/issues/1063)
- Bug: an uncommitted edit to a task file made it 409 "ambiguous" when `check_active_branches` was enabled (closed). — [Issue #818](https://github.com/MrLesk/Backlog.md/issues/818)

**git-bug**
- Entities are stored as "a series of edit `Operation`s", following the operation-based CRDT idea. An `OperationPack` (one edit session, a JSON blob of ops with type, timestamp, random nonce, and payload) is a git Blob at `/ops` in a Tree. Media goes under `/media`. Each pack is a Commit, and the chain sits at `refs/<namespace>/<id>`. — [data-model.md](https://github.com/git-bug/git-bug/blob/master/doc/design/data-model.md)
- Namespaces in use: `refs/bugs/*` and `refs/identities/*`. — [Makefile](https://github.com/git-bug/git-bug/blob/master/Makefile)
- Ordering uses Lamport clocks, encoded as empty-blob tree entries (`create-clock-14`, `edit-clock-137`). "You can't rely on the time provided by other people ... for anything other than just display." — [data-model.md](https://github.com/git-bug/git-bug/blob/master/doc/design/data-model.md)
- Merging: when a pull meets local changes, git-bug "creates the equivalent of a merge commit" to form a DAG. Commits whose clocks violate the DAG are refused. Operations are ordered by Lamport clock, then by "the lexicographic order of the OperationPack's identifier". That tie-break "doesn't carry much meaning, but it's unbiased and hard to abuse." — same source
- IDs: `id = hash(json(op))`, and an entity ID is the hash of its first operation. The UI shows 7 characters and accepts any unambiguous prefix. — same source
- v0.11.0 (2026-09-22) fixed cache races, "a commit path that was not atomic", and "an index not updated on merge". One example: "#1582 — pushing to GitHub and pulling back duplicated every comment." — [v0.11.0 release](https://github.com/git-bug/git-bug/releases/tag/v0.11.0)

**Radicle COBs**
- "Each COB is a directed acyclic graph (DAG) of Git commits. Each modification is stored as a separate Git commit object." COBs live under `refs/cobs`, namespaced by type (`xyz.radicle.issue`, `xyz.radicle.patch`, `xyz.radicle.id`). — [Radicle protocol guide](https://radicle.dev/guides/protocol)
- Concurrency: "the commit graphs are simply unioned with each other in a non-destructive, idempotent way". State comes from replaying "in a deterministic and causally-consistent order". The simplest merge function sorts by commit hash, and "If a merge function cannot be defined, a COB may be configured to treat this as a conflict." — same source
- IDs: "a unique type name in reverse domain name notation and a unique Object ID". The guide does not say how the Object ID is generated. — same source

**TaskChampion (Taskwarrior 3 storage and sync)**
- Operations are `Create(uuid)`, `Delete(uuid)`, and `Update(uuid, property, value, timestamp)`. Versions (UUID-identified) form "one branch-free chain". The server accepts a version only if its parent is the latest, and it does not allow "force push"-style rewrites. When it falls behind, a replica rebases its local ops with operational transform: "Both transformation paths must reach exactly the same final state." — [TaskChampion sync model](https://gothenburgbitfactory.org/taskchampion/sync-model.html)
- A task is "a key/value map with string keys and values". Tags are `tag_<tag>`, annotations `annotation_<timestamp>`, and dependencies `dep_<uuid>`. Per the docs, storing each one under its own key lets a single change add or remove one item "without overwriting others during sync". — [TaskChampion task model](https://gothenburgbitfactory.org/taskchampion/tasks.html)
- IDs are UUIDs (Create(uuid)). — [sync model](https://gothenburgbitfactory.org/taskchampion/sync-model.html)

**Fossil tickets**
- "Each ticket change artifact corresponds to a single change to a ticket." It holds the ticket ID, timestamp, user, and key/value pairs. Current state comes from applying changes in timestamp order. TICKET and TICKETCHNG are SQL tables derived from the artifacts: "The content of the two ticket tables can always be reconstructed from the ticket change artifacts." Most fields are replaced, while comments append. — [Fossil tickets.wiki](https://fossil-scm.org/home/doc/trunk/www/tickets.wiki)
- IDs: `tkt_uuid` is a large random hex value ("appear to be SHA1 hash strings, but they are not really the hash of any identifiable artifact"). The integer `tkt_id` may change on `fossil rebuild`. — same source

**git-issue (dspinellis)**
- Data lives under `.issues/` as a separate nested git repo (`.issues/.git`), with one directory per issue: `issues/xx/xxxxxxx...`. Each field is its own file: `description`, `duedate`, `weight`, `timespent`, `timeestimate`, `tags` (one per line), `milestone`, `watchers`, `assignee`, plus `comments/` and `attachments/`. — [git-issue README](https://github.com/dspinellis/git-issue#readme)
- IDs: issues and comments are "specified through the SHA hash associated with the parent of the commit that opened them, which is specifically crafted for that element". — same source

**todo.txt**
- One task per line in a plain text file. Rule 1: priority `(A)`–`(Z)` comes first. Rule 2: an optional creation date `YYYY-MM-DD` follows. Rule 3: `@context` and `+project` can appear anywhere after that. Lines are designed to sort with plain `sort`: completed tasks start with `x`, which "sorts at the bottom". — [todo.txt format](https://github.com/todotxt/todo.txt#readme)

**wedow/ticket (`tk`)**
- "Tickets are markdown files with YAML frontmatter in `.tickets/`". IDs look like `nw-5c46` (prefix plus short hex) with partial matching. It was written as "a full replacement for beads ... without the need for keeping a SQLite file in sync or a rogue background daemon mangling your changes." — [ticket README](https://github.com/wedow/ticket#readme)

**dstask**
- Taskwarrior-like, but "uses git to synchronise instead of a special protocol". It keeps a "markdown note page per task", stores data in a separate `~/.dstask` git repo, and offers "Git powered sync/undo/resolve (passwordstore.org style)". Its README sets expectations: "If you're at home in the terminal, and with solving the occasional merge conflict, then dstask is for you!" — [dstask README](https://github.com/naggie/dstask#readme)

**git-appraise (git notes pattern, code review rather than issues)**
- Data is stored in git-notes under `refs/notes/devtools/*` (reviews, ci, ...). Each item is "a single line of JSON ... at most one such item per line. This allows the git notes to be automatically merged using the 'cat_sort_uniq' strategy." A `v` field versions the schema. — [git-appraise README](https://github.com/google/git-appraise#readme)

### Inferences
- Per-key storage turns most concurrent edits into non-conflicts at the field level. TaskChampion's `tag_x`/`dep_<uuid>`, git-issue's one-file-per-field, and git-appraise's one-JSON-line-per-item under `cat_sort_uniq` all use this idea. A frontmatter array like `labels: [a, b]` edited on two branches is a textual conflict in git. BACK-760 observed exactly that with adjacent frontmatter lines.
- Hash-of-first-op IDs (git-bug), random UUIDs (TaskChampion, Fossil), and prefix+short-hex IDs (tk) need no coordination. Sequential IDs are what humans prefer (#711), so a hybrid is worth evaluating: stable random IDs with a locally assigned short alias, or sequential numbers allocated through an atomic ref CAS.

### Gaps
- Radicle's Object ID derivation (likely the hash of the initial change commit) is not stated in the guide I fetched, so it is unverified.
- How tk resolves ID collisions or merges, and the exact length and alphabet of its IDs, were not checked.
- git-issue's behavior on concurrent edits to the same field file (presumably a plain git merge conflict inside `.issues/.git`) is not documented in the README.

## Q2. Where data lives (code repo, separate ref/branch, elsewhere), privacy and leak risk, lock-free reads

### Takeaway
Working-tree file trackers put tasks into every push of the code repo, so a public remote publishes them by default. Custom-ref trackers (git-bug, Radicle, Backlog.md's planned shared ref) stay off a remote until explicitly pushed, because a plain clone or push does not copy refs outside `refs/heads` and `refs/tags`. TaskChampion and Fossil keep data outside the code repo entirely, and TaskChampion encrypts it end to end.

### Cited Findings
- **Backlog.md**: tasks are in-repo files ("every task is a plain `.md` file in your repo"). `autoCommit` defaults to false. `remoteOperations` (default true) runs git fetch, and `checkActiveBranches` (default true, `activeBranchDays` 30) reads task state across branches. Filesystem-only mode exists (`backlog init --no-git`). — [README](https://github.com/MrLesk/Backlog.md#readme); [ADVANCED-CONFIG.md](https://github.com/MrLesk/Backlog.md/blob/main/ADVANCED-CONFIG.md)
- Backlog.md performance: v1.50.0 regressed so that on repos with many branches "common CLI task commands could take minutes or hang on remote fetches". In v1.50.1, CLI reads became "local-only by design", while web/board/MCP keep an incremental, cached cross-branch view. Measured: `task list` went from 4.2s to 0.2s, and warm reads from "~394 git operations down to 3 or fewer". — [v1.50.1 release](https://github.com/MrLesk/Backlog.md/releases/tag/v1.50.1)
- Backlog.md cross-branch visibility gap: "A task file changed in one branch or worktree is invisible everywhere else until that branch is merged ... A second agent can take the same task and neither is told." Planned fix: shared records in a ref, readable by every worktree immediately. "Reads never use the network." — [BACK-760](https://github.com/MrLesk/Backlog.md/blob/main/backlog/tasks/back-760%20-%20Share-task-IDs-assignment-and-status-across-branches-and-worktrees.md)
- Backlog.md web server binds to `127.0.0.1` and "is not reachable from other devices on the LAN or VPN". — [README](https://github.com/MrLesk/Backlog.md#readme)
- **git-bug**: data sits in `refs/bugs/*` and `refs/identities/*` inside the code repo but outside branches. It is synced with explicit `git bug push [REMOTE]` and `git bug pull`. Remote-tracking copies appear under `refs/remotes/origin/bugs/`. — [Makefile](https://github.com/git-bug/git-bug/blob/master/Makefile); [git-bug push docs](https://github.com/git-bug/git-bug/blob/master/doc/md/git-bug_push.md)
- **Radicle**: COBs replicate like other repo data via gossip and Git fetch ("Synchronization is handled by Git"). Private repos restrict replication to "a designated group of trusted peers", but "the data is not encrypted at rest". Seeding is opt-in per node policy. — [Radicle protocol guide](https://radicle.dev/guides/protocol)
- **git-issue**: `.issues/` is its own git repo and "should be placed under `.gitignore`, if it will coexist with another Git-based project". It is pushed and pulled separately. — [git-issue README](https://github.com/dspinellis/git-issue#readme)
- **dstask**: separate repo at `~/.dstask` (override with `DSTASK_GIT_REPO`), so it is personal rather than per project. — [dstask README](https://github.com/naggie/dstask#readme)
- **TaskChampion**: replicas never talk to each other directly, and the server "does not have access to the task data". Encryption is ChaCha20-Poly1305 AEAD with a PBKDF2-HMAC-SHA256 key (600,000 iterations), and it is not applied to local on-disk sync. — [sync model](https://gothenburgbitfactory.org/taskchampion/sync-model.html); [encryption](https://gothenburgbitfactory.org/taskchampion/encryption.html)
- **Fossil**: tickets are artifacts inside the Fossil repository and sync with it. "Only the low-level ticket change artifacts are synced." — [Fossil tickets.wiki](https://fossil-scm.org/home/doc/trunk/www/tickets.wiki)
- **git-appraise**: `refs/notes/devtools/*`, named so as to keep "these notes ... not in a human-friendly form" out of the way. — [git-appraise README](https://github.com/google/git-appraise#readme)

### Inferences
- Agents that run git commands themselves (`git add -A`, `git push`) will publish working-tree task files with the code. Custom refs need an explicit refspec. That protects privacy, but it also means teammates do not see the data unless the tool or the remote config pushes and fetches those refs. BACK-760's `--prune` finding shows the refs can even be deleted locally.
- Worktree-aware state is a real requirement for agent fleets. A ref is visible in every worktree immediately, while working-tree files are per-worktree. This is the main reason Backlog.md is moving coordination into refs.
- Lock-free reads come naturally for the git- and file-based designs: git objects are immutable, and files can be read without locking. Backlog.md uses a lockfile only for creating and editing. TaskChampion's replica storage is "transaction-protected, with the expectation of a serializable isolation level" ([storage](https://gothenburgbitfactory.org/taskchampion/storage.html)), so reads go through the storage backend's transactions. That this backend is SQLite, with SQLite locking semantics, is unverified (see Gaps).

### Gaps
- I did not verify whether GitHub, GitLab, or other forges accept pushes of arbitrary `refs/bugs/*` or `refs/cobs/*` refs, or show them in their UIs. Commonly reported as accepted but invisible; unverified.
- Fossil private-ticket and access-control behavior was not covered by the page fetched.
- The TaskChampion storage page describes only the interface (`Storage`/`StorageTxn`; tasks by UUID, last synced version, pending operations, a local-only small-integer to UUID map). It does not name an engine. That Taskwarrior 3 uses a SQLite replica comes from prior knowledge and is unverified here. The local-only integer map explains why Taskwarrior's numeric IDs are unstable. — [TaskChampion storage](https://gothenburgbitfactory.org/taskchampion/storage.html)

## Q3. Typed fields, schema and validation, custom attributes

### Takeaway
Schema strictness ranges from none (todo.txt `key:value`, TaskChampion string maps) to configured enums (Backlog.md statuses, priorities, and projects with case-insensitive validation) to a fixed small field set (git-bug: title, status, labels, comments). Fossil is the only one with a fully user-defined, SQL-backed schema. Taskwarrior UDAs are the best-known custom-attribute mechanism.

### Cited Findings
- **Backlog.md**: configured values (for example `projects`) are accepted "case-insensitively and store the configured spelling in task frontmatter". Dates are stored canonically as `yyyy-mm-dd [hh:mm]` UTC, and `dateFormat` affects display only. Agent-facing output (`--plain`, MCP) "always stays canonical". Projects can set Definition of Done defaults (`definition_of_done:` list) that are added to every new task. A per-task `onStatusChange` shell hook exists in frontmatter. — [ADVANCED-CONFIG.md](https://github.com/MrLesk/Backlog.md/blob/main/ADVANCED-CONFIG.md); [README](https://github.com/MrLesk/Backlog.md#readme)
- Backlog.md advises using its commands instead of hand-editing "so field types and metadata stay consistent". — [README](https://github.com/MrLesk/Backlog.md#readme)
- **TaskChampion**: "Any key not recognized is a user-defined attribute (UDA) ... The application that defines a UDA also defines its value format." Any map, even an empty one, is a valid task. Defined keys: status (pending, completed, deleted, recurring, iterative), description, entry, modified, start, end, wait. — [TaskChampion task model](https://gothenburgbitfactory.org/taskchampion/tasks.html)
- **Fossil**: TICKET table columns that start with `tkt_` are internal. Other columns are user-defined and should match artifact keys. The docs recommend adding fields rather than altering the defaults. Views are scripted in TH1. — [Fossil tickets.wiki](https://fossil-scm.org/home/doc/trunk/www/tickets.wiki)
- **git-bug**: the CLI surface is `bug new/title/status (open|close)/label/comment` plus `user`. "Project configuration (valid labels, ...)" and "Board" support are listed as not implemented or partial. — [git-bug CLI docs index](https://github.com/git-bug/git-bug/tree/master/doc/md); [feature-matrix.md](https://github.com/git-bug/git-bug/blob/master/doc/feature-matrix.md)
- **git-issue**: fixed set of tags, milestone, weight (positive integer), duedate (ISO-8601), timeestimate and timespent (seconds), assignee, and watchers. — [git-issue README](https://github.com/dspinellis/git-issue#readme)
- **tk**: type (bug|feature|task|epic|chore), priority 0–4, assignee, `--external-ref`, parent, tags, design, acceptance, and deps/links. — [ticket README](https://github.com/wedow/ticket#readme)
- **todo.txt**: priority, creation and completion dates, `@context`, and `+project` are structured. Anything else is free text ("the fields you can attach to each task are only limited by your imagination"). — [todo.txt format](https://github.com/todotxt/todo.txt#readme)
- **git-appraise**: notes conform to JSON schemas (`schema/request.json`, `schema/ci.json`) with a `v` version field. — [git-appraise README](https://github.com/google/git-appraise#readme)

### Inferences
- A useful middle ground for a new tracker: a typed core (status, priority, type, assignee, deps, acceptance criteria) validated against project config, plus an open namespace for custom keys in TaskChampion's UDA style. Keys should be stored separately so that independent edits merge cleanly.

### Gaps
- Taskwarrior UDA configuration syntax (`uda.<name>.type` = string|numeric|date|duration, plus `.values` and `.label`) is from prior knowledge. I did not re-fetch taskwarrior.org docs this session, so it is unverified.

## Q4. Git, branch, and PR integration; bridges to GitHub, GitLab, and Jira

### Takeaway
git-bug has the most complete bridges (GitHub, GitLab, Jira, Launchpad import and export). git-issue has GitHub and GitLab import and export. Backlog.md has no built-in bridge; a third-party Jira plugin exists, and the maintainer prefers external layers. Backlog.md's integration is workflow-level instead: one task per context window per PR, plus cross-branch state reading.

### Cited Findings
- **git-bug** "can import from and export to Github, Gitlab, Jira and Launchpad", via `git bug bridge new|pull|push|rm` and `bridge auth add-token`. — [git-bug README](https://github.com/git-bug/git-bug#readme)
- git-bug bridge bug fixed in v0.11.0: GitHub round-trip duplicated comments because comment metadata was not deduplicated (#1582). — [v0.11.0 release](https://github.com/git-bug/git-bug/releases/tag/v0.11.0)
- **git-issue**: `git issue import` / `export` / `exportall` / `create` for GitHub and GitLab, authenticated with `GH_CURL_AUTH` / `GL_CURL_AUTH` tokens. Import mapping is kept under `imports/<provider>/<user>/<repo>/<number>/sha` with a `checkpoint` file. — [git-issue README](https://github.com/dspinellis/git-issue#readme)
- **Backlog.md + Jira**: issue #392 proposed bidirectional sync. A contributor built [Backlog.md-jira-plugin](https://github.com/eciuca/Backlog.md-jira-plugin) as a separate CLI with 3-way merge. The maintainer suggested Jira fields in frontmatter, with "the plugin just solves the sync and conflicts", and avoids adding SQLite. The issue is closed. — [Issue #392](https://github.com/MrLesk/Backlog.md/issues/392)
- **Backlog.md branch awareness**: `checkActiveBranches` reads task state across branches active within `activeBranchDays`. The recommended flow is "one task = one context window = one PR". — [README](https://github.com/MrLesk/Backlog.md#readme); [ADVANCED-CONFIG.md](https://github.com/MrLesk/Backlog.md/blob/main/ADVANCED-CONFIG.md)
- **Radicle**: patches are a COB type (`xyz.radicle.patch`) that point to both the identity graph and the source commits, so PRs and issues share one storage model. — [Radicle protocol guide](https://radicle.dev/guides/protocol)
- **dstask**: has an import tool for GitHub issues and Taskwarrior tasks. — [dstask README](https://github.com/naggie/dstask#readme)
- **tk**: `--external-ref` (e.g. `gh-123`, `JIRA-456`) as a field, plus `migrate-beads`. — [ticket README](https://github.com/wedow/ticket#readme)

### Inferences
- git-bug's experience (bridge metadata dedupe bugs) suggests storing external IDs as first-class per-entity metadata from day one, so that round trips stay idempotent.

### Gaps
- I did not fetch git-bug's per-bridge feature matrix rows (which fields and directions each bridge supports).

## Q5. Agent interface: JSON output, MCP servers, AI-agent features

### Takeaway
Backlog.md is the most developed agent integration in this set. It offers versioned JSON CLI output with a `--watch` stream, an MCP server with workflow resources, generated agent instruction files, acceptance criteria and Definition of Done, implementation-plan sections, and a spec → plan → code review loop. It now defaults to CLI instructions rather than MCP. git-bug offers `--format json` and a GraphQL API. Taskwarrior relies on third-party MCP servers. tk targets agents with a minimal bash CLI and a `ready` command.

### Cited Findings
- **Backlog.md agent workflow**: "three review checkpoints". (1) The agent decomposes the idea into tasks with acceptance criteria and milestones. (2) The agent writes an implementation plan into the task, and a human approves it. (3) "one task = one context window = one PR." Nearly all of Backlog.md's own code "is written by AI agents working through Backlog.md itself". — [README](https://github.com/MrLesk/Backlog.md#readme)
- `backlog init` offers three AI setups. "CLI instructions (recommended)" creates a short instruction file telling agents to run `backlog instructions overview`. The "MCP connector" option auto-configures Claude Code, Codex, Gemini CLI, Kiro, and Cursor. The third option is Skip. It "preserves existing AGENTS.md content". — [README](https://github.com/MrLesk/Backlog.md#readme)
- **Backlog.md MCP**: `backlog mcp start`, a single user-scope server that "finds the active project from your client's MCP roots, and re-resolves when you switch workspace or worktree". `BACKLOG_CWD` pins a project. Resources: `backlog://workflow/overview`, `.../task-creation`, `.../task-execution`, `.../task-finalization`, and `backlog://init-required`. — [README](https://github.com/MrLesk/Backlog.md#readme)
- **Backlog.md JSON**: "stable, versioned JSON" for `task list`, `task view`, and `search`. "JSON mode is noninteractive and keeps successful stdout machine-readable." `task list --json --watch` emits a full list and then full replacements. v1.53.0 cut idle CPU from ~36% to <1% on ~650 files. — [README](https://github.com/MrLesk/Backlog.md#readme); [v1.53.0 release](https://github.com/MrLesk/Backlog.md/releases/tag/v1.53.0)
- Backlog.md also has `--plain` output for agents, a dependency graph with readiness (blocked or ready), and acceptance-criteria progress in JSON, plain, and MCP output. — [Releases v1.50.0–v1.51.0](https://github.com/MrLesk/Backlog.md/releases)
- `onStatusChange` example launches an agent: `'if [ "$NEW_STATUS" = "In Progress" ]; then claude "Task $TASK_ID ... assigned to you" & fi'`. — [ADVANCED-CONFIG.md](https://github.com/MrLesk/Backlog.md/blob/main/ADVANCED-CONFIG.md)
- Multi-agent claim race in Backlog.md: "several agents on one backlog, and two of them start the same ticket before either push lands ... the assignee field can't either, because it only shows up after a push." — [Issue #1063](https://github.com/MrLesk/Backlog.md/issues/1063)
- **git-bug**: `--format` supports `default, plain, id, json, org-mode` on `bug` listing. A GraphQL API backs the web UI and is open to tooling. — [git-bug bug docs](https://github.com/git-bug/git-bug/blob/master/doc/md/git-bug_bug.md); [README](https://github.com/git-bug/git-bug#readme)
- **tk**: "The git-backed issue tracker for AI agents". Agent setup is one line in CLAUDE.md/AGENTS.md ("Run `tk help`"). Markdown files are meant to keep agents from "dumping ten thousand character JSONL lines into their context window". It has `ready` and `blocked` commands and `query [jq-filter]` JSON output, and plugins are `tk-<cmd>` executables on PATH. — [ticket README](https://github.com/wedow/ticket#readme)
- **git-issue**: `git issue dump` outputs the whole database as JSON. — [git-issue README](https://github.com/dspinellis/git-issue#readme)
- **Taskwarrior MCP servers (third party)**: awwaiid's `mcp-server-taskwarrior` wraps the `task` binary and identifies tasks by numeric ID, which Taskwarrior can renumber. A Python "TaskChampion MCP" claims 35 tools. — [awwaiid listing](https://mcp.so/servers/awwaiid_mcp-server-taskwarrior); [TaskChampion MCP listing](https://mcpservers.org/de/servers/gabiup2/taskchampion_mcp) (aggregator sources, not verified against the repos)

### Inferences
- Backlog.md moving its default from MCP to "CLI instructions" (as of the current README) suggests that for coding agents a self-documenting CLI (`instructions overview`) plus stable JSON is enough, and cheaper in context than an MCP tool schema.
- Unstable display IDs are a known agent hazard (Taskwarrior working-set numeric IDs, as noted by the MCP server listing). Agents need IDs that never change.

### Gaps
- None of git-bug, Radicle, Fossil, or todo.txt has a first-party MCP server, as far as found. Absence not exhaustively verified.
- Radicle CLI JSON output was not verified; search turned up only fork example docs.

## Q6. Human UI: TUI, web UI, Kanban

### Takeaway
Backlog.md has the most complete human UI, with a terminal Kanban, a local web Kanban with drag-and-drop, and a TUI. git-bug has a TUI and a newly rewritten web UI but no boards. Fossil has a scriptable built-in web UI. The rest are CLI-only.

### Cited Findings
- **Backlog.md**: `backlog board` (terminal Kanban), `backlog board export` (Markdown report), and `backlog browser` (local web Kanban with drag-and-drop, multi-select, AC checklist editor, real-time updates, and mobile responsive layout). A TUI exists. A community VS Code extension adds an issues panel and Kanban. Groma.md is a companion C4 architecture map that pins tasks to components. — [README](https://github.com/MrLesk/Backlog.md#readme)
- **git-bug**: `git bug termui` (TUI) and `git bug webui`. The v0.11.0 web UI was rewritten "from scratch" and adds URL-encoded filters, themes, a multi-repo picker, and a code browser (file tree, history, diffs) over GraphQL. A public portal with external OAuth is a goal, "not here yet". The Board entity is partial in core and CLI and missing from the TUI and web UI. — [v0.11.0 release](https://github.com/git-bug/git-bug/releases/tag/v0.11.0); [feature-matrix.md](https://github.com/git-bug/git-bug/blob/master/doc/feature-matrix.md)
- **Fossil**: built-in web ticket UI and reports, scripted in TH1. — [Fossil tickets.wiki](https://fossil-scm.org/home/doc/trunk/www/tickets.wiki)
- **dstask**: a terminal UI with screenshots of list, show-resolved, edit, add, and sync. Per-task Markdown notes open in an editor. — [dstask README](https://github.com/naggie/dstask#readme)
- **todo.txt / tk / git-issue**: CLI only (git-issue has `%`-format list output; tk has `dep tree`). — [todo.txt-cli](https://github.com/todotxt/todo.txt-cli); [ticket README](https://github.com/wedow/ticket#readme); [git-issue README](https://github.com/dspinellis/git-issue#readme)

### Inferences
- A local web UI bound to 127.0.0.1 (Backlog.md) or served from the same binary over the API the CLI uses (git-bug GraphQL) gives humans a board without a hosted service. Keeping every UI as an adapter over one core avoids the drift Backlog.md had to fix (ordering and pagination differed between surfaces until v1.54.0).

### Gaps
- Radicle's web frontend (radicle-explorer / desktop app) issue UI was not examined.

## Q7. Stack, license, version, maintenance (as of 2026-10-16)

### Takeaway
Backlog.md (TypeScript, MIT), git-bug (Go, GPL-3.0), Taskwarrior/TaskChampion (C++/Rust, MIT), todo.txt-cli, and Radicle are all actively maintained in October 2026. git-issue is low-activity, and git-appraise has been dormant since 2023. Of the AI-focused file trackers, Backlog.md ships fastest.

### Cited Findings (from `gh api` runs this session)
| Tool | Lang | License | Stars | Latest release | Last push |
|---|---|---|---|---|---|
| [Backlog.md](https://github.com/MrLesk/Backlog.md) | TypeScript (Bun; npm/brew/nix) | MIT | 7,055 | v1.54.0, 2026-10-16 | 2026-10-16 |
| [git-bug](https://github.com/git-bug/git-bug) | Go | GPL-3.0 | 10,772 | v0.11.0, 2026-09-22 (16 months after v0.10.1) | 2026-10-16 |
| [git-issue](https://github.com/dspinellis/git-issue) | Shell | GPL-3.0 | 885 | no GitHub releases; last commit 2025-10-17 | 2025-10-17 |
| [Taskwarrior](https://github.com/GothenburgBitFactory/taskwarrior) | C++ | MIT | 6,120 | v3.5.0, 2026-08-16 | 2026-10-14 |
| [TaskChampion](https://github.com/GothenburgBitFactory/taskchampion) | Rust | MIT | 186 | v3.1.0, 2026-05-30 | 2026-10-16 |
| [todo.txt-cli](https://github.com/todotxt/todo.txt-cli) | Shell | GPL-3.0 | 6,193 | v2.14.0, 2026-09-01 | 2026-10-11 |
| [wedow/ticket](https://github.com/wedow/ticket) | Bash | MIT | 910 | n/a | 2026-03-16 |
| [dstask](https://github.com/naggie/dstask) | Go | MIT | 1,212 | n/a | 2026-05-11 |
| [git-appraise](https://github.com/google/git-appraise) | Go | Apache-2.0 | 5,328 | n/a | 2023-08-12 (dormant) |
| [Radicle heartwood (mirror)](https://github.com/radicle-dev/heartwood) | Rust | Apache-2.0 | 267 | latest tag `releases/1.10.3` | 2026-10-14 |
| [beads](https://github.com/steveyegge/beads) (reference only) | Go | MIT | 27,941 | v1.3.1, 2026-09-30 | 2026-10-16 |

- Backlog.md ships a minor release roughly every 1.5 to 3.5 weeks: v1.50.0 on 08-09, v1.51.0 on 09-02, v1.52.0 on 09-12, v1.53.0 on 09-24, v1.54.0 on 10-16 (gaps of 24, 10, 12, and 22 days). — [Releases](https://github.com/MrLesk/Backlog.md/releases)
- The GPL-3.0 licenses on git-bug, git-issue, and todo.txt-cli limit how their code can be reused in an MIT tool. Their data formats can still be borrowed.

### Inferences
- License matters only for code reuse. The designs are documented well enough (git-bug data-model.md, TaskChampion book) to reimplement from the spec.

### Gaps
- Fossil version and release date were not checked. Radicle's canonical repo lives on Radicle itself; the GitHub mirror's star count understates usage.

## Q8. Known limitations and complaints

### Takeaway
File trackers mostly hit coordination failures: duplicate IDs, claim races, invisibility across worktrees, and the cost of scanning branches. Op-log trackers mostly hit cache and correctness bugs and missing features. Poor visibility on forges is a likely further problem, but unverified. Sync-server trackers need periodic syncs and a server.

### Cited Findings
- **Backlog.md**:
  - Sequential ID collisions across clones; prevention declined and detection plus repair shipped instead. — [#711](https://github.com/MrLesk/Backlog.md/issues/711)
  - Archived-ID reuse (open). — [#997](https://github.com/MrLesk/Backlog.md/issues/997)
  - Cross-worktree invisibility of status and assignee, and claim races between agents. — [BACK-760](https://github.com/MrLesk/Backlog.md/blob/main/backlog/tasks/back-760%20-%20Share-task-IDs-assignment-and-status-across-branches-and-worktrees.md); [#1063](https://github.com/MrLesk/Backlog.md/issues/1063)
  - Cross-branch scanning cost, with minutes-long hangs in v1.50.0. — [v1.50.1](https://github.com/MrLesk/Backlog.md/releases/tag/v1.50.1)
  - Breaking changes to ordering and pagination in v1.54.0 ("Update scripts that depend on the previous order"). — [v1.54.0](https://github.com/MrLesk/Backlog.md/releases/tag/v1.54.0)
  - Open: CLI spins at 100% CPU and ignores SIGTERM after writing to a closed stderr pipe. — [#1048](https://github.com/MrLesk/Backlog.md/issues/1048)
  - Requests for multiple ID prefixes per project. — [#642](https://github.com/MrLesk/Backlog.md/issues/642)
  - Creating tasks "no longer deletes or replaces existing records when filenames and stored IDs disagree" (fixed in v1.54.0), which means the filename/ID duality was a source of data loss. — [v1.54.0](https://github.com/MrLesk/Backlog.md/releases/tag/v1.54.0)
- **git-bug**:
  - Long release gap (16 months).
  - Cache correctness bugs where git-bug "could hand you data that did not match what was stored" (fixed in v0.11.0).
  - No project config or label validation, no PR entity, partial boards, and no public portal yet. — [v0.11.0](https://github.com/git-bug/git-bug/releases/tag/v0.11.0); [feature-matrix.md](https://github.com/git-bug/git-bug/blob/master/doc/feature-matrix.md)
- **dstask**: users must resolve git merge conflicts themselves. — [dstask README](https://github.com/naggie/dstask#readme)
- **tk** positions itself against beads' "SQLite file in sync" and "rogue background daemon mangling your changes", a complaint about DB+JSONL dual-storage designs. — [ticket README](https://github.com/wedow/ticket#readme)
- **git-appraise**: notes are "not in a human-friendly form", and the project is dormant since 2023. — [git-appraise README](https://github.com/google/git-appraise#readme)
- **Radicle**: private repos are "not encrypted at rest". Peer connections use a Noise XK handshake. The guide's network-protocol section states: "Because of a severe security vulnerability in the network protocol, this section was removed. It will be updated once there is a resolution." — [Radicle protocol guide](https://radicle.dev/guides/protocol)
- **TaskChampion**: replicas must sync periodically or pending operations "accumulate indefinitely". A second rejection on the same base version indicates divergence. — [sync model](https://gothenburgbitfactory.org/taskchampion/sync-model.html)
- **Taskwarrior MCP**: numeric IDs can be renumbered. — [mcp.so listing](https://mcp.so/servers/awwaiid_mcp-server-taskwarrior)

### Inferences
- Every coordination complaint about Backlog.md comes from treating working-tree files as the source of truth for facts that need one answer for everyone (ID, owner). The op-log tools avoid this by design but give up plain-file readability.

### Gaps
- I did not survey git-bug's issue tracker for user complaints. Forge invisibility of `refs/bugs` and clone-does-not-fetch are common complaints, not verified this session.

## Q9. Patterns a new tracker for agents and humans could borrow

### Takeaway
The patterns below are synthesized from the evidence above:
- Keep human-readable Markdown task bodies (Backlog.md, tk).
- Put coordination-critical facts (ID allocation, claim/assign) in atomic compare-and-swap writes to git refs outside branches (BACK-760, the #1063 claims fork).
- Model history as an operation log with Lamport ordering and a deterministic tie-break if multi-writer merges must never conflict (git-bug, Radicle).
- Store multi-valued fields per key so that independent edits commute (TaskChampion, git-appraise).
- Expose a self-documenting CLI with stable, versioned JSON, with MCP as optional (Backlog.md).

### Cited Findings (patterns with evidence)
- Atomic ref CAS for claims and IDs: "`git update-ref` with an expected old value lets exactly one of 20 parallel writers win". Pushes with an expected old value arbitrate across clones, but each write must be unique to its operation, or identical pushes both "succeed". — [BACK-760](https://github.com/MrLesk/Backlog.md/blob/main/backlog/tasks/back-760%20-%20Share-task-IDs-assignment-and-status-across-branches-and-worktrees.md)
- Take succeeds only on unassigned: idempotent "already has value = success" produced 40 winners for 8 tasks. — same source
- Never rewrite task files during sync, to avoid frontmatter merge conflicts. — same source
- Operation packs in commits, a Lamport clock in tree entry names, hash-of-first-op IDs, and DAG-respecting clock validation as an anti-abuse measure. — [git-bug data-model.md](https://github.com/git-bug/git-bug/blob/master/doc/design/data-model.md)
- Derived query tables rebuilt from an append-only artifact log (Fossil TICKET/TICKETCHNG), which is the same idea as an index or cache over git objects (git-bug cache). — [Fossil tickets.wiki](https://fossil-scm.org/home/doc/trunk/www/tickets.wiki)
- One-JSON-line-per-item git notes merged with `cat_sort_uniq`. — [git-appraise README](https://github.com/google/git-appraise#readme)
- E2E-encrypted op sync through a dumb server with a linear version chain and OT. — [TaskChampion sync model](https://gothenburgbitfactory.org/taskchampion/sync-model.html); [encryption](https://gothenburgbitfactory.org/taskchampion/encryption.html)
- Agent-facing affordances:
  - Acceptance criteria and Definition of Done checklists.
  - A plan section approved before coding.
  - `ready`/`blocked` queries.
  - Fail-closed ambiguous IDs.
  - Canonical, non-localized machine output.
  - `--watch` JSON streams.
  - MCP roots-based project resolution across worktrees.

  — [Backlog.md README](https://github.com/MrLesk/Backlog.md#readme); [ticket README](https://github.com/wedow/ticket#readme); [Backlog.md releases](https://github.com/MrLesk/Backlog.md/releases)

### Inferences
- A hybrid design is suggested by these failure modes. Task content lives as Markdown in the working tree, so it is reviewable in PRs and travels with the branch. Identity, claims, and status live in a ref-based op log, so they are visible across worktrees and arbitrated atomically. This is roughly where Backlog.md is heading (BACK-760). Its maintainer limited that design to IDs and assignment, without leases or heartbeats. Whether leases are needed for crashed agents is an open design question that #1063 argues for.
- Privacy defaults favor refs: nothing leaks on a normal `git push`. Visibility for teammates then requires the tool to manage refspecs, and BACK-760 notes that `--prune` hazards must be handled.

### Gaps
- No primary-source data on how well git-bug or Radicle op-log performance scales to thousands of entities, beyond git-bug's "Fast indexing ✅" in its feature matrix.
- Jujutsu-adjacent: one search found no built-in jj tracker. A third-party tool, `jjj` (0.4.0 on docs.rs), is described as distributed project management and code review for jj. It syncs via jj git push/pull and keys metadata to jj's stable change IDs, which survive rebases and squashes. This comes from a search summary of [docs.rs/crate/jjj/0.4.0](https://docs.rs/crate/jjj/0.4.0) and was not fetched directly, so it is unverified. The relevant pattern: anchor task-to-code links on change IDs rather than commit SHAs. Gerrit's jj design notes that jj change IDs can be stored as a Git commit header ([Gerrit design doc](https://gerrit.googlesource.com/homepage/+show/HEAD/pages/design-docs/support-jujutsu/use-cases.md), not fetched).
