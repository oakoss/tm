# Core commands, JSON output and structured errors

- Date: 2026-10-17
- Author: Claude (research pass for Jace Babin)
- Scope: What bead tm-u8g.8 must build: `create`, `list`, `show`, `move`, `comment`, `ready`, `claim` and `undo`, each with `--json` in a stable, versioned schema and errors that carry the fix. Covers the eight follow-ups in the bead's notes, and how to split the bead into pull requests.

Method note: sources were read on 2026-10-17. Repo code was read at `49b8560`. External repos were cloned shallowly into a `mktemp -d` scratch directory (`$SCRATCH` below) and are cited by path at the commits under Sources. `bd` 1.3.1 (Homebrew), `gh` 2.102.0, `kubectl` 1.36.1 (client only) and `cargo` 1.99.1 were run in `$SCRATCH`. `jj`, `task` (taskwarrior) and a Linear CLI are not installed here, so they are cited from source and docs only. Kubernetes, Cargo, Claude Code and Cursor pages were read through WebFetch, which returns summaries, so their quotes are near-verbatim. The agent environment-variable survey was delegated to a sub-agent that cloned the agents' repos (commits as it reported them, abbreviated) and ran `env` inside this Claude Code session. Codex's variables were then re-read at the full commit below. Only Claude Code was run. Measurements ran on macOS 27.0.1 on an Apple M2 Max with Rust 1.99.1. The probe and `tm` were both built with tm's release profile (`opt-level = "z"`, fat LTO, `panic = "abort"`). Nothing ran on Linux. Anything not quoted or measured is labeled **inferred**.

Scripts:

- [scripts/cli-cost/](scripts/cli-cost/): `run.sh <repo> <scratch-dir>` builds `tm` and the probe into `<scratch-dir>/target` with the same release profile. It measures:
  - `tm` start-up
  - the cost of a `list` built on today's `Store::find` plus `Store::issue`
  - what `Store::open` scans
  - a prototype ready query on a merged cycle
  - the store's error shapes
  - whether flipping an operation undoes it

  The probe depends on `tm-core` and `tm-store` by path, so it measures the real API and stops building if that API moves. Its in-process timings and counts vary by run and machine.

## Question

1. How do gh, jj, kubectl, cargo, beads, a Linear CLI and taskwarrior shape machine output and version it, and what should tm's envelope be?
2. What shape should tm's errors take (code, message, fix, details), with which exit codes, on which stream, and how do `StoreError` variants map to stable codes?
3. How does tm decide the Actor and whether it is an Agent, where does `on_behalf_of` live, and what does `@me` mean for an Agent?
4. What are the semantics and the minimal Phase 1 scope of each command, and which need a new Store API?
5. Which argument and output conventions apply: Key or UUID input, `--json` everywhere, text output, exit codes, no TTY, clap, cold start?
6. How should tm-u8g.8 be split into pull requests, and in what order?
7. What in design.md, the ADRs or the glossary conflicts or is undecided?

## Sources

Repo (at `49b8560`):

- [design.md](../../specs/design.md): "Agents" (Interface, Identity, Claims), "Architecture" (`tm undo`), "Links and gates", "Search and filters", "Health"
- [build-plan.md](../../contributing/build-plan.md): Phase 1 and Phase 3
- [GLOSSARY.md](../../../GLOSSARY.md), [ADR 0006](../../decisions/0006-operation-merge-ordering-and-versioning.md), [ADR 0007](../../decisions/0007-the-projection-does-not-depend-on-the-scheme.md), [ADR 0008](../../decisions/0008-projects-are-identified-by-id-and-keys-are-assigned-once.md), [ADR 0009](../../decisions/0009-claims-apply-by-compare-and-set-and-links-are-keyed-canonically.md)
- [claims-and-links.md](../workspaces/claims-and-links.md), [filter-language.md](../fields/filter-language.md)
- `crates/tm-cli/src/{main,init}.rs`, `crates/tm-core/src/{op,link,scheme}.rs`, `crates/tm-core/src/filter/mod.rs`, `crates/tm-store/src/{lib,claim,read,schema,validate}.rs`, `crates/tm-store/tests/it/links.rs`, the workspace `Cargo.toml`
- `bd show tm-u8g.8` (read-only)

External, pinned:

