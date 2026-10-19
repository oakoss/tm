# Import, export and the beads adapter

- Date: 2026-10-19
- Author: Claude (research pass for Jace Babin)
- Scope: What bead tm-u8g.10 must build: `tm export` and `tm import` over a published JSONL schema, idempotent through `source:id`, with a beads adapter. Its acceptance: an export then import round trip loses nothing, and importing this repo's beads backlog matches `bd list --json` counts, dependencies and statuses. Also covers how to split the bead into pull requests and which choices need an ADR.

Method note: sources were read on 2026-10-19. Repo code was read at `f378d96`. `bd` 1.3.1 (Homebrew) was run in this repo with read commands only: `bd list`, `bd show`, `bd ready`, `bd export` to stdout, and `--help`. From `bd show` on, every command ran under bd's `--readonly` flag. Before and after them, `git status --porcelain --ignored` hashed the same (`1ab47a32…`) and `HEAD` stayed `f378d96`. The first, unflagged `bd list` changed the mtimes of git-ignored files under `.beads/embeddeddolt/` (`noms/manifest`, `noms/journal.idx`); nothing tracked changed. beads and taskwarrior were cloned into a scratch directory and are cited by path at the commits under Sources. A `tm` debug build of `f378d96` ran in a scratch `TM_HOME` with no global git config. Jira and Linear pages were read through WebFetch, which returns summaries, so their quotes are near-verbatim. Nothing ran on Linux. Anything not quoted or measured is labeled **inferred**.

Scripts:

- [scripts/beads_counts.py](scripts/beads_counts.py): `python3 -I beads_counts.py EXPORT.jsonl LIST.json`, given `bd --readonly export` and `bd --readonly list --json --all --limit 0`. It counts statuses, types, priorities and dependency types, lists the fields the data carries, and checks that the two bd outputs agree. People are printed as counts only.
- [scripts/log_roundtrip.sh](scripts/log_roundtrip.sh): `log_roundtrip.sh <tm-binary> <scratch-dir>`. It writes a store with eight of tm's ten operation kinds (`remove` and `rename_project` have no CLI command yet), keeps only its log under a new Replica id, rebuilds, and compares `tm show --json` and `tm list --json` before and after. It then copies the same log into a Workspace that already holds a Project with the same key.

## Question

1. What must `tm export` carry so that a round trip loses nothing? Should it export the operation log, a state snapshot per Issue with its history, or both? What happens on import into an empty store and into one with data, and how can "loses nothing" be checked?
2. What is the published JSONL schema that adapters write? How does import turn records into ordinary validated writes, how are they batched, where is `source:id` stored, and what does a re-import do when the source changed?
3. What does beads 1.3.1 emit for this repo, and what fields exist at all?
4. How does each beads field map to tm: built-in fields, Links, parent, Statuses, issue types, priorities, Keys, timestamps and Actors? What cannot be represented?
5. How do beads, taskwarrior, Jira and Linear import and export?
6. How should tm-u8g.10 be split into pull requests, and which choices need an ADR?

## Sources

Repo (at `f378d96`):

- [design.md](../../specs/design.md): "Import and export", "Workspaces, Projects and config", "Identity", "Built-in fields", "Health"
- [build-plan.md](../../contributing/build-plan.md): Phase 1 done-checks and the switch milestone
- [GLOSSARY.md](../../../GLOSSARY.md); ADRs [0004](../../decisions/0004-one-authority-assigns-keys.md), [0005](../../decisions/0005-operation-log-is-the-source-of-truth.md), [0006](../../decisions/0006-operation-merge-ordering-and-versioning.md), [0007](../../decisions/0007-the-projection-does-not-depend-on-the-scheme.md), [0008](../../decisions/0008-projects-are-identified-by-id-and-keys-are-assigned-once.md), [0009](../../decisions/0009-claims-apply-by-compare-and-set-and-links-are-keyed-canonically.md), [0010](../../decisions/0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md), [0011](../../decisions/0011-the-cli-speaks-one-json-envelope-and-flat-coded-errors.md), [0012](../../decisions/0012-a-human-undoes-their-last-command-without-naming-it.md)
- [workspaces-and-keys.md](../workspaces/workspaces-and-keys.md) (section 5, "Fresh clone in Phase 1"), [core-commands.md](core-commands.md)
- `crates/tm-core/src/{op,hlc,scheme,link,actor}.rs`, `crates/tm-store/src/{schema,lib,apply,read,validate,project}.rs`, `crates/tm-cli/src/{main,show}.rs`, `crates/tm-store/tests/it/{merge,rebuild,support}.rs`

External, pinned:

- beads `gastownhall/beads` @ `v1.3.1` = `c1c4b642ac1c08d8c828007a1c2f96e47e43ef7c`: `internal/types/types.go` (`Issue`, `Dependency`, `Comment`, the `Status`, `IssueType` and `DependencyType` constants), `cmd/bd/export.go`, `cmd/bd/import_shared.go`, `docs/core-concepts/sync-concepts.md`, `docs/cli-reference/{jira,linear}.md`; plus `bd export --help`, `bd import --help` and `bd list --help` as run
- taskwarrior `GothenburgBitFactory/taskwarrior` @ `bd40c6e3ee65594189d583f5a154da172cfd6ff1`: `src/commands/CmdImport.cpp`, `doc/man/task.1.in`
- Jira Cloud, "Import data from JSON": <https://support.atlassian.com/jira-cloud-administration/docs/import-data-from-json/> (WebFetch)
- Linear, "Import issues": <https://linear.app/docs/import-issues> (WebFetch)

## Findings

### 1. What an export must carry

**What the store holds today.** Everything a Workspace holds is in one SQLite file (`store.db`, `workspace.rs:107`):

