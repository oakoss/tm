# The CLI speaks one JSON envelope and flat, coded errors

- Status: accepted
- Date: 2026-10-18
- Deciders: Jace Babin

## Context and Problem Statement

design.md promises that every command supports `--json` (or `TM_JSON=1`) "with a stable, versioned output schema", and that errors are structured and carry the fix. Agents are the main readers: they run `tm` through a shell tool and parse stdout. What does a document look like, how is it versioned, where do errors and warnings go, and how does a caller tell success from failure? The research is in [core-commands.md](../research/cli/core-commands.md), sections 1, 2 and 5.

## Decision Drivers

- One key for the payload, so `jq .data` works on every command, and a version a caller can check without knowing the command.
- An agent must receive the error and its fix, so errors cannot go to a stream it may not read.
- `2>&1` must still yield one parseable document.
- Arrays must be versionable too: beads is migrating away from bare arrays with the version mixed into objects.

## Considered Options

- **Shape**: bare values, as gh and jj print; the version inside each object, as kubectl and legacy beads do; or an envelope around the payload.
- **Errors**: text on stderr, as gh does; JSON on stderr; or one JSON document on stdout.

## Decision Outcome

**Documents.**

- Success: `{"schema_version": 1, "data": <payload>, "warnings": [...]}`.
- Failure: `{"schema_version": 1, "error": "<code>", "message": "...", "fix": "...", "warnings": [...], <details>}`, flat, as design.md's example and `FilterError` already are. `fix` is a runnable command, present only where tm can write one.
- A document is a failure exactly when it has `error`. Details never use the reserved keys `schema_version`, `data`, `warnings`, `error`, `message` or `fix`.
- A write's `data` carries `batch`, the id `tm undo` takes ([ADR 0010](0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md)).

**Streams and exit codes.**

- `--json` anywhere before `--`, or `TM_JSON=1`, selects JSON mode before the arguments are parsed, so a usage error is a document too.
- In JSON mode exactly one document goes to stdout and nothing to stderr, failures included. A usage error is a `usage` document and a panic an `internal` one.
- Exit codes: 0 success, 1 a refusal or failure, 2 a usage error. The `error` code says which refusal. A panic writes its document and exits 1 rather than aborting.
- In text mode, output names Issues by Key, errors and warnings go to stderr, and a prompt appears only when stdin and stdout are both terminals.

**Versioning.**

- `schema_version` is one integer for the whole CLI. Adding a field does not bump it; removing, renaming or retyping one does, and readers ignore fields they do not know.
- Snapshot tests of each command's `--json` pin the shapes.
- Until tm v1 the number moves freely and no older shape is kept, as with the operation format ([ADR 0010](0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md)).

**Error codes.** Each store, Workspace and filter error maps to a stable snake_case code, with its typed fields as details:

| Error | Code |
|---|---|
| Required fields left unset | `missing_fields`, naming each field with its allowed values |
| Unknown field, or a value, name, key or type that does not fit | `unknown_field`, `invalid_value` |
| An operation over the size limit | `too_large` |
| A newer operation format or store | `upgrade_required` |
| A log another build wrote before the format changed in place ([ADR 0010](0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md)) | `store_format_changed` |
| A Link or parent that closes a cycle | `cycle` |
| A Claim held, the wrong Claim token, a human taking a Claim | `already_claimed`, `not_claim_holder`, `no_agent` |
| An Issue or Project that does not exist | `not_found` |
| A Project id already created, a Project key taken or used up | `duplicate_project`, `key_taken`, `keys_exhausted` |
| Another writer holds the store | `busy` |
| A store that cannot be used here | `read_only`, `not_a_store`, `workspace_mismatch` |
| An export that is not one, holds another operation format, or meets a Workspace holding other operations ([ADR 0013](0013-export-carries-the-log-and-issue-records.md)) | `invalid_export` naming the line, `export_format_changed`, `workspace_not_empty` |
| No repo file maps this repo to a Workspace | `no_workspace`, with fix `tm init` |
| A repo file names a Workspace this machine lacks, or one that holds no store | `unknown_workspace`, `incomplete_workspace`, with the remedy in `message` |
| `tm init` on a repo that already has a Workspace | `workspace_exists` |
| No home directory, or a relative `TM_HOME` | `no_home` |
| No human to write as, or a blank or non-UTF-8 `TM_ACTOR` or `TM_AGENT` | `no_identity` |
| Two repo files, or one that does not parse | `invalid_repo_file` |
| A path, or an export stream, that cannot be read or written | `io`, naming the path when there is one |
| A filter that does not parse or fit | the filter's own codes |
| Anything else: a malformed stored row, a clock error, SQLite | `internal`, with `cause` |

A Workspace error that wraps a store error takes the store error's code.

The mapping is an exhaustive match, so a new error cannot fall to `internal` unnoticed.

### Consequences

- Good, because every command, success or failure, is read the same way, and lists are versioned like objects.
- Good, because an agent sees the fix even when it reads only stdout.
- Bad, because `tm … --json | jq .data` prints `null` on failure; the exit code carries it. This is the opposite of gh and most of beads.
- Bad, because tm's errors sit at the top level, while beads' envelope nests them under `.data`, so beads parsers do not read them unchanged.

### Confirmation

A missing required field returns `missing_fields` with allowed values. A usage error under `--json` is a `usage` document on stdout at exit 2, with nothing on stderr. `--json` after `--` is not read as the flag. Each command's `--json` has a snapshot test.

## More Information

- Research: [core-commands.md](../research/cli/core-commands.md).
- Error codes for `move`, Links and undo arrive with those commands.
