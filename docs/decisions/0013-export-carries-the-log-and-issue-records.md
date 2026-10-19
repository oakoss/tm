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
- **Where an import's provenance lives**: fields on the Issue holding its sources and what each last wrote; a source on each operation it writes.
- **Imported times and Actors**: the source's; the import's and its runner's.
- **Batches**: one per record; one per file.

## Decision Outcome

**One file, two layers.**

- `tm export` writes JSONL to stdout; saving it is the caller's redirect, so tm writes nothing outside its Workspace. Every line is an object with `_type`: a `header` (format, record schema version, operation format, Workspace id, export time), then `project` records, then every operation as an `op` record in replay order, then one `issue` record per Issue.
- An `op` record holds every logged column verbatim, its payload as the stored string, so each `op_hash` still verifies. Lines are canonical: operations in replay order, keys in a fixed order, ids in lowercase hex, so the same log always exports the same bytes. Undo batches and the clock are local and never exported (ADR 0010, [ADR 0012](0012-a-human-undoes-their-last-command-without-naming-it.md)).
- The `issue` record is the published schema adapters write, documented with a JSON Schema in `docs/specs/`.

**Import restores a log, or writes records.**

- A file whose operation format matches this binary is restored: into a Workspace that does not exist on this machine, created under the exported id, or one already holding exactly those operations, which makes it a no-op. Operations are inserted verbatim, every hash is checked, the projections are rebuilt, and the clock observes the highest imported HLC under ADR 0006's cap.
- Outside a repo, restore creates the exported Workspace under `TM_HOME` (default `~/.tm`), so every repo mapping it works. Inside one, the repo file's Workspace must be the exported one, or the restore is refused with `workspace_mismatch` naming both, whose fix is `tm import --records FILE`; running `tm init` before restoring lands here, since init makes a new Workspace. A command run where the repo file names a Workspace this machine lacks, as on a fresh clone, is refused with `unknown_workspace`, whose fix is `tm import FILE`.
- Into the exported Workspace holding other operations, restore is refused with `workspace_not_empty`, whose fix is `tm import --records FILE`. A file of another operation format is refused the same way, with the same fix.
- A restore warns that the Workspace now lives on this machine too. Until sync, two live copies of one Workspace each assign the same next Keys, the collision [ADR 0004](0004-one-authority-assigns-keys.md) exists to prevent, so the copy restored from is to be retired.
- `--records`, and every adapter, write the `issue` records as ordinary validated writes: pass one creates or updates each Issue, pass two sets parents and Links once every target exists. A record from a tm export carries `tm:<Issue id>` as its source. History stays in the source file; the flag is the caller's choice to leave it there.
- `--dry-run` runs every check and rolls back.

**Each imported operation names its source.**

- An operation gains an optional `source`, such as `beads:tm-u8g.10`, hashed after `on_behalf_of` behind a presence byte as ADR 0010 added that field, and stored in an indexed `ops` column covered by the `ops_immutable` trigger. Only import writes it; an undo writes its inverses without one.
- A re-import finds the Issue whose `create` carries the record's source. `show --json` gives an Issue's source from its `create`.
- What the last import wrote to a field or element is the operation that wins among those on it carrying that source, folded in replay order by the ordinary merge rule, whether or not it beat tm's own writes. Being in the log, it survives rebuild, export, restore and sync, and it merges by the rules every operation does, so no separate record of it can disagree.
- A re-import compares each mapped field three ways: that value, the source's value now, and tm's value now. A field changed only in the source is written; a field changed only in tm, an undone import included, keeps tm's value; a field changed in both keeps tm's value and is reported as a conflict. When the source changed to the value tm already holds, the import still writes it, so the next comparison starts from it. A record with nothing to write is unchanged.
- A comment, which has no field to compare, carries its own source, the record's followed by `#` and the comment's id at the source. A re-import skips a comment already carrying its source and reports a changed body without writing it, since a comment is not edited. It retracts a comment gone from the source, with the comment's source, as the comment's own Actor at the import's time, unless a retraction, or an undo of one, has been written since the comment was; so it does not retract twice, and a comment shown again in tm stays shown.
- Each record is written as one batch.

**Imported writes carry the source's time and person.**

- An imported operation's `utc_ms` is the source's time for it, and its Actor is the source's person mapped to a tm name; its `hlc` is still when this Replica wrote it. This narrows [ADR 0006](0006-operation-merge-ordering-and-versioning.md)'s `utc_ms`, "when it happened", to when it happened at the source.
- A batch holding an operation with a source is never a bare-undo candidate, so a bare `tm undo` after an import reverts the human's own last command, not the last imported Issue. A named `tm undo <batch>` still reverts an import batch.

### Consequences

- Good, because the log layer loses nothing and serves as the switch milestone's daily backup.
- Good, because the record layer is one path for adapters, for a pre-v1 format change, and for bringing an export into a Workspace that has data.
- Good, because re-running an import never overwrites an edit made in tm, and history can say where each imported value came from.
- Bad, because the operation envelope grows by one field, which amends ADR 0010's "Nothing else joins the envelope"; before v1 the format changes in place.
- Bad, because a record import keeps current state and comments but not history, op identity or a retracted comment's text; tm says so before it writes.
- Bad, because undoing a whole import takes one undo per record; `--dry-run` is the guard against a wrong import.
- Bad, because nothing but the warning stops a Workspace from living on two machines before sync; sync, which owns Key assignment across Replicas, is where that is prevented.
- Neutral, because a Workspace merge that renumbers incoming Keys, which ADR 0008's placement of `assigned_key` permits, is left for later.

### Confirmation

Export, restore into a Workspace this machine lacks and export again gives identical files apart from the header's time, over the rebuild property test's random logs. Restore into the exported Workspace holding other operations is `workspace_not_empty`, and into a repo mapped to another Workspace is `workspace_mismatch`; each writes nothing and names `tm import --records FILE`. A fresh clone's commands are `unknown_workspace` naming `tm import FILE`. Importing a file twice writes nothing the second time, and a field edited in tm survives a re-import whose source changed it too, reported as a conflict, including after the store is exported and restored between the two imports.

## More Information

- Amends ADR 0010's "moves through `tm export` and `tm import`": across a format change, through the records, with history left in the file.
- Amends ADR 0010's envelope: an operation's optional `source` joins it.
- Amends ADR 0012's bare-undo candidate rule: a batch holding an operation with a source is not a candidate.
- Narrows ADR 0006's `utc_ms` to when it happened at the source, for an imported operation.
- Research: [import-export.md](../research/cli/import-export.md). The beads mapping is decided with the beads adapter.