| Data | Where | Rebuilt from the log? |
|---|---|---|
| Projects, their current key and every key they held | `projects`, `project_keys` | yes, from `create_project` and `rename_project` |
| Issues: built-in fields | `issues` columns | yes |
| Scheme fields | `field_values` | yes |
| Sets (labels, multi-select) | `set_members`, with display spelling | yes |
| Links | `links` | yes |
| Comments and retractions | `comments.retracted` | yes, from `comment` and `retract` |
| Claims | `claims` | yes, from `claim` |
| Keys | `keys`, `key_numbers` | yes, from `assigned_key` on each `create` |
| Per-field winners, unapplied operations | `field_versions`, `unapplied` | yes |
| Undo batches, step-back record | `op_batches`, `undo_batches` | no: local to the Replica (ADR 0010, 0012) |
| Replica id, clock | `meta` | no: local |
| The Scheme | compiled in (`Scheme::built_in()`, `main.rs`) | not data in Phase 1 (ADR 0007) |
| History | the log itself; no history reader exists (`grep -rni history crates/tm-store/src crates/tm-cli/src` finds only a comment in `schema.rs`) | n/a |

The log is the source of truth (ADR 0005), so "everything" is the `ops` table plus whatever is local. The local tables hold undo state only, and ADR 0010 already accepts that "batches are lost when a store is recreated".

**Three layers compared.**

| | (a) The log, verbatim | (b) A state record per Issue, plus history | (c) Both in one file |
|---|---|---|---|
| Carries | every `ops` row: `op_id`, `op_hash`, `v`, `kind`, `replica_id`, `actor`, `on_behalf_of`, Project, subject, `hlc`, `base_seq`, `utc_ms`, `offset_min`, `payload`, `server_seq`, `server_received_ms`, `assigned_key` | current fields, sets, Links, parent, comments, Key, times; history as a separate list | (a) for fidelity, (b) as the published, readable layer |
| Import is | replay, as `Store::rebuild` does, with each `op_hash` checked (`lib.rs`, `replay`) | ordinary writes through `append_all`, with new operations | replay when the log fits this binary; otherwise the records |
| Keeps op identity, Replicas, clocks, retracted text, lost Claims | yes | no | yes |
| Survives an in-place operation format change before v1 | no (see below) | yes | yes, at state level |
| What adapters write | no: adapters have no operations | yes | the record layer |

**Measured: the log alone reproduces the store.** `log_roundtrip.sh` wrote 31 operations covering eight of the ten kinds (not `remove` or `rename_project`, which no CLI command writes yet): `create_project`, `create`, `set_field` (21), `add` (2), `link` (2), `comment` (2), `retract`, and `claim`. It included a parent, labels with mixed case, a symmetric and a directional Link, a retracted comment, an Issue archived by undoing its create, and an Agent's Claim. It then deleted `op_batches` and `undo_batches`, gave the store a new Replica id and a zeroed clock, dropped every projection and forced a rebuild (`PRAGMA user_version = 0`, since tm has no `tm rebuild` command yet). `tm list --json` (archived and not) and `tm show --json` for all four Issues were byte-identical before and after (5,376 bytes), with 0 unapplied (Raw data). This is the rebuild property test (`tests/it/rebuild.rs`, `projections_match_the_log`) seen from the CLI: what the projections show is a function of the log.

**Measured: a state snapshot loses what the log keeps.** In the same run, the retracted comment shows in `tm show --json` with `"body": null, "retracted": true`, because show prints a retracted comment's place and Actor but not its words (`show.rs`). The log keeps the text, and undoing the retraction shows it again (ADR 0010). A snapshot built from `show` therefore cannot restore it. A snapshot also drops:

- the losing take of a contested Claim (kept as `unapplied`, ADR 0009)
- who wrote each change and for whom
- every superseded value

**Timestamps under (b) without help.** `apply::create` sets `created_ms` and `reporter` from the `create` operation's `utc_ms` and `actor`, and every operation raises `updated_ms` to its own `utc_ms` (`apply.rs`). `write_drafts` stamps every operation with the store clock's `now` (`lib.rs`). Re-creating an Issue through `append_all` therefore gives it the import's time as its created time and the importer as its reporter, unless the import controls the clock (section 4).

**The format tension in ADR 0010.** ADR 0010 keeps `CURRENT_VERSION` and `STORE_FORMAT` at 1 until v1, changes the format in place, and says data that must outlive a format change "moves through `tm export` and `tm import`". An export of verbatim operations is tied to the format that wrote it:

- A new hashed envelope field changes every `op_hash`. A newer binary can read old operations only if it recomputes the hash with the field absent, and then it cannot verify the exported hash.
- A changed `Change` payload shape fails `Op::change()` unless the importer translates it. Translating is the code ADR 0010 defers to v1.

So (a) alone cannot be the format-crossing path ADR 0010 promises, and (b) alone loses history and retracted text. (c) is both: replay the log when its `op_format` matches this binary; otherwise import the Issue records and say what was lost.

**Import into an empty store versus one with data.**

