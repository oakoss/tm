# Export carries the log and Issue records; import restores the log or writes the records

- Status: accepted
- Date: 2026-10-19
- Deciders: Jace Babin

## Context and Problem Statement

design.md asks for a `tm export` that loses nothing, a `tm import` that reads a published JSONL schema adapters write, and re-runs made idempotent by `source:id`. [ADR 0010](0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md) routes data that must outlive a pre-v1 operation format change through export and import. The research is in [import-export.md](../research/cli/import-export.md). What does an export hold, what may import do with it, and how does a re-import treat a record that changed?

## Decision Drivers

- The log is the source of truth ([ADR 0005](0005-operation-log-is-the-source-of-truth.md)). Measured: a store rebuilt from a copied log alone reproduced `tm show --json` and `tm list --json` byte for byte, while a state snapshot already loses a retracted comment's text.
- Adapters for beads, Jira, Linear and GitHub have Issues, not operations.
- A verbatim log cannot cross an in-place format change before v1 without translation code ADR 0010 defers.
- Measured: a log copied into a Workspace with its own data took the local Project's key, and the local Project and both its Issues vanished from every list; only `tm doctor` showed it.
- An edit made in tm must not be overwritten by re-running an import.

## Considered Options

- **What an export holds**: the log; a record per Issue; both in one file.
- **Restoring into a Workspace with data**: refuse; renumber incoming Issues.
- **A re-imported record that changed**: write the fields that differ; skip it; three-way compare.
- **Imported times and Actors**: the source's; the import's and its runner's.
- **Batches**: one per record; one per file.

## Decision Outcome

**One file, two layers.**

- `tm export` writes JSONL to stdout; saving it is the caller's redirect, so tm writes nothing outside its Workspace. Every line is an object with `_type`: a `header` (format, record schema version, operation format, Workspace id, export time), then `project` records, then every operation as an `op` record in replay order, then one `issue` record per Issue.
- An `op` record holds every logged column verbatim, its payload as the stored string, so each `op_hash` still verifies. Undo batches and the clock are local and never exported (ADR 0010, [ADR 0012](0012-a-human-undoes-their-last-command-without-naming-it.md)).
- The `issue` record is the published schema adapters write, documented with a JSON Schema in `docs/specs/`.

**Import restores a log, or writes records.**

- A file whose operation format matches this binary is restored: into a Workspace that does not exist on this machine, created under the exported id, or one already holding exactly those operations, which makes it a no-op. Operations are inserted verbatim, every hash is checked, the projections are rebuilt, and the clock observes the highest imported HLC under ADR 0006's cap.
- Into any other Workspace, restore is refused with `workspace_not_empty`, whose fix is `tm import --records FILE`. A file of another operation format is refused the same way, with the same fix.
- `--records`, and every adapter, write the `issue` records as ordinary validated writes: pass one creates or updates each Issue, pass two sets parents and Links once every target exists. A record from a tm export carries `tm:<Issue id>` as its source. History stays in the source file; the flag is the caller's choice to leave it there.
- `--dry-run` runs every check and rolls back.

**Sources make re-runs idempotent.**

- A built-in multi-valued field `sources` holds each Issue's `<system>:<id>` elements, such as `beads:tm-u8g.10`, written by `add` like labels; a re-import finds the Issue by it.
- A built-in field `imported` holds, for each source, the mapped values its last import wrote: one element per source, its value those values as a JSON string, projected into its own table. It is the base a re-import compares against; being logged it survives rebuild, export, restore and sync, which batches do not, and one element per source keeps two sources' bases apart under merge.
- Only import, and an undo of an import batch, write `sources` and `imported`; any other write naming either is refused. Both are shown in `--json`, not in `show`'s text.
- Each record is written as one batch, which sets `imported`.
- A re-import compares each mapped field three ways: `imported`'s value for that source, the source's value now, and tm's value now. A field changed only in the source is written; a field changed only in tm keeps tm's value; a field changed in both keeps tm's value and is reported as a conflict. A record with nothing to write is unchanged.

**Imported writes carry the source's time and person.**

- An imported operation's `utc_ms` is the source's time for it, and its Actor is the source's person mapped to a tm name; its `hlc` is still when this Replica wrote it. This narrows [ADR 0006](0006-operation-merge-ordering-and-versioning.md)'s `utc_ms`, "when it happened", to when it happened at the source.
- A batch that writes `imported` is never a bare-undo candidate, so a bare `tm undo` after an import reverts the human's own last command, not the last imported Issue. A named `tm undo <batch>` still reverts an import batch.

### Consequences

- Good, because the log layer loses nothing and serves as the switch milestone's daily backup.
- Good, because the record layer is one path for adapters, for a pre-v1 format change, and for bringing an export into a Workspace that has data.
- Good, because re-running an import never overwrites an edit made in tm.
- Bad, because a record import keeps current state and comments but not history, op identity or a retracted comment's text; tm says so before it writes.
- Bad, because undoing a whole import takes one undo per record; `--dry-run` is the guard against a wrong import.
- Neutral, because a Workspace merge that renumbers incoming Keys, which ADR 0008's placement of `assigned_key` permits, is left for later.

### Confirmation

Export, restore into a fresh Workspace and export again gives identical files apart from the header's time, over the rebuild property test's random logs. Restore into a Workspace with data is `workspace_not_empty` and writes nothing. Importing a file twice writes nothing the second time, and a field edited in tm survives a re-import whose source changed it too, reported as a conflict, including after the store is exported and restored between the two imports.

## More Information

- Amends ADR 0010's "moves through `tm export` and `tm import`": across a format change, through the records, with history left in the file.
- Amends ADR 0012's bare-undo candidate rule: a batch that writes `imported` is not a candidate.
- Narrows ADR 0006's `utc_ms` to when it happened at the source, for an imported operation.
- Research: [import-export.md](../research/cli/import-export.md). The beads mapping is decided with the beads adapter.