- beads `gastownhall/beads` @ `4a67d0d0288c1b4ee23e17fc896f47616931ba2d`: `cmd/bd/output.go`, `cmd/bd/errors.go`, `docs/reference/json-schema.md`, `cmd/bd/main.go` (actor precedence, `getOwner`)
- GitHub CLI `cli/cli` @ `fa92838bef95fe4a29c1587693885d5e9bdb6cf1`: `pkg/cmdutil/json_flags.go`; `gh help exit-codes`
- jj `jj-vcs/jj` @ `12643c016a89d338e3d630655af098852636158e`: `docs/templates.md` (`json()`, `Serialize`), `docs/operation-log.md`, `cli/src/commands/undo.rs`
- Linear CLI `schpet/linear-cli` @ `63463ac8a31d7a14b7a7d5964c363853d0089842` (a community CLI, not Linear's own; no official general-purpose Linear CLI was found, **inferred**): `crates/linear-cli/src/error.rs`, `AGENTS.md`, `README.md`
- taskwarrior `GothenburgBitFactory/taskwarrior` @ `bd40c6e3ee65594189d583f5a154da172cfd6ff1`: `src/commands/CmdUndo.cpp`, `doc/man/task.1.in`; taskchampion `GothenburgBitFactory/taskchampion` @ `01cc8a45b7ad173c884a3664ad57b9765e5d648f`: `src/operation.rs`, `src/taskdb/undo.rs`
- Kubernetes API conventions: <https://github.com/kubernetes/community/blob/master/contributors/devel/sig-architecture/api-conventions.md> (WebFetch; the page's last 15% was not read)
- Cargo external tools: <https://doc.rust-lang.org/cargo/reference/external-tools.html> (WebFetch)
- Agent variables:
  - Claude Code: <https://code.claude.com/docs/en/env-vars> (WebFetch; quoted entries checked in the fetched text)
  - openai/codex @ `8c076942cb70fba8e5c4732c23f1faabc4aed484`: `codex-rs/core/src/exec_env.rs`, `codex-rs/protocol/src/shell_environment.rs`
  - google-gemini/gemini-cli @ `ed96faed`
  - sst/opencode @ `63c1a27a`
  - block/goose @ `bfcc6ee6`
  - cline/cline @ `a42df8c8`
  - Aider-AI/aider @ `5dc9490b`
  - `@vercel/detect-agent` 1.2.5 in vercel/vercel @ `c628be78`
  - unjs/std-env 4.3.0 @ `a737979c`
  - Cursor forum threads 132427 and 152451

## Findings

### 1. Machine output and its versioning

| Tool | Shape | Version marker | Breaking change signalled by |
|---|---|---|---|
| gh | Bare value; the caller must name fields (`--json number,title`), and with no fields gh lists the available ones and exits 1 | None | Nothing in the output; field names are the contract |
| jj | Whatever the template builds; `json(value)` serializes a `Serialize` value | None | Docs: names and types are "usually stable across jj versions, but the backward compatibility isn't guaranteed" (`docs/templates.md`) |
| kubectl | Every object carries `kind` and `apiVersion` | `apiVersion` | A new API version; per the conventions page, "adding new enum values breaks backward compatibility" |
| cargo `--message-format json` | One object per line, discriminated by `reason`; ends with `build-finished` | None per message; `cargo metadata` alone is "stable and versioned" with `--format-version` | `--format-version` for metadata only |
| beads | Legacy: objects get `schema_version` injected, arrays stay bare. Opt-in `BD_JSON_ENVELOPE=1`: `{"schema_version":1,"data":…}`, default from v2.0 | `schema_version` integer | "Additive changes (new optional fields) do NOT bump the version" (`docs/reference/json-schema.md`) |
| Linear CLI (schpet) | GraphQL field names; lists are arrays, views are objects (`AGENTS.md`) | None | Nothing (not run; **inferred** from source and docs) |
| taskwarrior | `task export` prints an array of task objects (not checked here; **inferred**) | None | Nothing |

**Measured.**

- `gh issue list -R cli/cli --limit 2 --json number,title,state`, piped (not a terminal), printed a bare, compact, single-line array.
- `kubectl create configmap demo --from-literal=a=b --dry-run=client -o json` printed an object with `kind` and `apiVersion`.
- `bd create "First" -t task --json` printed an object with `"schema_version": 1` mixed into the Issue's own fields.
- `bd list --json` printed a bare array with no version anywhere (Raw data).

**What beads teaches.** Mixing the version into the payload and leaving arrays bare is what beads is now migrating away from. The migration takes an environment variable, a deprecation notice ("NOTE: bd --json output format will change in v2.0", shown only when stderr is a terminal) and a release cycle with an escape hatch (`cmd/bd/output.go`). tm has published nothing yet, so it can start with an envelope and avoid that migration.

**What design.md already says.** "Every command supports `--json` (or `TM_JSON=1`) with a stable, versioned output schema." The one example is an error, a flat object: `{"error":"missing_fields","target":"active","fields":[{"name":"area","allowed":["cli","tui","web"]}],"fix":"tm move CPL-42 active --set area=<value>"}`. `tm scheme show --json` exists today and prints the bare Scheme with no version (`main.rs`, `scheme_json`).

**How agents consume it.** Agents run commands through a shell tool that captures stdout and stderr, and parse stdout with `jq` or their own JSON parsing (**inferred**; not measured here). Two things matter more than the envelope's exact keys:

- one stable top-level key for the payload, so `jq .data` works for every command
- a version they can check without knowing the command

beads' consumer guidance asks readers to "Ignore unknown fields" and to warn but keep parsing on a higher version. That is the contract that makes additive changes safe.

### 2. Structured errors

**How others do it.**

- **rustc (via cargo), measured.** A diagnostic carries `level`, a stable `code` (`E0384`), a `message`, and `children` whose spans hold `suggested_replacement: "mut "` with `suggestion_applicability: "MachineApplicable"`. The fix is data with a confidence label, not prose. `cargo build` exited 101.
- **Kubernetes `Status`.** `status` is `Success` or `Failure`, `reason` is "a machine-readable, one-word, CamelCase description of why", and `message` is human-readable. `details.causes` holds `StatusCause` entries with `reason`, `message` and `field` for invalid input.
- **beads, measured.**
  - The documented shape is `{schema_version, error, code?, hint?}`; `code` and `hint` are optional.
  - `bd show pr-nope --json` wrote its JSON error to **stdout**, with no `code`.
  - The claim conflict (`bd update --claim` under another actor) wrote its JSON to **stderr**, also with no `code`, and the holder appeared only inside a message string.
  - `bd create --json` with no title printed plain text (`Error: title required`) and exited 1.
  - Every JSON error exited 1. beads' own docs say errors go "most" to stderr but "some command-result error paths emit the same shape to stdout".
  - In envelope mode, the error moves under `.data` (`{"data":{"error":…},"schema_version":1}`, Raw data). A beads parser that reads `.error` therefore cannot read tm's flat error unchanged, and the reverse holds too.
  - beads also has a typed `ClaimConflictError` in its storage layer (claims-and-links.md), but the CLI flattens it to text.
- **gh, measured.** Errors are plain text on stderr even under `--json` (`GraphQL: Could not resolve to an issue…`), with exit 1. `gh help exit-codes` defines 0, 1, 2 (cancelled) and 4 (authentication needed).
- **Linear CLI (source only, not run; behaviour inferred).** Errors are text (`✗ message` plus a hint line). Exit codes are classed as 1 general, 2 usage or invalid input, 3 not found, 4 authentication, 5 unavailable and 130 cancelled. Classes combine so that "not found" is reported only "when nothing worse happened" (`error.rs`).
- **clap, as tm uses it.** A usage error exits 2 and prints usage on stderr (`tests/it/scheme.rs`, `an_unknown_command_prints_usage_and_fails`). Under `--json` that is still text, because clap fails before tm knows `--json` was given.

**What tm has today, measured.**

- `FilterError` already serializes in the flat, `error`-tagged shape design.md shows. `stauts:todo` produced `{"error":"unknown_field","field":"stauts","span":[0,6],"valid":[…],"did_you_mean":["status"]}`, and `status:doing` produced `unknown_value` with `valid` and `did_you_mean`.
- No other `StoreError` variant is `Serialize`. Each carries its details as typed fields but reaches the user only as `Display` text.
- An Issue created with no fields failed with `Issue 01a14d4f-… is missing required fields: type, title, status`. That text has field names but no allowed values, no target and no fix, and it names the Issue by UUID.

The acceptance criterion ("a missing required field returns the missing_fields error with allowed values") is therefore not met by the store alone. The CLI must add `allowed` from the Scheme, since the store returns only names.

**Mapping `StoreError` to codes (recommendation).** The code is a stable snake_case id. The details are the variant's fields, with Issue UUIDs rendered as Keys where one exists. `fix` is a runnable command where tm can write one.

| `StoreError` | Code | Details | Fix |
|---|---|---|---|
| `MissingFields { issue, fields }` | `missing_fields` | `target` (the Status being entered), `fields[{name, allowed?}]`, with allowed values from the Scheme (types, Statuses, options, priorities) | `tm create … --type <type>` or `tm move KEY STATUS --set f=<value>` |
| `UnknownField { field, valid }` | `unknown_field` | `field`, `valid`, `did_you_mean` | none |
| `Invalid { field, reason, valid }` | `invalid_value` | `field`, `reason`, `allowed` | when `valid` is non-empty |
| `Op(InvalidField \| ReservedField \| InvalidLinkType \| InvalidProjectKey)` | `invalid_value` | `field` or `value` | none |
| `Op(TooLarge(n))` | `too_large` | `bytes`, `limit` (`MAX_PAYLOAD_BYTES`) | none |
| `Op(UnsupportedVersion(v))`, or `Replay { source: Op(UnsupportedVersion) }` when met while rebuilding | `upgrade_required` | `found` (the operation version), `supported` | upgrade tm |
| `NewerStore { found, supported }` | `upgrade_required` | `found` (the store format), `supported` | upgrade tm |
| `Cycle { relation, path }` | `cycle` | `relation`, `path` as Keys | `tm unlink …` for the edge that closes it, **inferred** useful |
| `AlreadyClaimed { … }` | `already_claimed` | `agent`, `actor`, `claim`, `expires` | none in Phase 1 (`--force` is Phase 3) |
| `NotClaimHolder { … }` | `not_claim_holder` | `claim`, `current` | `tm release KEY --claim <current>` only for the holder |
| `UnknownIssue` / `UnknownProject` | `not_found` | `kind`, `id` or Key | none |
| `DuplicateProject`, `KeyTaken`, `KeysExhausted` | `duplicate_project`, `key_taken`, `keys_exhausted` | fields | none |
| `Busy` | `busy` | none | "retry" |
| `ReadOnly`, `NotAStore`, `WorkspaceMismatch` | `read_only`, `not_a_store`, `workspace_mismatch` | path or ids | `tm init` where it applies |
| `Filter(FilterError)` | its own tag (`syntax`, `bare_word`, `unsupported`, `unknown_field`, `unknown_value`, `invalid_value`, `no_identity`) | as serialized, plus `span` | none |
| `Op(Payload \| KindMismatch \| InvalidVersion)` on a stored row, `Replay` with any other source, `Clock`, `Corrupt`, `Sqlite`, `DuplicateIssue`, `ProjectMismatch`, `SubjectNotProject` | `internal` (with `cause` set to the variant's name) | message | `tm doctor`, once it exists |

The filter and the store both use `unknown_field` and `invalid_value`; filter errors alone carry `span`. Lost Claims are not errors: they are `unapplied` rows (see 4 and follow-up 5).

### 3. Identity, `on_behalf_of` and `@me`

**Today.** `tm init` takes the writer from `TM_ACTOR`, else `git config --get user.email`, and refuses a blank `TM_ACTOR` (`init.rs`, `actor`). design.md says the human is set once with `tm whoami` and defaults to git `user.email`, and that "Agents are identified by `TM_ACTOR`, else by known environment variables … else as the human" (design.md:123).

beads takes the actor from `--actor`, then `BEADS_ACTOR`, then `BD_ACTOR`, then git `user.name`, then `$USER` (`cmd/bd/main.go:1010`). Separately, `getOwner()` records a human owner per Issue from `GIT_AUTHOR_EMAIL`, else git `user.email` (`cmd/bd/main.go:1060`). beads therefore keeps a writer and a human owner apart, but it does not detect Agents. The measured `bd create` output carried both `created_by` and `owner` (Raw data, redacted).

**Agent detection variables** (sub-agent survey; only Claude Code was run):

| Agent | Variable | Evidence | Status |
|---|---|---|---|
| Claude Code | `CLAUDE_CODE_CHILD_SESSION=1` | env-vars page | documented |
| Claude Code | `CLAUDE_CODE_SESSION_ID` | env-vars page | documented; seen in this shell |
| Claude Code | `CLAUDECODE=1` | env-vars page | documented; seen in this shell; also set by IDE extensions |
| Claude Code | `AI_AGENT=claude-code_<version>_agent`, `CLAUDE_CODE_ENTRYPOINT=cli` | seen in this shell | **undocumented** |
| Codex CLI | `CODEX_SESSION_ID`, `CODEX_VERSION`; `CODEX_THREAD_ID` "when a thread id is provided"; `CODEX_SANDBOX=seatbelt` (macOS sandbox only); `CODEX_CI=1` (unified exec only) | `exec_env.rs`, `shell_environment.rs` @ `8c076942…`; sandbox and CI from the sub-agent's read of `sandboxing/mod.rs` and `unified_exec/process_manager.rs` | source |
| Gemini CLI | `GEMINI_CLI=1` | `packages/core/src/services/shellExecutionService.ts` @ `ed96faed` | source and in-repo docs |
| OpenCode | `OPENCODE=1`, `AGENT=1`, `AI_AGENT=opencode` (only when unset), `OPENCODE_SESSION_ID` | `packages/core/src/tool/plugin/shell.ts` @ `63c1a27a` | source |
| Goose | `AGENT_SESSION_ID`; docs also promise `GOOSE_TERMINAL=1`, `AGENT=goose` | `developer/shell.rs` @ `bfcc6ee6` | source; the docs' claim is **unverified** |
| Cline | `CLINE_ACTIVE=true` | `VscodeTerminalRegistry.ts` @ `a42df8c8` | source |
| Cursor | `CURSOR_AGENT=1` | forum staff replies only | **unverified** |
| Copilot, Amp, Windsurf, Aider | none found authoritatively | libraries and web search | **unverified** |

The Claude Code page (WebFetch, near-verbatim) says:

- `CLAUDECODE`: "Set to `1` in subprocesses Claude Code spawns (Bash and PowerShell tools, tmux sessions, hook commands, status line commands, stdio MCP server subprocesses). IDE extensions also set this in their integrated terminals."
- `CLAUDE_CODE_CHILD_SESSION`: "Set to `1` in subprocesses Claude Code spawns via the Bash, PowerShell, and Monitor tools, hook commands, and status line commands. Not set for stdio MCP server subprocesses … Unlike `CLAUDECODE`, this is only set by Claude Code itself when it launches a subprocess and not by IDE extensions, so it reliably distinguishes a nested session from a top-level `claude` launched in an IDE-integrated terminal."
- `CLAUDE_CODE_SESSION_ID`: "Set automatically to the current session ID in Bash and PowerShell tool subprocesses, hook command subprocesses, and stdio MCP server subprocesses. … this matches the `session_id` field in the hook JSON input and is updated on `/clear`."

Codex's source says `CODEX_THREAD_ID` "is injected when a thread id is provided, even when `include_only` is set". It also says `inject_session_env` "Exposes the shared root-session identity and harness version to shell commands" as `CODEX_SESSION_ID` and `CODEX_VERSION` (`exec_env.rs:30` and `:40`).

`AI_AGENT` is a proposed cross-tool convention (the `@vercel/detect-agent` README), and both detection libraries check it first. Its value is not a clean name: Claude Code's carries a version, and OpenCode sets it only when unset, so a parent agent's value wins.

All of these variables are inherited by child processes. A shell opened inside an agent's terminal, or one agent launched from another, carries several sets at once, so the order of checks decides the answer. Any of them can be unset or spoofed, which is fine for choosing an identity label but not for security. design.md's open item "Codex and Copilot identity variables are unverified" is half settled: Codex is verified from source, Copilot is not.

**Where `on_behalf_of` lives.** The hashed envelope has one `actor` slot (`Op::envelope`), and ADR 0006 says `op_hash` covers "every field the Replica writes", so a new field must be hashed. Two version numbers are involved:

- `Op.v` (`CURRENT_VERSION = 1`) is the operation format. `Op::change()` accepts any `v` up to `CURRENT_VERSION` and refuses higher ones with "upgrade tm".
- `ENVELOPE_FORMAT = 1` is the first byte of the hashed layout, and `v` is also inside it.

The payload is per-kind `Change` JSON, so `on_behalf_of` in the payload would have to be repeated in every kind. It belongs beside `actor`.

**Sketch of v2 and its translation (recommendation).**

- `Op` gains `on_behalf_of: Option<String>`, and `CURRENT_VERSION` becomes 2. Nothing else joins the envelope.
- `envelope()` branches on `v`. For `v == 1` it writes today's bytes exactly, with `ENVELOPE_FORMAT = 1` and no new field. For `v >= 2` it writes format byte 2 and appends a presence byte plus a length-prefixed `on_behalf_of` after `actor`.
- Reading a stored `v: 1` row yields `on_behalf_of: None`. That branch is the first "old versions are translated when read" (ADR 0006). The payload needs no translation, because v2 changes no `Change` shape.
- A `v: 1` `Op` carrying `Some(on_behalf_of)` is refused by `Op::change()`, or made unrepresentable by the type, because its v1 hash would silently drop the field.
- `ops` gains a nullable `on_behalf_of TEXT` column, and `ops_immutable` must list it, or the column could be rewritten.
- **This is tm's first log-table migration.** `Store::open` accepts any `format` from 1 to `STORE_FORMAT` and has no upgrade step (`lib.rs:318` and `:417`). `STORE_FORMAT = 2` needs an upgrade path that adds the column, recreates the trigger and stamps the format inside one transaction, so a v1 store opens under a v2 binary. An older binary then refuses the store through the existing `NewerStore` guard.
- **Rule: every new `Change` kind bumps `CURRENT_VERSION`.** The v2 bump covers only the kinds that exist today. Enforce it with a test that ties the kind list to the version, for example a table of `(kind, introduced_in)` that `kind_names_match_the_serialized_tag` must cover exactly, with every `introduced_in` at or below `CURRENT_VERSION`. A new kind with no entry, or one added without a bump, then fails the build.
- Tests:
  - An explicit `v: 1` sample keeps `PINNED_OP_HASH`. Today `sample()` uses `CURRENT_VERSION`, so after the bump it would silently become a v2 sample. The v1 case must be its own sample.
  - A `v: 2` sample gets a second pinned hash.
  - `op_hash_covers_every_written_field` gains an `on_behalf_of` variant, and that variant fails if the v2 envelope drops the field.
  - A `v: 1` `Op` with `Some(on_behalf_of)` is refused.
  - A v1 store file written by today's code (a fixture or a generated database) opens, upgrades, rebuilds with `on_behalf_of` empty, and keeps every `op_hash`.
  - Removing the `v == 1` branch must make the pinned-hash test fail.

**Follow-up 6 falls out of the bump.** `link` and `claim` were added under `CURRENT_VERSION = 1`. A binary from before them therefore meets `"kind":"link"` and fails with serde's "unknown variant" rather than "upgrade tm". Once operations are written as `v: 2`, a binary that knows only v1 refuses them by number, and the kind-bumps-version rule keeps that true for later kinds. No released binary predates the change, so the gap affects development stores only. This is the moment to close it, because the switch milestone imports this project's real backlog in Phase 1.

**`@me` for an Agent.** `Context.me` is "the human the filter runs for" (`filter/mod.rs`), and `@me` without it fails with `no_identity`. For an Agent, `me` should be the human it acts for. Follow-up 3 therefore depends on the identity resolver existing, not on a filter change. `@agent` (design.md's function list) would name the Agent itself, but it is not in the first cut of the filter language.

### 4. Commands

| Command | Today's Store API | Gap | Phase 1 shape (recommendation) |
|---|---|---|---|
| `create` | `create_issue(actor, project, type, title)` writes create, type, title and status in one batch; status is `workflow(type).start` (`lib.rs:640`) | No description, priority, labels, parent or Scheme fields; Project comes from repo mapping (`workspace::discover`) | `tm create --type T --title "…" [--set f=v]… [--label l]… [--parent KEY]`; one `append_all` batch so required fields are checked once; output is the created Issue in the `show` shape |
| `list` | `find(filter, Context)` returns ids; `issue(id)` reads one | N+1 reads: at 10,000 Issues, `issue()` ×10,000 took 134 ms of a 157 ms total (Raw data) | `tm list [FILTER…]` joining arguments into one filter; needs a batch read, `Store::issues(&[Uuid])` or `find_views`; `--limit` (**inferred** useful; beads has `pagination`) |
| `show` | `issue(id) -> IssueView` with fields, sets, Key and category | No Links, Claim, comments or history; `parent` is a UUID, not a Key | `tm show KEY` adding `links` (outward and inward, named), `claim`, `comments`; history behind `--history` |
| `move` | `append` of `set_field status`; `Workflow::allows` is `Transitions::Any` | No per-status requirements in the model | `tm move KEY STATUS [--set f=v]…`, Status matched by id or label without case; the `--set`s go in the same batch, which is what design.md's `fix` assumes |
| `comment` | `Change::Comment { body }` and the `comments` table exist; no reader | No `Store::comments(issue)` | `tm comment KEY "body"`, or the body from stdin with `-`; output includes the comment's id (its `op_id`) |
| `ready` | none | Query, ordering and "would unblock" | See below |
| `claim` | `take`, `release`, `claim(issue)` | None for take and release | See below |
| `undo` | none | Grouping, prior state, a refusal rule | See below |

**`create` with no type.** `tm create` has no default type. When `--type` is missing, the store reports `type`, `title` (if missing) and `status`. `status` is not something the caller can supply here: it is derived from the type's Workflow start. The CLI should therefore drop fields derived from type (today only `status`) from `missing_fields` when `type` itself is missing. The error then lists `type` with its allowed ids, plus any other genuinely missing field. A test pins this: `tm create --title x --json` returns `missing_fields` naming `type` with `allowed` set to the seven type ids, and not naming `status`.

**`ready`.**

- **The queue.** design.md reads categories, never Status names. The queue is Issues in category todo, not archived, unclaimed, with no `blocks` source Link from an Issue outside done or cancelled. Triage is excluded ("Triage Issues are left out of default Views").
- **"What each Issue would unblock."** For each ready Issue, this is the open Issues it blocks whose only open blocker it is.
- **Measured.** The probe implements both as single, non-recursive queries. At 10,000 Issues the ready count took 8.8 ms and the unblock counts 8.9 ms.
- **Merged cycle.** Two ready Issues joined into a `blocks` 2-cycle (as a merge can leave, ADR 0009) both dropped out of the queue (5,999 to 5,997), and the query finished in 4.1 ms. It cannot loop, because it walks no paths.
- **Probe shortcut.** The probe hard-codes `status = 'todo'`. The real query must expand `category:todo` to the Scheme's (type, Status) pairs, as the filter's `Condition::Category` already does (ADR 0007). The timings show the query's shape, not the production SQL.

Two readiness rules need a decision first:

- Whether a child of a blocked parent is blocked. beads does this: "a blocked parent blocks its children" (`json-schema.md`, `gated_by`). Doing so walks ancestry, which must stop on a merged parent cycle, the way `ancestry_reaches` already does.
- Whether an Issue with open children is ready. design.md's "waits for all children" is a per-Link-type readiness behaviour, which is Phase 4.

Minimal Phase 1 is direct blockers only, with no snooze (Phase 4) and no Gates.

**`claim`.**

- **What exists.** `Store::take` already enforces the strict rule and returns `Claim { id, … }`, and the second take in the probe was refused naming the holder.
- **Phase 1 command.** `tm claim KEY [--agent NAME] [--for DURATION]`, printing the Claim with its `id`.
- **The token.** Under ADR 0009 the id is the token: a release names it, and without it release is impossible. `tm release KEY --claim ID` (or `tm claim KEY --release ID`) wraps `Store::release`.
- **Phase 3.** design.md's `tm start` and `ready --claim` are Phase 3 (build-plan.md).
- **No Agent identity.** `tm claim` with no Agent identity runs into ADR 0009's open question of whether a human may hold a Claim. Phase 1 needs an answer, even if it is "refused with `no_agent` and a fix of `TM_AGENT=<name> tm claim …`".

**`undo`.** design.md says it "reverts the caller's last operation by appending its inverse". How others do it:

- **jj** appends a new operation that restores the repo to the undone operation's parent. Repeated `jj undo` walks back through an undo stack rather than toggling, and `jj redo` exists. The one refusal is undoing another workspace's operation, which needs `--allow-cross-workspace` (`cli/src/commands/undo.rs`, `docs/operation-log.md`). jj does not refuse because of an intervening write to the same data.
- **taskwarrior** reverses the operations since the last `UndoPoint`. It can do so because each `Update` stores `old_value`. Its refusal, "Could not undo: other operations have occurred" (`CmdUndo.cpp`), fires when the operations to undo are no longer the most recent unsynced local operations. taskchampion supports reversal only for "local operations that have not yet been synchronized", pops them from storage rather than appending (`taskdb/undo.rs`), and has a TODO to "Support concurrent undo by adding the reverse". Its `UndoPoint` marks a command's boundary locally, outside anything that syncs. That is the prior art for keeping tm's batches local.
- **beads** has no undo command (`bd --help` lists none). `bd reclaim` reverts stale leases only.

What this means for tm:

- **Grouping.** One command writes several operations; `create_issue` writes four. "Last operation" must mean the last command's batch, but the log records no batch identity and time cannot stand in for it. Operations in one `append_all` share a `utc_ms` by design. Separate transactions collide too: the probe's 1,000-Issue seed ran 1,534 write transactions (4,534 operations) that landed on 466 distinct `utc_ms` values, and an earlier run of the same seed gave 309. The count varies by run and machine, but transactions outnumber timestamps either way.
- **Inverses must restore prior state, not flip the operation.**
  - Flipping is wrong for sets. Measured: link A to B, link A to B again (a duplicate add, logged), then apply the flipped inverse of the last operation (a remove). The Link the first add made is gone (199 → 200 → 200 → 199 Links). Removing an absent element has the mirror problem.
  - Undo must therefore restore each touched element's membership as it was before the batch. That applies to labels, multi-select fields and Links alike.
  - `set_field` restores the value the field held before the batch, from the operation that held it (readable through the `ops_target` index, skipping `unapplied` rows).
  - A `claim` take is reversed by a release naming its id. A release is reversed by a new take, which makes a new Claim id.
  - `create` has no inverse kind. The nearest is `set_field archived = true`, which matches "everything is soft-deleted".
  - `comment` has no inverse at all. Comments are append-only, and no retract or edit kind exists.
- **Refusal (tm's own rule).** Undo should refuse with `undo_conflict` when another batch has written the same field or element since. It should name the current value and leave the decision to the caller. Neither jj nor taskwarrior uses exactly this rule.
- **What undo targets.** If undo targets "the last batch of this Actor name", two sessions named `claude` undo each other's work. A session id does not separate them either:
  - Measured: a Claude Code sub-agent's Bash tool sees the parent session's `CLAUDE_CODE_SESSION_ID`.
  - Codex's `CODEX_SESSION_ID` is "the shared root-session identity" (`exec_env.rs:40`), so its sub-agents share it too.
  - Parallel sub-agents are the likeliest way one Agent name gets concurrent writers, so a session key fails where it matters most. It also leaves humans and undetected Agents with no key.
  - A token does separate them, as a Claim id does for release: every write reports its batch id, and `tm undo <batch>` reverts exactly that batch.

**The eight follow-ups.**

1. **Unapplied warning.** No CLI code reads `Store::unapplied()` today (`grep -rn unapplied crates/tm-cli` found nothing). Rebuilds happen in `Store::open` when `PROJECTION_VERSION` changes, so any command can trigger one. Recommendation: after opening, report unapplied rows that are not lost Claims. In text mode that is one stderr line; under `--json` it is `warnings[]` in the document, with nothing on stderr.
2. **`tm scheme show <type> --json` with `required_to_enter`.** The Scheme model has nowhere to put it. `Status` is `{id, label, category}`, `Field.required` is global, and `Transitions` is only `Any`. The global rule is also narrower than "required": `append_all` records each Issue's missing fields before the batch and refuses only fields the batch newly leaves missing (`lib.rs`, the `before` map). An Issue replayed without a title can still move to `in_progress` (read from code, not run). A Phase 1 output can carry a `required_to_enter` list on every Status, filled with the globally required fields, to fix the shape before Phase 4. That field is **proposed**, not current behaviour: no write is refused per Status today.
3. **`@me` for an Agent.** See section 3. This depends on the identity resolver.
4. **`on_behalf_of` and format v2.** See section 3, including the first log migration.
5. **`tm doctor` lists lost Claims.** `Unapplied::is_lost_claim()` exists. A minimal `tm doctor` that prints `lost_claims[]` and `problems[]` from `unapplied()` meets the note. Full `doctor` is Phase 4.
6. **Link kind version.** This is covered by the v2 bump and the kind-bumps-version rule (section 3).
7. **Inward names.** No test writes a Link from B's end with an inward name. `links.rs` covers symmetric order, the type in the key, two Replicas and cycles, but not "B is blocked by A". The store API takes only a type id and `(from, to)`, so inward names are a CLI parsing step: a `tm_core::link` lookup from a name to `(type, reversed)`. The test belongs where the name is parsed: `tm link B "is blocked by" A` and `tm link A blocks B` must leave one `field_versions` row and one `links` row (ADR 0009 Confirmation).
8. **Reading Links.** No reader exists; `links_by_dst` indexes the inward side.
   - Unlinking an unheld Link returns `Ok` and appends an operation. The probe unlinked a pair it had checked held no Link: the call succeeded, the Link count stayed 199, and the log gained a `present = 0` row. This is deliberate and tested (`adding_a_held_link_or_removing_a_missing_one_is_logged`).
   - The recommended `Store::links(issue) -> Vec<LinkView { link_type, direction, other, name }>` serves `show`. It also lets the CLI answer `tm unlink` on an unheld pair with `not_linked` and what the pair does hold.
   - Whether to also stop appending the no-op remove is a store decision. Keeping it matches set semantics under sync; refusing it is a write-time check like the Workflow rules.

### 5. Arguments, output and start-up

- **Input.** Accept a Key, matched without case, through `issue_by_key`, which resolves retired prefixes. Also accept a full UUID. A UUID prefix is a later convenience; design.md says "Ambiguous Key references are refused, never guessed". Output carries both `key` and `id`. A provisional or missing Key prints `null`, as `IssueView.key` already allows.
- **`--json` everywhere.**
  - Today `--json` is a per-subcommand flag on `scheme show` only. clap can declare the flag once as a global argument (`global = true`, per clap's docs; not exercised here).
  - Reading `TM_JSON=1` through clap needs clap's `env` feature, which the workspace does not enable (`Cargo.toml`: `features = ["derive"]`). A manual `std::env::var` check works without it.
  - clap fails before the flag is parsed, so the CLI must check the raw arguments and `TM_JSON` for JSON mode before calling `Cli::try_parse()`. The scan stops at `--`, so a literal `--json` after it (in a comment body, say) is not read as the flag.
  - clap's `Error` (its `ErrorKind`, say `MissingRequiredArgument`) is rendered as a `usage` error document.
  - Scheme-required inputs (`--type`, `--title`) should not be clap-required. Otherwise clap reports them as usage text and `missing_fields` with allowed values can never be returned.
- **Text output.** Text is for humans, uses Keys rather than UUIDs, and adds no colour without a terminal (**inferred** convention; the compact array gh printed into a pipe is the measured analogue). design.md: "In a terminal, humans are prompted for the missing values instead". Prompts should happen only when stdin and stdout are both terminals and `--json` is off. `init.rs`'s `confirm()` checks only that stdin is a terminal (`io::stdin().is_terminal()`), so that is the pattern to tighten, not copy.
- **Exit codes.** 0 for success, 1 for a refusal or failure with a document, 2 for usage (clap's default, already tested). `busy` has no exit code of its own: under `--json` the `error` code says it, and finer exit codes add little once errors are documents. Linear CLI's finer classes exist because its errors are text.
- **Stream.** Under `--json`:
  - Exactly one JSON document per invocation goes to stdout, success or failure.
  - Nothing goes to stderr, so `2>&1` still yields one parseable document. Warnings ride inside the document, on failure too.
  - A panic hook writes an `internal` error document before the process ends. With `panic = "abort"` the hook still runs before the abort (per Rust's docs; not exercised here).
  - Flattened error details never use the reserved keys `error`, `message`, `fix`, `schema_version`, `data` or `warnings`.

  This is the opposite of gh and of most of beads' paths, but design.md's error example only works if the agent receives it. The cost is that `tm … --json | jq .data` prints `null` on failure, and the exit code carries the failure.
- **Writes.** `main.rs` already treats a reader that goes away (`BrokenPipe`) as success, and every command should go through that path.
- **Read commands take the write lock today.** Every `Store::open` runs `init()` inside `BEGIN IMMEDIATE` (`lib.rs:407`), so `tm show` or `tm list` waits on, or blocks, any writer (read from code; the contention itself was not measured). `init()` also runs `SELECT coalesce(max(hlc), 0) FROM ops WHERE replica_id = …` on every open. `ops` has no index on `replica_id` (only `ops_target` and `ops_server_position`), and `EXPLAIN QUERY PLAN` shows `SEARCH ops` with no index.
  - Measured: that statement alone took 0.34 ms at 4,534 operations and 5.43 ms at 45,334. Whole opens were 1.5 ms and 6.9 ms, so the scan accounts for nearly all of the growth. `PRAGMA user_version` took about 2 µs.
  - Recommendation: give read commands an open path that skips the write transaction and the scan. A writer can keep `last_hlc` current as it appends, as `append_all` already does, or an index on `ops (replica_id, hlc)` can make the repair a seek. This belongs in the envelope and identity PR, which is the first to open stores from many commands, and tm-u8g.13 should measure it in its cold-start budget.
- **Cold start.** A release `tm --version`, `tm scheme show` and `tm scheme show --json` each took 3.8 to 5.8 ms per run, including Python's process-spawn overhead, and were noisy between runs. The build plan leaves the budget "to be set in this phase" (tm-u8g.13). `show` and `ready` fit a budget in the low tens of milliseconds at 10,000 Issues. `list` at 10,000 Issues (157 ms) does not, until it has a batch read.

### 6. Conflicts and open decisions

An index; the detail is in Open questions or Implications.

- **`TM_ACTOR`** names whoever writes in `init.rs`, but design.md:123 has it identify Agents. The user's decision (Implications) keeps `TM_ACTOR` for the writer, which means design.md:123 needs updating.
- **design.md's error example moves to `active`,** a Status category, while `move` takes a Status id.
- **Per-Status requirements** ("required to enter X") have no place in the Scheme model; `Field.required` is global and applies only to newly introduced omissions.
- **Comments.** "Only the author's edits apply" (ADR 0006), but no edit or retract kind exists, so neither editing nor undoing a comment is possible.
- **Human Claims.** ADR 0009 leaves open whether a human with no Agent identity may hold a Claim, and Phase 1's `claim` meets it first.
- **Phase boundaries.** `tm start` and `ready --claim` are Phase 3 and `claim` is Phase 1. Actor detection with "on behalf of" history is listed under Phase 3, while the bead's notes put the format change here.
- **`tm undo`** "reverts the caller's last operation", while one command writes several operations, and design.md does not say what the caller is.

## Conclusions

- **Start with an envelope, not bare values.** Every tool that versions its output either puts the version on each object (kubectl) or wraps the payload (beads' v2). The tools that use bare arrays (gh, Linear CLI, legacy beads) cannot version lists, and beads is paying for that now with a migration.
- **The error shape design.md shows is already half built.** `FilterError` serializes in exactly that flat, `error`-tagged form. The store's errors carry typed details but only `Display` output, and `missing_fields` lacks the allowed values the acceptance criterion requires. The CLI, not the store, should add Scheme-derived `allowed` values and the `fix`, and drop type-derived fields when the type itself is missing.
- **Format v2 should land before any command writes real data.** It adds only `on_behalf_of`. It closes the unversioned `link` and `claim` kinds, and the kind-bumps-version rule keeps later kinds closed. It also brings tm's first log migration. The switch milestone, still within Phase 1, makes later changes permanent translations.
- **Most commands are thin over the existing Store, but there are gaps.** The missing APIs:
  - a batch read for `list`, which is 85% of its cost at 10,000 Issues
  - readers for Links, comments and history for `show`
  - a ready query that expands categories through the Scheme
  - a read-only open path, since every open now takes the write lock and scans the log
  - the whole of `undo`
- **`ready` can be one non-recursive query.** Measured, it stays finite and fast on a merged cycle, which meets ADR 0009's "anything that walks Links or parents must stop on a cycle" by walking nothing.
- **`undo` is a local feature in Phase 1.** It needs a batch record kept outside the log, inverses that restore prior state rather than flip operations, a refusal rule for intervening writes, and a batch token the caller names, since a session id is shared by sub-agents.

## Implications / actions

Recommendations for tm-u8g.8. Where the user has decided, that is marked.

**Envelope.**

- Success: `{"schema_version": 1, "data": <payload>, "warnings": [...]}`.
- Failure: `{"schema_version": 1, "error": "<code>", "message": "...", "fix": "...", "warnings": [...], <details>}`, flat as design.md shows and as `FilterError` serializes.
- A document is a failure exactly when it has `error`.
- `schema_version` reuses beads' name. beads nests its envelope's errors under `.data`, so beads parsers do not read tm's errors unchanged.

**Versioning.**

- `schema_version` is one integer for the whole CLI.
- Additive fields do not bump it; removing, renaming or retyping does.
- Insta snapshots of each command's `--json` (build-plan Phase 1) pin the shapes.
- A compatibility promise starts at tm v1, so the number can move freely until then. Pre-v1 there is no `TM_JSON_VERSION` escape hatch.

**Errors and streams (decided).**

- Map `StoreError` to codes per the table in Findings section 2.
- Exit codes are 0, 1 and 2.
- Under `--json`, exactly one document goes to stdout and nothing to stderr, failures included. clap errors become `usage` documents, a panic hook writes an `internal` document, and JSON-mode scanning stops at `--`.
- Details never take the reserved keys, and the failure document carries `warnings`.

**Identity (decided).**

- `TM_ACTOR` only names the writer, defaulting as `init.rs` does to git `user.email`. The human is that writer when no Agent is detected, and later `tm whoami`.
- Whether an Agent is writing comes only from environment detection, in this order:
  1. `CLAUDE_CODE_CHILD_SESSION` or `CLAUDE_CODE_SESSION_ID` (Claude Code)
  2. `CODEX_SESSION_ID` or `CODEX_THREAD_ID`
  3. `GEMINI_CLI`
  4. `OPENCODE`
  5. `CURSOR_AGENT` (unverified)
- `CLAUDECODE` is not read. Claude Code's tools see `CLAUDE_CODE_SESSION_ID` too, so `CLAUDECODE` alone mostly means a human in an IDE terminal. The cost: an Agent writing from a tmux session Claude Code started, where the docs list `CLAUDECODE` but not the session id, records as the human.
- `TM_AGENT=<name>` is an explicit opt-in for Agents tm cannot detect, those cases included.
- `AI_AGENT` is not read until its values settle.
- `on_behalf_of` is set only when an Agent is detected and its name differs from the human's.
- `@me` resolves to the human in both cases.
- design.md:123 ("Agents are identified by `TM_ACTOR`") needs updating to match.

**Format v2 (decided).** The envelope gains only `on_behalf_of`. Every new `Change` kind bumps `CURRENT_VERSION`, with a test tying the kind list to the version. The test plan is in Findings section 3, including an explicit v1 sample that keeps `PINNED_OP_HASH` and a refused v1 `Op` carrying `on_behalf_of`.

**Undo batches (decided).**

- Batches are not in the hashed envelope. ADR 0006 says `op_hash` covers every field the Replica writes, so an unhashed, Replica-written `ops` column has no category.
- Batches live in a local side table, `op_batches(op_id, batch)`, outside both the log and the projections. A rebuild neither reads nor drops it.
- Every write's output carries its batch id. Phase 1 `tm undo <batch>` reverts exactly that batch, restoring prior state per field and per set element, and refuses with `undo_conflict` on an intervening write. There is no bare "undo my last command"; the caller keeps the id, as it does a Claim's. Undo after sync is designed together with sync.
- design.md:28 ("reverts the caller's last operation") needs updating to match.
- taskchampion's local-only `UndoPoint` is the prior art.

**`create` (decided).** No default type. When `type` is missing, `missing_fields` names `type` with its allowed ids and drops type-derived `status`, with a test.

**Read path.** A read-only open that takes no write lock and does not scan `ops` for `max(hlc)`, landing in PR 2 and measured in tm-u8g.13.

**Proposed pull requests.** One branch each, each merged before the next starts:

1. **`feat(core): operation format v2 with on_behalf_of`**: `on_behalf_of` only, the envelope branch, `CURRENT_VERSION = 2`, the kind-bumps-version rule and its test, `STORE_FORMAT = 2` with the first migration, the pinned-hash and v1-fixture tests, and the `op_batches` side table (local, outside the log). It covers follow-ups 4 and 6. It comes first because every later PR writes operations, and adding fields later would mean a second migration.
2. **`feat(cli): JSON envelope, error codes and identity`**: global `--json`/`TM_JSON`, the error mapping, clap and panic errors as JSON, the identity resolver, Key-or-UUID resolution, the read-only open path, the unapplied warning (follow-up 1) and the insta dev-dependency. It ships with `tm create` and `tm show` (fields, Claim) as the first two consumers, so the plumbing is reviewed against real commands. `missing_fields` with `allowed` meets the acceptance criterion here. It depends on PR 1 for `on_behalf_of`.
3. **`feat(cli): list and ready`**: `Store::issues` batch read, `@me` from the resolver (follow-up 3), the ready query through Scheme categories, and "would unblock". It depends on PR 2's plumbing and resolver.
4. **`feat(cli): move, comment and Links`**: `move` with `--set`, `comment` and `Store::comments`, `Store::links` with inward names, `tm link`/`tm unlink` (with `not_linked`), and `show` gaining Links and comments (follow-ups 7 and 8). It also adds `tm scheme show <type>` with the proposed `required_to_enter` (follow-up 2), since `move`'s `missing_fields` and `fix` read the same requirements. `tm link` and `tm unlink` are not among the bead's eight commands; follow-ups 7 and 8 motivate them, and whether they belong here or in their own bead is the user's call.
5. **`feat(cli): claim, release and doctor's lost Claims`**: `tm claim` and `tm release` with the token in the output, plus a minimal `tm doctor` (follow-up 5). It depends on the resolver for the Agent name.
6. **`feat: undo`**: `tm undo <batch>` from `op_batches`, prior-state inverses per kind, `undo_conflict`, and the decision on `create` and `comment`. It has no format dependency; it comes last because it needs the most design.

**ADR work, for the user to decide.**

- An amendment to ADR 0006 or a new ADR recording the v2 envelope, the translation rule as implemented, the kind-bumps-version rule and the log migration policy.
- A short ADR on the CLI's JSON contract (envelope, versioning, stream, exit codes), since agents will depend on it and it should not drift quietly.

**Splitting the bead** into child beads per PR is the user's call (beads writes are not run here).

## Open questions

- **Undo of `create` and `comment`.** Archive the Issue, and add a comment-retract kind or refuse? design.md says comments are append-only and "only the author's edits apply", but no edit kind exists.
- **May a human with no Agent identity run `tm claim`?** This is open in ADR 0009, and Phase 1 needs an answer.
- **design.md's example moves to `active`,** which is a Status category, while the default Workflow's Statuses are `in_progress` and `in_review`. Should `move` accept a category and pick its first Status, or is the example's `target` a Status id? The example's `area` field and `--set` also assume per-Status requirements the Scheme cannot express yet.
- **Readiness of children.** Does a blocked parent block its children (beads), and is an Issue with open children ready? Phase 1 or Phase 4?
- **Should an unheld `unlink` still append an operation?** The current test pins that it does.
- **Linux.** Nothing here ran on Linux. Agent variables were read from source and docs, and only Claude Code's were observed.

## Raw data

`run.sh <repo> $SCRATCH` (both built with tm's release profile, macOS):

```text
4.28 ms per run: --version
5.78 ms per run: scheme show
3.78 ms per run: scheme show --json
seeded 1000 Issues: 1534 write transactions, 4534 operations, 466 distinct utc_ms
list: 1000 Issues, 300784 bytes; open 1.52 ms, find 0.32 ms, issue() x1000 13.94 ms, encode 1.42 ms, total 17.20 ms
list: 1000 Issues, 300784 bytes; open 1.42 ms, find 0.32 ms, issue() x1000 13.70 ms, encode 1.37 ms, total 16.80 ms
list: 1000 Issues, 300784 bytes; open 1.50 ms, find 0.36 ms, issue() x1000 13.79 ms, encode 1.37 ms, total 17.03 ms
open: 4534 operations; max(hlc) for this Replica 0.342 ms (plan: SEARCH ops); PRAGMA user_version 0.002 ms
ready (as seeded): 599 ready, 67 would unblock something; ready 0.99 ms, with unblock counts 0.90 ms
ready (with a merged 2-cycle): 597 ready, 67 would unblock something; ready 0.40 ms, with unblock counts 0.87 ms
seeded 10000 Issues: 15334 write transactions, 45334 operations, 4391 distinct utc_ms
list: 10000 Issues, 3027785 bytes; open 6.92 ms, find 3.11 ms, issue() x10000 134.54 ms, encode 13.82 ms, total 158.40 ms
list: 10000 Issues, 3027785 bytes; open 6.58 ms, find 2.75 ms, issue() x10000 133.64 ms, encode 13.69 ms, total 156.67 ms
list: 10000 Issues, 3027785 bytes; open 6.83 ms, find 2.82 ms, issue() x10000 133.61 ms, encode 14.23 ms, total 157.49 ms
open: 45334 operations; max(hlc) for this Replica 5.433 ms (plan: SEARCH ops); PRAGMA user_version 0.002 ms
ready (as seeded): 5999 ready, 667 would unblock something; ready 8.83 ms, with unblock counts 8.87 ms
ready (with a merged 2-cycle): 5997 ready, 667 would unblock something; ready 4.06 ms, with unblock counts 8.86 ms
stauts:todo: {"error":"unknown_field","field":"stauts","span":[0,6],"valid":["type","title","description","status","category","priority","assignee","reporter","parent","key","agent","claim","labels","archived","created","updated"],"did_you_mean":["status"]}
status:doing: {"error":"unknown_value","field":"status","value":"doing","span":[7,12],"valid":["triage","todo","in_progress","in_review","done","cancelled"],"did_you_mean":["done"]}
title:x: {"error":"unsupported","construct":"text search","span":[6,7]}
create without fields: Issue 01a14d4f-ef1a-76ee-828e-660d72cbd4d1 is missing required fields: type, title, status
second take: Issue 01a14d4f-d9bd-728b-b0c7-c0d894b1d1a3 is held by claude (Claim 01a14d4f-ef1c-71f6-bd63-c881bba848a2, taken by probe)
unlink of an unheld Link: ok=true, links 199 -> 199
link, duplicate link, flipped inverse: links 199 -> 200 -> 200 -> 199
```

Expected ready count at 1,000 Issues: 666 todo (every 3rd Issue is done), minus the 67 whose blocker (the Issue before it) is still open, gives 599, which matches. An earlier run of the same 1,000-Issue seed, with the probe at `opt-level = 3`, gave 309 distinct `utc_ms`. The timestamp count varies by run and build, while the transaction count does not.

`EXPLAIN QUERY PLAN` for the open-time scan (`sqlite3 $SCRATCH/store-10000/tm.db`):

```text
QUERY PLAN
|--SEARCH ops
`--SCALAR SUBQUERY 1
   `--SEARCH meta USING INDEX sqlite_autoindex_meta_1 (k=?)
```

beads 1.3.1 in a scratch git repo (owner and author values redacted):

```text
$ bd create "First" -t task --json
{
  "created_at": "2026-10-18T04:09:59.647225Z",
  "created_by": "<redacted>",
  "id": "pr-xi0",
  "issue_type": "task",
  "owner": "<redacted>",
  "priority": 2,
  "schema_version": 1,
  "status": "open",
  "title": "First",
  "updated_at": "2026-10-18T04:09:59.647225Z"
}
$ bd list --json            # bare array, no schema_version
[
  {
    "id": "pr-xi0",
    "title": "First",
    ...
$ bd show pr-nope --json 2>/dev/null      # JSON error on stdout; text on stderr
{
  "error": "no issues found matching the provided IDs",
  "hint": "some IDs may reference deleted/purged records with no trace left in the live database — try 'bd history <id>' to check",
  "schema_version": 1
}
exit=1
$ BD_JSON_ENVELOPE=1 bd show pr-nope --json
{
  "data": {
    "error": "no issues found matching the provided IDs",
    "hint": "..."
  },
  "schema_version": 1
}
$ BD_ACTOR=other bd update pr-xi0 --claim --json 1>/dev/null     # JSON on stderr, no code
Error updating pr-xi0: issue already claimed by <redacted>
{"error":"1 of 1 issues failed to update","failed":[{"id":"pr-xi0","error":"updating issue: issue already claimed by <redacted>"}],"schema_version":1}
exit=1
$ bd create --json          # missing title: plain text, exit 1, stdout empty
Error: title required (or use --file to create from markdown)
```

gh 2.102.0:

```text
$ gh issue list --json
Specify one or more comma-separated fields for `--json`:
  assignees
  author
  blockedBy
  ...
exit=1
$ gh issue list --json bogus
Unknown JSON field: "bogus"
Available fields:
  ...
exit=1
$ gh issue list -R cli/cli --limit 2 --json number,title,state     # piped
[{"number":14733,"state":"OPEN","title":"gh skill search: opt-in discovery of supported hidden skill directories"},{"number":14732,...}]
$ gh issue view 999999999 -R cli/cli --json title
GraphQL: Could not resolve to an issue or pull request with the number of 999999999. (repository.issue)
exit=1
```

kubectl 1.36.1 (client):

```text
$ kubectl create configmap demo --from-literal=a=b --dry-run=client -o json
{
    "kind": "ConfigMap",
    "apiVersion": "v1",
    "metadata": { "name": "demo" },
    "data": { "a": "b" }
}
$ kubectl create configmap demo --dry-run=client -o json --from-literal
error: flag needs an argument: --from-literal
exit=1
```

cargo 1.99.1, `cargo build --message-format json` on `let x = 5; x = 6;` (fields trimmed by a filter):

```text
{"reason": "compiler-message", "message": {"$message_type": "diagnostic", "level": "error", "code": "E0384",
  "message": "cannot assign twice to immutable variable `x`",
  "children": [{"level": "help", "message": "consider making this binding mutable",
    "spans": [{"suggested_replacement": "mut ", "suggestion_applicability": "MachineApplicable"}]}]}}
{"reason": "build-finished", "success": false}
exit=101
$ cargo metadata --format-version 1 --no-deps   # .version
1
```

Codex @ `8c076942cb70fba8e5c4732c23f1faabc4aed484`:

```text
codex-rs/core/src/exec_env.rs:30: /// `CODEX_THREAD_ID` is injected when a thread id is provided, even when
codex-rs/core/src/exec_env.rs:31: /// `include_only` is set.
codex-rs/core/src/exec_env.rs:40: /// Exposes the shared root-session identity and harness version to shell commands.
codex-rs/core/src/exec_env.rs:42:     env.insert(CODEX_SESSION_ID_ENV_VAR.to_string(), session_id.to_string());
codex-rs/protocol/src/shell_environment.rs:6: pub const CODEX_SESSION_ID_ENV_VAR: &str = "CODEX_SESSION_ID";
codex-rs/protocol/src/shell_environment.rs:166:    if let Some(thread_id) = thread_id {
```

Repo greps at `49b8560`:

```text
$ grep -rn unapplied crates/tm-cli
(no output, exit 1)
$ grep -n 'fn ' crates/tm-store/tests/it/links.rs   # no inward-name test
a_symmetric_link_written_either_way_round_is_one_link
the_type_is_part_of_a_links_key
the_same_link_from_two_replicas_is_one_element_and_a_later_remove_wins
adding_a_held_link_or_removing_a_missing_one_is_logged
...
```