- **No public API inserts a foreign operation.** `append_all` stamps every draft with this Replica's id and clock, and the merge tests insert remote rows with raw SQL (`tests/it/support.rs`, `insert_acked`). Log import needs a new Store method that inserts verbatim rows and replays them, sharing `replay`.
- **A store with operations and no Replica id is refused.** The first version of the script deleted the Replica id; `Store::open` then failed with `the store is corrupt: the Replica identity is missing from a store that has operations` (`lib.rs:485`). An importer must create the store, with its own new Replica id, before adding the log.
- **Measured: a log copied into a Workspace that already has data displaces it.** The script ran `tm init --key RT` in a second Workspace, created two Issues there, copied in the first store's 31 operations and rebuilt. Rebuild orders unacknowledged operations by `hlc` (`ORDER BY server_seq IS NULL, server_seq, hlc, replica_id, op_id`). The copied log was older, so its `create_project` took key `RT` first. The Workspace's own `create_project` then went to `unapplied` ("Project key RT belongs to Project 01a15278-c345…"). Both local Issues followed it ("Project … does not exist"), and so did their six `set_field`s, for 9 unapplied operations. All nine were this Replica's own (Raw data). The repo file maps the directory by key (`project = "RT"`), so after the import the directory resolved to the imported Project and the local Issues vanished from every list. Nothing reported it except `tm doctor`.
- **Keys survive replay, and can be renumbered.** ADR 0008 records the Key on the `create` row outside the hash (`assigned_key`), and replay reads it rather than recomputing. A verbatim import of a whole log therefore reproduces every Key, which the measured run confirms (`RT-1`…`RT-4`; the next create got `RT-5`). Because `assigned_key` is not in `Op::envelope()`, an importer could also give incoming creates new Keys without breaking any `op_hash`. That is how design.md's "an import that renumbers incoming Issues and keeps old Keys as source-tagged aliases" can be built later. It is not needed for the switch milestone.
- **The clock must observe the imported log.** `last_hlc` reads only this Replica's operations and `meta.last_hlc` (`lib.rs`, `last_hlc`). The script's next create did order after the log (`hlc ordered after the log|1`), but only because wall time had moved on; nothing observed the copied clocks. An importer must advance the clock with `Hlc::observe(max imported hlc, now)`, capped at now plus 5 minutes (ADR 0006), so a log from a fast clock cannot outrank new local writes. This is **inferred** from the code; the fast-clock case was not run.
- **Server effects and the hash chain.** No operation has a `server_seq` in Phase 1. An export carries `server_seq`, `server_received_ms` and `assigned_key` as they are, so a later sync-era export round-trips too. Verifying a chain against a server is `tm sync status`'s job (ADR 0006), not import's.
- **Re-import of the same export is idempotent by identity.** `ops.op_id` is `UNIQUE` (`schema.rs`). An operation already present with the same `op_hash` is skipped; one present with a different hash is corruption and refuses the import (**inferred**; not run).

**How "loses nothing" can be checked.**

1. **Re-export equality.** Export, import into a fresh Workspace, export again. The two files must be byte-identical apart from the header's export time, because operations are written in canonical replay order and the Issue records are derived from the same projections. This needs no notion of "equal enough".
2. **Rebuild equality.** The importing store's projections equal the source's. `tests/it/support.rs` already has `dump(store)`, and the rebuild proptest's generator already produces random logs, so a property test exports and imports each generated log and compares dumps.
3. **CLI equality,** as `log_roundtrip.sh` measured: `tm show --json` for every Issue and `tm list --json` are byte-identical.

Undo state is outside all three on purpose: after an import, `tm undo` has no candidates and named batches are `not_found` (ADR 0010, 0012). The fresh store's first bare undo should say why rather than just "nothing to undo" (**inferred** useful).

### 2. The published record schema and how import writes it

