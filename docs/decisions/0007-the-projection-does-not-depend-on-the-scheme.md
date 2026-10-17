# The projection does not depend on the Scheme

- Status: accepted
- Date: 2026-10-17
- Deciders: Jace Babin

## Context and Problem Statement

The projection is rebuilt from the operation log ([ADR 0005](0005-operation-log-is-the-source-of-truth.md)), and the Scheme says which types, statuses and fields exist. If the projection stores anything derived from the Scheme, such as a Status category, every Scheme edit leaves it stale until a rebuild, and two Replicas with different Schemes rebuild different tables from the same log. Where do Scheme-dependent values live, how are Scheme field values stored, and how do filters read them?

## Decision Drivers

- A rebuild must produce the same projection from the same log, whatever the Scheme.
- Editing the Scheme must not need a rebuild.
- Filters must stay indexed at 100k Issues, including negation.
- Operations are never rewritten ([ADR 0006](0006-operation-merge-ordering-and-versioning.md)), so what they reference must survive renames.

## Decision Outcome

- **Scheme-derived values are computed when read.** The projection stores the type and Status ids an Issue holds; its Status category comes from the Scheme at read time, and a Status the Scheme does not define reads as triage. Measured at 10,000 Issues: a stored category still counted 3,334 active Issues after a Scheme edit where the lookup counted the correct 1,667, for 0.110 ms against 0.083 ms ([research](../research/fields/scheme-and-workflow.md)).
- **The Scheme is configuration, not log.** Phase 1 compiles in a default Scheme; Phase 4 reads it from config ([ADR 0003](0003-issue-data-lives-outside-code-repos.md)). Scheme edits are never operations.
- **Ids are readable short names; labels are display.** Fields, types, Statuses and options are referenced by id; operations also record the display label of the field and of an enumerated value at write time, so history reads as it was written after a rename.
- **The Scheme is checked when this Replica writes, never on replay.** Replay applies what fits its column and records any operation that does not, with its reason, rather than stopping the rebuild.
- **Built-in fields are columns; Scheme fields are rows.** `issues` holds the built-ins, priority as an integer rank; `field_values` holds one row per Scheme field value with typed `txt` and `num` columns; sets are rows in `set_members`. One static index per typed column serves every Scheme field.
- **Filters compile each field term to `id IN (subquery)` or `id NOT IN (subquery)`.** At 100k Issues this form was within 2x of the fastest in every case, never duplicates rows, was the fastest negation (36 ms) and stays fast under `OR` (35 ms against 163 ms for `EXISTS`) ([research](../research/fields/filter-language.md)). `NOT IN` is only correct because `issue_id` is `NOT NULL` in both value tables; negation on a nullable column adds `IS NULL OR`.

### Consequences

- Good, because a Scheme edit takes effect on the next read, and rebuilds are reproducible across Replicas.
- Good, because adding a Scheme field adds rows, never DDL.
- Bad, because every read that shows a category or label consults the Scheme, and a filter on category expands to the (type, Status) pairs the Scheme defines.
- Bad, because `tm show` assembles an Issue from three tables, and the filter emitter generates subqueries rather than plain column tests.
- Neutral, because this supersedes the `status_category` column and the "joins, not EXISTS" filter shape proposed in [projection-schema.md](../research/store/projection-schema.md).

### Confirmation

Revisit if a measured View at realistic scale misses its budget because of the per-read Scheme lookup or the subquery form.