**Adapters produce Issues, not operations.** A beads, Jira, Linear or GitHub export has current state, comments and sometimes a change list (Jira's JSON has `history` items with `fromString` and `toString`). None has tm's per-field operations, Replica ids or clocks. design.md: "`tm import` reads JSONL against a published schema and validates it like any other write". The published schema is therefore the Issue record layer (b), and the log layer (a) is tm's own.

**One file, records discriminated by `_type`.** beads does this: `bd export` writes `"_type":"issue"` on every line and `"_type":"memory"` for memories (`cmd/bd/export.go:331`, measured in this repo's export: 16 lines, all `issue`). A proposed shape:

| `_type` | Written by | Fields |
|---|---|---|
| `header` | `tm export` | `format: "tm-export"`, `version` (the record schema), `op_format` (`CURRENT_VERSION`), `workspace` (its id), `exported_ms`, `tm` (version) |
| `project` | `tm export` | `id`, `key`, `keys` (every key held), `name` |
| `op` | `tm export` | every `ops` column, `payload` as the exact stored string, since `op_hash` covers its bytes |
| `issue` | `tm export` and every adapter | below |

An `issue` record, as adapters write it:

- `source`: `"<system>:<id>"`, such as `beads:tm-u8g.10`. tm's own export writes `tm:<uuid>`.
- `project` (a key) and optional `key`; `type`, `title`, `description`, `status`, `priority`, `assignee`, `reporter`, `labels`, `archived`.
- `fields`: Scheme fields by id.
- `parent`: a reference.
- `links`: `[{type, target}]`, outward only, as ADR 0009 stores them.
- `comments`: `[{actor, on_behalf_of?, body, at, retracted}]`.
- `created`, `updated`: RFC 3339.
- `history`: an optional list of `{at, actor, field, from, to}`. It is shown, not replayed, and is the place for Jira's `history`.

A reference is another record's `source` in the same file, or an existing Key or UUID. Records may refer forward, so `blocks` targets and parents need not come first.

**Writes are ordinary, validated drafts.** Today's write path checks the things an importer must respect:

- **Unknown fields are refused.** `settable` returns `UnknownField` for a field the Scheme lacks, and computed fields (`key`, `reporter`, `created`, `updated`, `agent`, `claim`) are refused as "computed and cannot be set" (`validate.rs`). The Scheme is compiled in (`main.rs` calls `Scheme::built_in()`; only tests call `add_field` or `with_scheme`). **Phase 1 cannot add Scheme fields for imported data.**
- **A parent must already exist,** and a parent that closes a cycle is refused (`validate.rs`, `IssueReference`). A Link's source must exist too (`link_draft`).
- **Required fields are checked per batch.** `no_worse` refuses a batch that leaves an Issue missing `type`, `title` or `status` (`lib.rs`), so a record's `create` and its required fields share a batch.

So import runs in two passes. Pass one creates each Issue with its fields, labels, `source` and comments. Pass two sets parents and adds Links, once every target exists. A per-record batch keeps each Issue's undo separate and the write lock short; beads writes "in bounded transactions (a few hundred issues each…)" for the same reason (`bd import --help`). A single batch per file makes the import atomic and undoable with one id. At this repo's 16 Issues either is instant; the lock time of one batch over a large Jira export was not measured.

**Where `source:id` lives.**

| Option | Survives rebuild, export and sync | Lookup | Verdict |
|---|---|---|---|
| A Scheme field | yes | `field_values_by_txt` | Phase 1 has no configurable Scheme fields |
| A built-in set field `sources`, element `beads:tm-u8g.10` | yes: it is `add` operations | `set_members_by_elem (field, elem)`, already indexed | holds several sources per Issue, such as a beads id and a later Jira key |
| The `keys` table, as an alias | no: rebuilt only from `assigned_key` | n/a | beads ids do not parse as Keys (`parse_issue_key` wants `PREFIX-N`) |
| A local table | no: lost on export, import and sync | any | breaks idempotency after the switch milestone's own export and re-import |
| A label | yes | yes | pollutes labels, which are the human's |

A built-in `sources` field is the one that fits: one more multi-valued entry in `Scheme::built_in()`, written by `add` operations like labels, kept out of the default `tm show` text and shown in `--json`. design.md's "old Keys as source-tagged aliases" for Workspace merges can use the same field later (`tm:<old key>`).

**Re-import when the source changed.**

| Rule | Who | Result |
|---|---|---|
| Upsert by id, newer `updated_at` wins, a tie keeps local columns | beads (`bd import --help`, `import_shared.go`, `filterStaleImportIssues`) | `created`, updated (with a field summary), `stale_skipped_ids`, `tie_kept_local_ids` |
| Match by UUID; equal means skip, different means modify | taskwarrior (`CmdImport.cpp`, `importSingleTask`) | prints `add`, `mod` or `skip` per task |
| Skip anything already imported from that source into that team | Linear ("Reimporting from the same external source to the same Linear team … will skip any already-imported issues", WebFetch) | skip |

For tm, a re-import finds the Issue by `sources`, maps the record, and compares each mapped field with the Issue's current value:

- Equal fields are skipped, so a re-run of an unchanged file writes nothing.
- A differing field is written as an ordinary operation, so history shows it and `tm undo` reverts it.

This is taskwarrior's add, mod or skip, and the output reports `created`, `updated` (with fields) and `unchanged`. The alternative is Linear's skip-only, which never overwrites tm-side edits but also never picks up source changes. The choice is the user's (Open questions). Because import writes locally, ADR 0006's concurrent-merge rules do not apply: an import is just a later write.

### 3. What beads 1.3.1 emits here

**`bd list --json` is not the whole backlog.** Measured:

- `bd list --json` returned 7 Issues, all `open`.
- `bd list --json --all` returned 16, but its default `--limit` is 50 ("Limit results (default 50, use 0 for unlimited)", `bd list --help`). At 16 Issues that does not bite yet.
- `bd list --json --all --limit 0` returned 16.

The milestone's "check … against `bd list --json`" must mean `--all --limit 0`, or it misses every closed Issue.

**`bd export` is the adapter's input.** `beads_counts.py` over `bd --readonly export` and `bd --readonly list --json --all --limit 0` (Raw data):

- Both name the same 16 Issues with identical `status`, `priority`, `issue_type`, `title`, `updated_at` and dependency edges.
- The export adds `_type`. The list adds `parent`, which it derives from the `parent-child` edge (15 of 15 agree).
- `bd export` includes labels, dependencies and comments ("Each line is a complete JSON object representing one issue, including its labels, dependencies, and comments", `bd export --help`; `export.go:228–230`). `bd list` carries only `comment_count`.

**This repo's data:**

| | Count |
|---|---|
| Issues | 16 |
| Status | closed 9, open 7 |
| Type | task 13, epic 1, decision 1, milestone 1 |
| Priority | 2 for all 16 |
| Dependencies | 35 edges: blocks 20, parent-child 15; none dangling; every edge filed under its `issue_id`; edge `metadata` is `"{}"` on all 35 |
| `blocks` edges by the blocker's status | closed 15, open 5 |
| Comments, labels | 0, 0 |
| Timestamps | 48, all whole seconds in UTC (`Z`) |
| Closed with `closed_at` and `close_reason` | 9 of 9; every reason describes shipped work |
| `started_at` | 7, all on closed Issues |
| Text fields | description 16, acceptance_criteria 15, design 4, notes 2 |
| People | `owner` on 16 (1 distinct, an email), `created_by` on 16 (1 distinct, a name, not an email), `assignee` on 3 (1 distinct, a name) |
| `bd ready` | 6: the epic and five open tasks |

Two creation-time ties matter for Key order. `tm-u8g` and `tm-u8g.1` share `2026-10-17T03:04:01Z`, and `tm-u8g.9` and `tm-u8g.10` share `03:04:09Z`. A plain string sort puts `tm-u8g.10` before `tm-u8g.9`.

**Fields beads has that this repo does not use** (from `internal/types/types.go` at v1.3.1, not measured):

- **Issue.** `spec_id`, `estimated_minutes`, `closed_by_session`, `due_at`, `defer_until`, `external_ref`, `source_system`, `metadata` (arbitrary JSON), `pinned`, `is_template`, `ephemeral`. Also lease fields (`lease_expires_at`, `heartbeat_at`), compaction fields, and molecule, gate and event fields (`await_type`, `waiters`, `mol_type`, `event_kind`, `payload`, …).
- **Comment.** `{id, issue_id, author, text, created_at}`.
- **Dependency.** `{issue_id, depends_on_id, type, created_at, created_by, metadata}`.
- **Statuses.** `open`, `in_progress`, `blocked`, `deferred`, `closed`, `pinned`, `hooked`.
- **Types.** `bug`, `feature`, `task`, `epic`, `chore`, `decision`, `message`, `molecule`, `gate`, `spike`, `story`, `milestone`, `event`.
- **Dependency types.** `blocks`, `parent-child`, `conditional-blocks`, `waits-for`, `related`, `discovered-from`, `replies-to`, `relates-to`, `duplicates`, `supersedes`, `authored-by`, `assigned-to`, `approved-by`, `attests`, `tracks`, `until`, `caused-by`, `validates`, `delegated-from`.

`bd import` says rows with status `tombstone` are skipped (`bd import --help`), so `tombstone` also exists in data, though not in the `Status` constants.

### 4. Mapping beads to tm

**Identity and Keys.**

- Each beads Issue becomes a new tm Issue with a new UUID and the next Key in the importing Project, assigned by the local authority (ADR 0004).
- The beads id goes into `sources` as `beads:<id>`.
- Creating in `created_at` order, ties broken by the id's numeric parts (`tm-u8g` < `tm-u8g.1` < … < `tm-u8g.9` < `tm-u8g.10`), keeps tm's Keys in creation order, which ADR 0004 is for. This repo has no `.config/tm.toml` yet, so the switch import starts at `TM-1` (**inferred**: Key `TM-1` assumes `tm init --key TM`).
- Typing a beads id (`tm show tm-u8g.10`) does not resolve, since Keys must parse as `PREFIX-N`. Resolving through `sources` would be a later convenience (**inferred** useful).

**Fields.**

| beads | tm | Note |
|---|---|---|
| `title`, `description` | `title`, `description` | direct |
| `design`, `acceptance_criteria`, `notes` | appended to `description` as `## Design`, `## Acceptance criteria`, `## Notes` | no Scheme fields until Phase 4; design.md's acceptance criteria and plan fields arrive then |
| `status` | `status` | below |
| `issue_type` | `type` | below |
| `priority` 0–4 | `urgent`, `high`, `medium`, `low`, `none` | both have five levels, so the map is one to one; 0 is beads' "critical" |
| `owner` | `reporter`, through the `create` operation's actor | `owner` is an email, matching tm's default human (git `user.email`, ADR 0010) |
| `created_by` | kept only in the record's provenance | a git `user.name` here, not tm's identity |
| `assignee` | `assignee`, mapped to the human when it names them | here a name, not an email |
| `labels` | `labels` | tm lowercases and keeps the spelling (`apply.rs`, `label_key`) |
| `comments` | comments by `author` at `created_at` | none in this repo, so unexercised |
| `close_reason` | a comment at `closed_at` | no closing-reason field in tm |
| `created_at`, `closed_at` | `created`, and the time of the move to `done` | see Timestamps |
| `updated_at`, `started_at` | not representable as such | see Timestamps |
| `external_ref` | `sources`, as given (`gh-9` becomes `gh:9`, say) | **inferred**; none here |
| `due_at`, `defer_until` | none | snooze and dates are Phase 4 |
| `metadata`, `spec_id`, `estimated_minutes`, lease, molecule and event fields | none | reported per field by `--dry-run` |

**Statuses** (default Workflow: `triage`, `todo`, `in_progress`, `in_review`, `done`, `cancelled`):

| beads | tm | Here |
|---|---|---|
| `open` | `todo`: accepted work, not Triage | 7 |
| `in_progress`, `hooked` | `in_progress` | 0 |
| `blocked` | `todo`: blocking is derived from `blocks` Links | 0 |
| `deferred` | `todo`, plus a label until snooze exists | 0 |
| `pinned` | `todo`, plus a label | 0 |
| `closed` | `done`; beads has no cancelled state, so a reason saying otherwise needs a human | 9 |

**Types** (default: `epic`, `story`, `task`, `bug`, `sub_task`, `spike`, `design`): `task`, `epic`, `bug`, `spike` and `story` map to themselves, `feature` to `story`, and `chore` to `task`. Here, `decision` (1) and `milestone` (1) have no counterpart. The glossary's Design ("a written approach") is the nearest to a decision. A milestone that "marks completion of a set of related issues" (`types.go:724`) is nearest an Epic or a Task. Keeping the original type as a label (`beads-type:milestone`) loses nothing either way. `message`, `molecule`, `gate` and `event` are beads infrastructure that `bd export` leaves out by default ("excluding infrastructure beads", `bd export --help`).

**Dependencies.** A beads edge reads "`issue_id` depends on `depends_on_id`".

| beads type | tm | Here |
|---|---|---|
| `blocks` | Link `blocks` from `depends_on_id` to `issue_id` | 20 |
| `parent-child` | `parent` of `issue_id` is `depends_on_id` | 15 |
| `related`, `relates-to` | `relates_to` (symmetric) | 0 |
| `duplicates`, `supersedes` | same, `issue_id` outward | 0 |
| `discovered-from` | `derived_from`, `issue_id` outward | 0 |
| `caused-by` | `causes`, from `depends_on_id` | 0 |
| `conditional-blocks`, `waits-for` | none until Phase 4's readiness options | 0 |
| `replies-to`, `authored-by`, `assigned-to`, `approved-by`, `attests`, `tracks`, `until`, `validates`, `delegated-from` | none | 0 |

The direction was checked against the data: `tm-u8g.10` lists `tm-u8g.8` as `blocks`, and `.8` (core commands) must land before `.10` (this bead). An edge's `created_by` is lost, and its `created_at` is lost too unless Links are written with a preserved time (see Timestamps).

**Readiness should agree.** `bd ready` lists 6: the epic and the five open tasks whose blockers are closed. tm's ready queue is todo, unarchived, unclaimed, with no open `blocks` source (core-commands.md). After the mapping it should list the same six, because tm does not hold back an Issue with open children. That makes it a cheap extra check for the milestone (**inferred** until run).

**Timestamps.** `Hlc::tick(now_ms)` never moves the clock back; when `now_ms` is behind the last value it advances the counter instead (`hlc.rs:54`, test `tick_never_moves_backwards`). A write whose wall time is in the past therefore still gets an HLC after everything before it. Only its `utc_ms` is historic. ADR 0006 keeps the two apart ("three kinds of time, none standing in for another"; `utc_ms` is "when it happened … for display"). So an import can record the original times through the existing `Clock` trait, which `Store::open_with_clock` takes. The limit is that one batch has one `now` (`append_with` reads the clock once). Preserving `created_at`, `closed_at` and each comment's time needs either:

- one batch per distinct time: create at `created_at`, move to `done` at `closed_at`, each comment at its own time; or
- an optional `at: WallTime` on `Draft`, used only by import, so one batch can carry several times.

With either, `created` equals beads' `created_at`. `updated` becomes the latest preserved time, which is `closed_at` or the newest comment, not beads' `updated_at`. `started_at` is kept only if import writes an `in_progress` step at that time before `done`. Measured times are whole seconds in UTC, so `offset_min` is 0. Nothing here was run with a backdated clock; it is **inferred** from `hlc.rs`, `lib.rs` and `apply.rs`.

**Actors.**

- Writing the `create` operation as the mapped human makes `reporter` right, and `@me` then matches the imported Issues.
- The alternative records the importer, perhaps as an Agent on behalf of the human. That is honest about who wrote the operation, but then every Issue's reporter is whoever ran the import.
- In this repo every person maps to the one human, so the two differ only in principle.
- A comment's writer who does not map to a tm identity keeps their beads name as `actor`, which `check_name` accepts.

**What cannot be represented in Phase 1:** `design`, `acceptance_criteria` and `notes` as separate fields; `close_reason` as a field; `updated_at` and `started_at` exactly; edge timestamps and creators; dependency types with no tm Link type; due and defer dates; `metadata`; and the original type and status where they have no counterpart (kept as labels). `--dry-run` should list each with its count.

### 5. How others do it

- **beads.**
  - `bd export` writes JSONL, one Issue per line with labels, dependencies and comments. It is "not a full database backup and does not capture Dolt branches, commit history, working-set state, or non-issue tables" (`bd export --help`).
  - `bd import` upserts by id. A row rewrites a local Issue only when its `updated_at` is strictly newer; it reports stale and tie rows, and timestamps are "preserved when present in the JSONL" (`bd import --help`).
  - The docs warn that "JSONL import is upsert-only; it cannot infer that records absent from an export were deleted, pruned, or simply never exported" (`docs/core-concepts/sync-concepts.md`).
  - Its Jira and Linear syncs resolve conflicts by "newer timestamp wins", overridable with `--prefer-local` or `--prefer-jira` (`docs/cli-reference/jira.md`).
  - Its export layer is state-only; history lives in Dolt, which export does not carry. That is the dual-storage gap ADR 0005 cites.
- **taskwarrior.** `task import` matches by `uuid`. An existing task is modified only if it differs, ignoring `modified` and any generated `entry` or `end`, and it prints `add`, `mod` or `skip` per task. Input that names one UUID twice is refused (`CmdImport.cpp`). Imported `entry` (created) times are kept. Exports carry state only; the undo log does not travel.
- **Jira (JSON importer, WebFetch).** Records carry an `externalId`, links name `sourceId` and `destinationId` that match those ids in the example, and an Issue may carry `created`, `updated` and a `history` array of `{author, created, items:[{field, fromString, toString}]}`. The page does not say what a re-import does.
- **Linear (WebFetch).** Re-importing from the same source into the same team "will skip any already-imported issues". To re-import, you delete the earlier import, which is possible only for a limited time.

Every one of these is state plus, at best, a shown history. None round-trips an operation log, because none has one. tm's (c) is the log for itself and the state layer for everyone else.

### 6. Conflicts and open decisions

An index; the detail is in Open questions or Implications.

- **`tm init` will refuse in this repo as it stands.** In a scratch repo it failed with `tm: git ignores .config/tm.toml (/Users/jacebabin/.gitignore:142:/.config .config/tm.toml, tab shown as a space), so no clone would see it; change that rule first`. In this repo, `git check-ignore -v .config/tm.toml` prints the same global rule. The switch milestone needs that rule changed, a repo `!` exception, or the `.tm.toml` form first.
- **The milestone's check names `bd list --json`,** which hides closed Issues (7 of 16 here). It needs `--all --limit 0`.
- **ADR 0010 routes format changes through export and import,** but a verbatim log export cannot cross a format change without translation.
- **The build plan lists `tm rebuild`** (Phase 1), and no such command exists; the script forced a rebuild through `PRAGMA user_version`.
- **The fresh-clone question** from workspaces-and-keys.md (refuse until sync, or offer `tm import`) is answered by log import into a missing Workspace under its exported id.
- **Populated-store import of a log** silently displaces local Projects today, if done naively (measured). It must be refused.
- **Phase 1 has no configurable Scheme fields,** so several beads fields land in `description`.
- **`source:id` needs a home,** and none exists. The proposed built-in `sources` field is a Scheme change and a new term.

## Conclusions

- **Export both layers in one JSONL file.** The log, verbatim, is the only thing that loses nothing: rebuilding from it alone reproduced `tm show --json` and `tm list --json` byte for byte, while a state snapshot already loses retracted comment text. Issue records are what adapters can write, what survives a pre-v1 format change, and what a human or `jq` can read. Each layer earns its place.
- **Log import is a restore, not a merge.** It is safe into a store with no data of its own, or one that already holds exactly that log. Into anything else it displaced the local Project and both its Issues (measured). Phase 1 refuses that case; Workspace merge with renumbering, which ADR 0008's `assigned_key` placement makes possible, comes later.
- **Record import is ordinary writes.** Two passes (Issues, then parents and Links), an idempotency key in a built-in `sources` set field, and add, update or skip per record. Original times survive through `utc_ms` without breaking HLC order, since `Hlc::tick` never moves back.
- **The beads adapter reads `bd export`, not `bd list`,** and maps 16 Issues, 15 parents and 20 `blocks` Links with no loss of counts, statuses or dependencies. The losses are field-level and can be listed: design, notes and acceptance text fold into the description, and close reasons become comments.
- **"Loses nothing" is checkable without judgment:** export, import, export again, and require identical files apart from the header time; plus a property test over the rebuild generator's random logs.

## Implications / actions

Recommendations for tm-u8g.10. Nothing here is decided until the user says so.

**Format (`tm-export`, record schema version 1).**

- JSONL, every line an object with `_type`: `header` first, then `project`, then `op` in canonical replay order, then `issue` in Key order.
- `op` records hold every `ops` column, with `payload` as the stored string and hashes and ids as lowercase hex.
- `issue` records are the published schema (section 2), documented in `docs/specs/` with a JSON Schema file, so adapters and agents have something to validate against.
- Local tables (`op_batches`, `undo_batches`, `meta`) are never exported.

**`tm export [FILE]`** writes to stdout by default, and follows ADR 0011 for its result (`--json` reports counts and the path). AGENTS.md has tm write outside its Workspace "only after a confirmed diff", so writing to a named `FILE` needs either that confirmation or a reading of the rule that exempts a path the user typed (Open questions). Stdout avoids the question.

**`tm import FILE`, by content, with no mode flag:**

- **A file with `op` records whose `op_format` matches this binary** is a restore:
  - Allowed when the Workspace the repo file names does not exist on this machine (it is created under the exported id), or when every incoming `op_id` is already present with the same `op_hash` (a no-op).
  - Anything else is refused with a new code, `workspace_not_empty`.
  - Operations are inserted verbatim, every `op_hash` is checked, the projections are rebuilt, and the clock observes the highest imported `hlc` under the 5-minute cap.
- **A file whose `op_format` does not match** falls back to its `issue` records, with a warning naming what is lost: history, op identity and retracted text.
- **A file with only `issue` records** (an adapter's) writes:
  - pass one: one batch per record holding create, fields, labels, `sources`, comments and the close step
  - pass two: parents and Links
  - output: `created`, `updated` and `unchanged`, by Key and source, plus `unmapped` fields with counts
- `--dry-run` runs every check and rolls back, as `append_with(commit: false)` already allows. It prints the field mapping and what each record would do.

**Beads adapter.** `tm import --from beads FILE`, or `bd export | tm import --from beads -`, converts each line into an `issue` record by the tables in section 4 and then runs the record import. `--dry-run` shows the conversion. The switch milestone's check:

- 16 Issues, 7 `todo` and 9 `done`
- 15 parents and 20 `blocks` Links
- every beads id present in `sources`
- `tm ready` lists the six Issues `bd ready` lists

A test fixture is this repo's `bd export`, with the owner email replaced.

**Store changes.**

- `Store::export` (an iterator over projects, operations and Issue records)
- `Store::restore` (verbatim insert, hash check, rebuild, clock observe)
- a built-in `sources` field
- `Draft.at: Option<WallTime>`, or per-time batches
- `tm rebuild` as the build plan lists it, since restore and its tests want it

**Tests.**

- A property test: random log, export, restore, compare `dump`, export again, compare bytes.
- A CLI test: the `log_roundtrip.sh` cases as an integration test.
- `workspace_not_empty` on a populated store.
- A record import of the beads fixture that checks the counts above.
- A re-import of the same fixture that writes nothing.
- A mutation check: dropping `retract` operations from the export must fail the round-trip test.

**Proposed pull requests.** One branch each, each merged before the next starts:

1. **`feat: tm export and restore of the operation log`**: the header, `project` and `op` records, `tm export`, `tm import` for a matching tm export into a missing or identical Workspace, `workspace_not_empty`, clock observation, `tm rebuild`, and the round-trip property and CLI tests. This alone meets "an export then import round trip loses nothing", and it gives the switch milestone its daily backup.
2. **`feat: the Issue record schema, sources and record import`**: the published schema and its doc, `issue` records in `tm export`, the `sources` built-in field, `Draft.at`, two-pass record import with add, update or skip, `--dry-run`, and the format-mismatch fallback.
3. **`feat(cli): the beads adapter`**: `--from beads`, the mapping tables, the fixture, and the milestone checks as a test.

**ADR work, for the user to decide.**

- **The export format and what import does with it:** two layers, restore-only log import with `workspace_not_empty`, records as the published schema and the pre-v1 format bridge. This amends ADR 0010's "moves through `tm export` and `tm import`" with how.
- **Source ids:** the `sources` field, its element syntax, and the re-import rule. design.md's `source:id` and "source-tagged aliases" both point here, and the glossary may want the term (**inferred**).
- **Possibly an amendment to ADR 0006:** an imported operation's `utc_ms` is the source's time, while its `hlc` is when this Replica wrote it.

**Docs to update if accepted:** build-plan.md's milestone check (`bd list --json --all --limit 0`), and workspaces-and-keys.md's fresh-clone open question.

## Open questions

- **Log export across a pre-v1 format change.** Accept that a format change drops history (record fallback), or commit to writing an importer translation for each change before v1, which is the code ADR 0010 deferred?
- **Re-import of a changed source.** Update changed fields (taskwarrior), or skip anything already imported (Linear)?
- **Batches for record import.** One batch per record (short locks, per-Issue undo), or one per file (atomic, one undo id)?
- **Imported times and Actors.** Should imported operations carry the source's times (`utc_ms`) and the mapped human as `actor`, or the import's time and the person running it?
- **`decision` and `milestone`.** Which tm types should these become, given that the original stays as a label either way?
- **Priority 2.** Is bd's default to be read as `medium` or `none`? All 16 Issues here are 2, which is beads' default for `bd create` (core-commands.md, Raw data).
- **`design`, `acceptance_criteria` and `notes`.** Fold them into `description` now, or add those three fields to the built-in Scheme in Phase 1 ahead of Phase 4?
- **Restore into an existing Workspace.** Should restore ever be allowed into a Workspace that has its own data, renumbering incoming Keys as ADR 0008 allows, before sync? Or does that wait for Workspace merge?
- **`tm export FILE`.** Does a path the user names on the command line count as a confirmed write outside the Workspace, or should export write only to stdout?
- **Linux, and a backdated clock.** Nothing here ran on Linux, and the backdated-clock path was read from code, not run.

## Raw data

`beads_counts.py` over `bd --readonly export` and `bd --readonly list --json --all --limit 0` (this repo, 2026-10-19):

```text
export rows: 16; list rows: 16
record kinds (_type): {'issue': 16}
status:
  closed: 9
  open: 7
issue_type:
  task: 13
  decision: 1
  epic: 1
  milestone: 1
priority:
  2: 16
dependency type:
  blocks: 20
  parent-child: 15
dependency edges: 35; target outside the export: 0
edges filed under another issue: 0
list's parent field set: 15; parent-child edges: 15; agree: True
blocks edges by the blocker's status: {'open': 5, 'closed': 15}
issues with more than one parent-child edge: 0
non-empty fields (issues carrying each):
  _type: 16
  comment_count: 16
  created_at: 16
  created_by: 16
  dependency_count: 16
  dependent_count: 16
  description: 16
  id: 16
  issue_type: 16
  owner: 16
  priority: 16
  status: 16
  title: 16
  updated_at: 16
  acceptance_criteria: 15
  dependencies: 15
  close_reason: 9
  closed_at: 9
  started_at: 7
  design: 4
  assignee: 3
  notes: 2
comments: 0; comment_count total: 0
labels: 0
dependency metadata values: {'{}': 35}
timestamps: 48; with fractional seconds: 0; ending in Z: 48
closed: 9; with closed_at: 9; with close_reason: 9
owner: 16 issues, 1 distinct, 1 look like emails
created_by: 16 issues, 1 distinct, 0 look like emails
assignee: 3 issues, 1 distinct, 0 look like emails
distinct (owner, created_by) pairs: 1
list and export name the same issues: True
  status differs on: 0
  priority differs on: 0
  issue_type differs on: 0
  title differs on: 0
  updated_at differs on: 0
  dependency edges agree: True
  keys only in export: ['_type']; only in list: ['parent']
```

Other bd runs:

Counts were read with `python3 -I` from the saved JSON; the person field is redacted:

```text
$ bd --readonly list --json            # default filter: open only
7 issues, statuses ['open']
$ bd --readonly list --json --all      # --limit defaults to 50
16 issues
$ bd --readonly ready --json
6 ['tm-u8g', 'tm-u8g.10', 'tm-u8g.11', 'tm-u8g.12', 'tm-u8g.13', 'tm-u8g.9']
$ bd list --json --all                 # tm-u8g.15's dependencies
[{"issue_id": "tm-u8g.15", "depends_on_id": "tm-u8g", "type": "parent-child",
  "created_at": "2026-10-19T02:12:40Z", "created_by": "<redacted>", "metadata": "{}"}]
$ git status --porcelain --ignored | shasum          # before and after the bd runs
1ab47a32b0cc69fe76a03189d52b5f74cba5c8e0  -
```

Creation order (`created_at`, id), showing the two same-second ties:

```text
2026-10-17T03:04:01Z tm-u8g      epic       open
2026-10-17T03:04:01Z tm-u8g.1    task       closed
2026-10-17T03:04:02Z tm-u8g.2    decision   closed
...
2026-10-17T03:04:09Z tm-u8g.10   task       open
2026-10-17T03:04:09Z tm-u8g.9    task       open
...
2026-10-17T03:04:13Z tm-u8g.14   milestone  open
2026-10-19T02:12:40Z tm-u8g.15   task       closed
```

`log_roundtrip.sh <tm debug build of f378d96> $SCRATCH`:

```text
ops|31|1
op_batches|31
undo_batches|2
replica|01A15278C34472F5A164B49689971BB3
replica after|9B15787317427E2E5975666183BD35AB
unapplied|0
show and list identical after rebuilding from the log alone (    5376 bytes)
RT-5
hlc ordered after the log|1
populated: issues|4
populated: projects|1
populated: keys|4
populated: unapplied|9
populated: reason|Issue 01a15278-c560-76de-8cba-5aadc38e0e1b does not exist|3
populated: reason|Issue 01a15278-c56e-7584-8c7d-5f12667133b0 does not exist|3
populated: reason|Project 01a15278-c53d-71fd-bf1f-153154833ceb does not exist|2
populated: reason|Project key RT belongs to Project 01a15278-c345-738a-a950-6aa1d19d37ec|1
```

Operation kinds in that log: `set_field` 21, `create` 5, `add` 2, `comment` 2, `link` 2, `claim` 1, `create_project` 1, `retract` 1. In the populated Workspace, all 9 unapplied operations were the local Replica's own: its `create_project`, its two `create`s and six `set_field`s. The surviving Project was the imported `roundtrip`. The first version of the script, which deleted the Replica id instead of replacing it, failed on open:

```text
{"cause": "corrupt", "error": "internal", "message": "…/store.db: the store is corrupt: the Replica identity is missing from a store that has operations", "schema_version": 1, "warnings": []}
```

The retracted comment as `tm show --json` prints it, before and after the rebuild:

```text
RT-2 comments: [("kept", retracted False), (body None, retracted True)]
```

taskwarrior `src/commands/CmdImport.cpp` @ `bd40c6e3`, `importSingleTask`:

```text
if (Context::getContext().tdb2.get(uuid, before)) {
  ...
  if (before != task) { ... modHelper.modifyAndUpdate(before, task); std::cout << " mod  "; }
  else { std::cout << " skip "; }
} else { Context::getContext().tdb2.add(task); std::cout << " add  "; }
```
