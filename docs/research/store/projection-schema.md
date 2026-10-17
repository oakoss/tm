# Projection schema: what tables the store needs beside the operation log

- Date: 2026-10-17
- Author: Claude (research pass for Jace Babin)
- Scope: How do local-first apps, issue trackers and event-sourcing frameworks lay out materialized state next to an append-only log, and what does that say about the proposed projection layout for `tm-store`?

Method note: sources were read on 2026-10-17. GitHub sources were cloned and pinned to the commits listed under Sources; permalinks below use those commits. WebFetch returns summarised pages, so quotes from docs sites are near-verbatim excerpts. SQLite measurements ran on the macOS system `sqlite3` (`sqlite3 --version` printed `3.54.0 2026-04-09 … (64-bit)`, Apple build) on an Apple M2 Max (`sysctl -n machdep.cpu.brand_string`); `tm` will run the bundled SQLite instead, which is 3.53.2 (`#define SQLITE_VERSION "3.53.2"` in `libsqlite3-sys-0.38.2/sqlite3/sqlite3.h`, the version `rusqlite 0.40.2` resolves to in `Cargo.lock`). The probe script is [scripts/projection-probes.sql](scripts/projection-probes.sql) (run as `sqlite3 <scratch.db> < projection-probes.sql`). Anything not quoted or measured is labeled **inferred**.

## Question

The proposal under test: one `issues` row per Issue with typed columns for built-in fields plus a JSON column for Scheme fields; separate tables for set members (labels, file paths, Links; one row per element with add/remove state), field versions (which operation currently wins each field) and comments. All tables are projections of the operation log ([ADR 0005](../../decisions/0005-operation-log-is-the-source-of-truth.md)), merged by the rules in [ADR 0006](../../decisions/0006-operation-merge-ordering-and-versioning.md).

1. How do local-first apps on SQLite (and IndexedDB) model materialized state against a log?
2. How do trackers store custom fields, history, Links and search?
3. What is established practice for projections: rebuild, versioning, idempotent apply, read models per query?
4. What does the proposal miss?

## Sources

Local-first and sync engines:

- Actual Budget @ `8d59f30cfb36b177882116e0bd0dae39e30d2751` (v26.10.0, 2026-10-02): <https://github.com/actualbudget/actual/tree/8d59f30cfb36b177882116e0bd0dae39e30d2751>
- cr-sqlite @ `43ab94cac537d080aac7cb7628a435c02ee9e268` (last release v0.16.3, 2024-01-17): <https://github.com/vlcn-io/cr-sqlite/tree/43ab94cac537d080aac7cb7628a435c02ee9e268>
- LiveStore @ `21d65a6c2b1d40320d6e5c2660ecc0e3f1712280` (v0.5.0, 2026-10-15): <https://github.com/livestorejs/livestore/tree/21d65a6c2b1d40320d6e5c2660ecc0e3f1712280>
- Evolu @ `2b3e69f431dd2b9f61b8926fa7834168e249cb65` (`@evolu/common` 8.23.2, 2026-10-17): <https://github.com/evoluhq/evolu/tree/2b3e69f431dd2b9f61b8926fa7834168e249cb65>
- Riffle essay (2022): <https://riffle.systems/essays/prelude/>; UIST 2023 paper (abstract only): <https://dspace.mit.edu/handle/1721.1/152980>
- Linear, reverse-engineered by wzhudev @ `45dc91e78e03300b69cc088057a28c42cee35a41`: <https://github.com/wzhudev/reverse-linear-sync-engine>; Linear's post: <https://linear.app/blog/scaling-the-linear-sync-engine>
- Rocicorp mono (Replicache 15.2.1, Zero 1.12.0) @ `9e0e85b626b5a70e5dfa3a5de33e2ca1263c5628`: <https://github.com/rocicorp/mono>
- powersync-sqlite-core @ `0ed9f4edba418a4f8ae7cb0594da683cd79a81b5` (v0.5.4): <https://github.com/powersync-ja/powersync-sqlite-core>; docs: <https://docs.powersync.com/usage/lifecycle-maintenance/implementing-schema-changes>, <https://docs.powersync.com/usage/use-case-examples/raw-tables>
- electric-old (Satellite client, superseded) @ `b4da7af0e45ed7c34a5069640922ab8acca389bd`: <https://github.com/electric-sql/electric-old>
- TanStack DB @ `7d60818831806dc1a64ad61872533886ce875dbf` (db 0.15.1, db-sqlite-persistence-core 0.4.11): <https://github.com/TanStack/db>
- automerge-repo @ `8b178e75ecca89f78465196ef786aa844da57aec`: <https://github.com/automerge/automerge-repo>; <https://automerge.org/docs/reference/under-the-hood/storage/>
- Triplit 0.x @ `b66721f8e5f4cb9c1e1a7cdb23d754d183aca2c3` and 1.x @ `d3029ee13e747abeeab8cf5bf64427f77a142e1f`: <https://github.com/aspen-cloud/triplit>

Trackers:

- Jira data center schema: <https://developer.atlassian.com/server/jira/platform/database-custom-fields/>, <https://developer.atlassian.com/server/jira/platform/database-change-history/>, <https://developer.atlassian.com/server/jira/platform/database-issue-fields/>; indexing: <https://support.atlassian.com/jira/kb/understand-the-index-process-in-jira-server/>, <https://confluence.atlassian.com/enterprise/jira-data-center-search-indexing-939499986.html>
- Plane @ `879ce8b5c6b4fe21efff4789a7062747f46d6383`: <https://github.com/makeplane/plane>; <https://docs.plane.so/work-item-types>
- YouTrack and Xodus: <https://github.com/JetBrains/xodus/wiki/Entity-Stores>, <https://www.jetbrains.com/help/youtrack/server/custom-fields.html>
- GitHub GraphQL: <https://docs.github.com/en/graphql/reference/projects>, <https://docs.github.com/en/graphql/reference/issues>; <https://github.blog/changelog/2026-07-02-issue-fields-are-now-generally-available>
- beads @ `46aa59cc65f34b44193b055ca489087342e55dd1`: <https://github.com/gastownhall/beads>
- git-bug @ `cc9039fa961485e1cb49d865e224fd54a2f4a654` (2026-10-16): <https://github.com/git-bug/git-bug>
- Fossil tickets: <https://fossil-scm.org/home/doc/trunk/www/tickets.wiki>, <https://fossil-scm.org/home/doc/trunk/www/custom_ticket.wiki>, <https://fossil-scm.org/home/help/rebuild>

Event sourcing and SQLite:

- Marten: <https://martendb.io/events/projections/>, <https://martendb.io/events/projections/rebuilding.html>, <https://martendb.io/events/projections/async-daemon.html>, <https://martendb.io/tutorials/advanced-considerations>
- Axon streaming processors: <https://docs.axoniq.io/axon-framework-reference/4.10/events/event-processors/streaming/>
- Commanded Ecto projections: <https://raw.githubusercontent.com/commanded/commanded-ecto-projections/master/guides/Usage.md>
- Kurrent subscriptions: <https://docs.kurrent.io/clients/node/v1.0/subscriptions>
- Sequent migrations: <https://www.sequent.io/docs/concepts/migrations.html>
- Microsoft CQRS Journey: <https://learn.microsoft.com/en-us/previous-versions/msp-n-p/jj591577(v=pandp.10)>; Fowler: <https://martinfowler.com/eaaDev/EventSourcing.html>, <https://martinfowler.com/bliki/CQRS.html>
- SQLite: <https://www.sqlite.org/fts5.html>, <https://www.sqlite.org/expridx.html>, <https://www.sqlite.org/json1.html>, <https://www.sqlite.org/gencol.html>, <https://www.sqlite.org/pragma.html>, <https://www.sqlite.org/partialindex.html>

Not reached: Eventuous and Rails Event Store docs URLs returned 404; Greg Young's CQRS Documents PDF and the Riffle paper could not be text-extracted without installing tools.

Related notes in this repo, linked rather than repeated: [sync-engines.md](../sync-engine-spike/sync-engines.md) (engine selection), [clocks-and-timestamps.md](../op-log/clocks-and-timestamps.md), [hash-chained-log.md](../op-log/hash-chained-log.md), and Fossil's design in [git-native-file-trackers.md](../task-tracker-landscape/git-native-file-trackers.md).

## Findings

### 1. Local-first apps: where the version of a value lives

**Actual Budget keeps per-cell versions only in the log, not in the tables.** `messages_crdt(id, timestamp TEXT NOT NULL UNIQUE, dataset, row, column, value BLOB)` ([init.sql#L73-L85](https://github.com/actualbudget/actual/blob/8d59f30cfb36b177882116e0bd0dae39e30d2751/packages/loot-core/src/server/sql/init.sql#L73-L85)), indexed `ON messages_crdt(dataset, row, column, timestamp)` (`migrations/1612625548236_optimize.sql`). An incoming message is checked against that index: "Exactly this message is already in the log: nothing to do" (idempotence) and "A later message for this cell exists, so this one is old"; old messages "aren't applied but still go into the merkle trie" ([sync/index.ts#L263-L360](https://github.com/actualbudget/actual/blob/8d59f30cfb36b177882116e0bd0dae39e30d2751/packages/loot-core/src/server/sync/index.ts#L263-L360)). Apply and log insert run in one `db.transaction(applyInTransaction, { immediate: true })`. Deletes are a `tombstone INTEGER DEFAULT 0` column set by an ordinary message. Messages for unknown tables or columns wait in `messages_pending` and replay after migrations (`sync/messages-pending.ts`). The tables are not rebuilt from the log: reset sync runs `DELETE FROM messages_crdt; …` and keeps the tables (`sync/reset.ts#L53`); no replay-into-tables code was found (**inferred** from grep).

**Evolu does the same with a STRICT history table.** `evolu_history(ownerId, table, id, column, timestamp, value any) strict` with `unique index … ("ownerId","table","id","column","timestamp" desc)`; its comment: "stores all values … for conflict-free merging using last-write-win CRDT … Time travel is available" ([Db.ts#L749-L776](https://github.com/evoluhq/evolu/blob/2b3e69f431dd2b9f61b8926fa7834168e249cb65/packages/common/src/local-first/Db.ts#L749-L776)). A cell is written only `where not exists (select 1 from evolu_history where … "timestamp" >= ?)`, then the history row is inserted `on conflict do nothing` (`Db.ts#L959-L1022`). App tables carry `createdAt`, `updatedAt` and an `isDeleted` "Soft delete flag" but no per-cell versions (`Schema.ts#L446-L478`). Unknown columns go to `evolu_message_quarantine`. No rebuild-from-history code was found (**inferred**).

**cr-sqlite keeps a per-column version table and no log.** `"{table}__crsql_clock"(key, col_name, col_version, db_version, site_id, seq, PRIMARY KEY (key, col_name)) WITHOUT ROWID, STRICT` ([bootstrap.rs#L203-L224](https://github.com/vlcn-io/cr-sqlite/blob/43ab94cac537d080aac7cb7628a435c02ee9e268/core/rs/core/src/bootstrap.rs#L203-L224)). Row existence is a sentinel column `"-1"` whose `col_version` is the causal length; `let is_delete = insert_cl % 2 == 0;` (`changes_vtab_write.rs#L514-L531`). Only the latest version per cell survives, so there is no history to rebuild from.

**LiveStore is the closest match to tm: an event log plus disposable state tables.** "LiveStore operates two SQLite databases by default: a state database (your materialized tables) and an event log database" ([sqlite.md#L30](https://github.com/livestorejs/livestore/blob/21d65a6c2b1d40320d6e5c2660ecc0e3f1712280/docs/src/content/docs/building-with-livestore/state/sqlite.md)). The `eventlog` header says "NEVER modify eventlog schemas without bumping `liveStoreStorageFormatVersion`! Eventlog is the source of truth." (`eventlog-tables.ts#L16-L43`). The state file carries:

- `__livestore_state_head`: "Single-row marker for the latest event sequence number reflected by the state DB".
- `__livestore_rebuild(id PK)`: "Only a completed replay and successful migration hooks may insert the marker row."
- `__livestore_schema(tableName, schemaHash, …)` and a materialization journal of SQLite session changesets, used to roll back pending events on rebase with `makeChangeset(changeset).invert().apply()` (`MaterializationJournal.ts`).

The state file is named after a hash of the table definitions (``getStateDbBaseName = (schema) => `state${schema.state.sqlite.hash}` ``, `persistence.ts#L11`), so a table change opens a fresh file and replays: "A schema change opens a fresh state database and replays the full eventlog, even when only one table changed" and "If a rebuild fails or is interrupted, the next boot discards incomplete derived state and retries" (`sqlite-schema.mdx#L63-L71`). The hash covers table definitions only (`mod.ts#L59-L62`), so a change to materializer code alone does not trigger a rebuild (**inferred** from the hash inputs). Events the binary does not know "still enter the eventlog so newer clients can replay them" and materialize as no-ops. Because log and state are two files, the state write and the log append are not one transaction (**inferred** from code order in `materialize-event.ts#L102-L135`); tm's single-transaction rule is stricter.

**Riffle** argues for state in one reactive SQLite store, with derived data computed by queries rather than stored: "Most applications have some canonical, normalized base state which must be further queried, denormalized, and reshaped", and slow cases use "materialized views which are recomputed outside of the main synchronous reactive loop" ([prelude](https://riffle.systems/essays/prelude/)). It has no log or projection model to compare.

### 2. Sync engines: generic versus typed storage, and pending writes

- **No engine stores per-field versions in its materialized tables.** Linear has one global `lastSyncId`; each SyncAction carries `data: <full row>` (wzhudev README L1300-1380). PowerSync versions per bucket (`ps_buckets.last_applied_op`), Zero per row (`_0_version`), TanStack persistence per row (`row_version`), Triplit 1.x per entity. Per-field clocks existed in Triplit 0.x (`['EAT', EntityId, Attribute, Timestamp]`, `triple-store-utils.ts#L23-L57`) and Electric's Satellite client, and both were replaced by later designs.
- **Pending writes are usually kept apart from confirmed state and overlaid.** Replicache stores each pending mutation as a local commit and on pull "_rewinds_ the state of the Client View to the last version it got from the server, applies the patch… and then replays any pending mutations on top" ([how-it-works.md#L220](https://github.com/rocicorp/mono/blob/9e0e85b626b5a70e5dfa3a5de33e2ca1263c5628/packages/replicache-doc/docs/concepts/how-it-works.md#L220)). Linear's model stores "cannot contain changes that have not been approved by the server" (README L1202). PowerSync is the exception: local writes land in the data table and a downloaded checkpoint is published only when `SELECT 1 FROM ps_crud LIMIT 1` finds no pending upload ([sync_local.rs#L62-L89](https://github.com/powersync-ja/powersync-sqlite-core/blob/0ed9f4edba418a4f8ae7cb0594da683cd79a81b5/crates/core/src/sync/sync_local.rs#L62-L89)).
- **Generic JSON storage trades query cost for schema freedom.** PowerSync's default is `ps_data__<t>(id TEXT PRIMARY KEY NOT NULL, data TEXT)` with a typed view `SELECT id, CAST(json_extract(data,'$.a') as text) …` on top ([table_info.rs#L185-L212](https://github.com/powersync-ja/powersync-sqlite-core/blob/0ed9f4edba418a4f8ae7cb0594da683cd79a81b5/crates/core/src/schema/table_info.rs#L185-L212)); the docs say "Updating this client-side schema is immediate… Data is internally stored as JSON." PowerSync then added raw tables ("define native SQLite tables… it is the developer's responsibility to manage migrations") and, in unreleased core source, typed "direct" tables with automatic migration from the JSON form (`management.rs#L100-L105`).
- **A schema change is a rebuild, not a migration.** Linear's `createStores` drops every store whose schema hash changed (`e.deleteObjectStore(s)`, [html.js#L79841-L79857](https://github.com/wzhudev/reverse-linear-sync-engine/blob/45dc91e78e03300b69cc088057a28c42cee35a41/code/html.js#L79841-L79857)); TanStack's default `schemaMismatchPolicy` is `sync-present-reset`; Zero consumers handle a column change (`r`) by "resetting their state entirely" (`change-log.ts#L13-L83`).
- **Automerge-repo** stores no tables at all; state is opaque chunks keyed `[documentId, "snapshot" | "incremental", hash]`, compacted when "the incremental size is greater than the snapshot size" ([StorageSubsystem.ts](https://github.com/automerge/automerge-repo/blob/8b178e75ecca89f78465196ef786aa844da57aec/packages/automerge-repo/src/storage/StorageSubsystem.ts)). A query index would still be separate, as sync-engines.md already noted.

### 3. Trackers: custom fields, history, Links and search

**Custom fields.**

- Jira stores custom fields as EAV with typed value columns: "instances of custom fields are stored in `customfieldvalue`" and "each issue can have multiple `customfieldvalue` rows"; columns include `STRINGVALUE`, `NUMBERVALUE decimal(18,6)`, `TEXTVALUE`, `DATEVALUE`, `PARENTKEY`; multi-value fields are one row per value ([database-custom-fields](https://developer.atlassian.com/server/jira/platform/database-custom-fields/)). Built-in fields are columns: "Most fields in Jira are kept in the `jiraissue` table".
- Fossil uses real columns: "All the other fields of the TICKET and TICKETCHNG tables are available for customization", and key/value pairs with no matching column "are silently ignored" ([tickets.wiki](https://fossil-scm.org/home/doc/trunk/www/tickets.wiki)); an admin adds a field by editing the `CREATE TABLE` ([custom_ticket.wiki](https://fossil-scm.org/home/doc/trunk/www/custom_ticket.wiki)). Everything is `TEXT`.
- beads (now Dolt-only: "the SQLite storage layer and all migration infrastructure have been removed", CHANGELOG 0.58.0) has about 50 typed columns on `issues` plus `metadata JSON DEFAULT (JSON_OBJECT())`, documented as "arbitrary JSON data for extension points" (`internal/storage/schema/migrations/0001_create_issues.up.sql`, `types.go`). Extension fields became columns over time.
- GitHub models values as a typed union, one object per item and field: `ProjectV2ItemFieldValue` types Date, Iteration, Number ("float(8)"), SingleSelect (with both `name` and `optionId`), Text and more ([projects](https://docs.github.com/en/graphql/reference/projects)); 2026 issue fields add `IssueFieldValue` (Date, MultiSelect, Number, SingleSelect, Text) ([issues](https://docs.github.com/en/graphql/reference/issues)).
- Plane's open-source models have no custom properties (no `IssueProperty` in `apps/api` or `apps/web` at the pinned commit); docs place them in Pro ([work-item-types](https://docs.plane.so/work-item-types)), storage unknown. YouTrack documents fields as global and attached per project ([custom-fields](https://www.jetbrains.com/help/youtrack/server/custom-fields.html)) but not their storage.

**History pairs an identifier with a display string.** Jira: "Each change to the issue triggered by a user inserts one record into the changegroup table", each `changeitem` holding the field and old/new values; "The OLDVALUE column records the ID of the changed entity" and "OLDSTRING records the name for the entity, so that if the entity is removed from the system" history still reads ([database-change-history](https://developer.atlassian.com/server/jira/platform/database-change-history/)). Plane's `IssueActivity` has `old_value`/`new_value` and `old_identifier`/`new_identifier`; GitHub's SingleSelect value holds both `name` and `optionId`.

**Links are stored once and named in both directions by the type.** Jira's `issuelink(LINKTYPE, SOURCE, DESTINATION, SEQUENCE)` with `issuelinktype(LINKNAME, INWARD, OUTWARD, …)`; Plane's `IssueRelation(issue, related_issue, relation_type)` with `_RELATION_PAIRS = (("blocked_by","blocking"), …)` in code ([issue.py#L272-L305](https://github.com/makeplane/plane/blob/879ce8b5c6b4fe21efff4789a7062747f46d6383/apps/api/plane/db/models/issue.py#L272-L305)). beads: `dependencies(issue_id, depends_on_id, type DEFAULT 'blocks', …, PRIMARY KEY (issue_id, depends_on_id))`.

**The ready queue started as a view and became a maintained column in beads.** Migration 0017 created `VIEW ready_issues` with a recursive CTE capped at `depth < 50`; migration 0046 added `is_blocked TINYINT(1)` indexed with `status`, "the denormalized, transitive is_blocked column … maintained by the write paths" (`types.go`). The older SQLite build had a `blocked_issues_cache` table ("Performance optimization for GetReadyWork", CHANGELOG 0.24.0).

**Keys survive their Issues.** Jira keeps moved Keys in `moved_issue_key(id, old_issue_key, issue_id)` (KB pages only, thin: <https://support.atlassian.com/jira/kb/reset-the-project-counter-and-reuse-jira-issue-keys-after-moving-issues/>). Plane's `IssueSequence` uses `on_delete=models.SET_NULL … # we want to keep the sequence even if the issue is deleted` (`issue.py`). Fossil's integer `tkt_id` keys "are for internal use only and may change when doing a 'fossil rebuild'"; the stable identity is `tkt_uuid`. git-bug resolves ID prefixes and fails with `ErrMultipleMatch` when several match (`cache/subcache.go#L322-L328`).

**Soft delete needs partial unique indexes.** Plane sets `deleted_at`, filters it in the default manager, and writes constraints like `UniqueConstraint(fields=["issue","related_issue"], condition=Q(deleted_at__isnull=True))` ([mixins.py#L61-L80](https://github.com/makeplane/plane/blob/879ce8b5c6b4fe21efff4789a7062747f46d6383/apps/api/plane/db/mixins.py#L61-L80)).

**Search is a separate rebuildable read model.** Jira: "The database is the canonical source of truth for Jira and the index uses SQL to read the database so it can build the index", and "Jira does not support 2 phase commits between DB and index". git-bug replaced bleve with one SQLite cache: `docs(… excerpt BLOB CHECK (json_valid(excerpt, 8)) …)`, a contentless `fts USING fts5(tokens, content='', contentless_delete=1, …)`, a `built_from` column for incremental refresh, and namespaces versioned so "processes of different versions [can] share the database"; on corruption "the index only holds what the cache can rebuild from the repository, so the file can go" ([index_sqlite.go#L75-L98](https://github.com/git-bug/git-bug/blob/cc9039fa961485e1cb49d865e224fd54a2f4a654/repository/index_sqlite.go#L75-L98)). Plane has no index: `Q(name__icontains)` (`views/search/base.py#L84-L94`).

**git-bug computes state as a fold over operations**: `for _, op := range bug.Operations() { op.Apply(snap); … }` (`entities/bug/bug.go#L141-L153`), and its `SetMetadataOp` is marked `OperationDoesntChangeSnapshot`, keeping metadata beside the snapshot.

### 4. Event-sourcing projection practice

- **Checkpoint and projection commit together.** Marten's daemon commits "each batch of projected documents plus its progression update" ([async-daemon](https://martendb.io/events/projections/async-daemon.html)). Axon: "Axon Framework will combine committing the event handling task with updating the token", and if rollback is impossible, "we strongly recommend making the task idempotent" ([streaming](https://docs.axoniq.io/axon-framework-reference/4.10/events/event-processors/streaming/)). Commanded: "The `projection_versions` table is used to ensure that events are only projected once" with `last_seen_event_number`, all "executed within a single transaction" ([Usage.md](https://raw.githubusercontent.com/commanded/commanded-ecto-projections/master/guides/Usage.md)). tm's same-transaction projection is Marten's Inline lifecycle: "executed at the time of event capture and in the same unit of work" ([projections](https://martendb.io/events/projections/)).
- **Rebuild is drop and replay.** Fowler: "We can discard the application state completely and rebuild it by re-running the events from the event log" ([EventSourcing](https://martinfowler.com/eaaDev/EventSourcing.html)). Marten: "Projections need to be rebuilt when the code that defines them changes in a way that requires events to be reapplied". Fossil's rebuild is manual: "Run this command after updating the fossil executable in a way that changes the database schema" ([help/rebuild](https://fossil-scm.org/home/help/rebuild)).
- **Versioned projections build beside the old ones.** Marten: "Increment `ProjectionVersion` on your projection class to create a new version that writes to separate database tables", and "Marten does not automatically drop the old tables" ([advanced-considerations](https://martendb.io/tutorials/advanced-considerations)). Sequent replays into a `replay_schema`, catches up, then archives the old tables ([migrations](https://www.sequent.io/docs/concepts/migrations.html)).
- **Replay must not repeat side effects.** Axon offers `@ResetHandler`, `@DisallowReplay` and `ReplayStatus` for this.
- **Read models are shaped per query.** CQRS Journey: a read store "denormalized to suit the specific queries that the application must support", and "Ideally, the recipient of the data should not need to perform any joins". Snapshots "typically only provide a performance benefit when an aggregate has a very large number of events" ([CQRS Journey](https://learn.microsoft.com/en-us/previous-versions/msp-n-p/jj591577(v=pandp.10))). Fowler warns "for most systems CQRS adds risky complexity" ([CQRS](https://martinfowler.com/bliki/CQRS.html)).
- **Version stamp on open.** No fetched primary source rebuilds automatically on startup except LiveStore (state file named by schema hash plus a completion marker). SQLite provides the slots: `user_version` "is available to applications to use however they want", and `application_id` lets "utilities such as file(1) … determine the specific file type" ([pragma](https://www.sqlite.org/pragma.html)).

### 5. SQLite behavior that constrains the layout (measured)

From `projection-probes.sql`; output quoted verbatim.

**(a) An expression index is used only when the query spells the expression the same way.** The docs: the expression must appear "_exactly_ as it is written in the CREATE INDEX statement. The query planner does not do algebra." ([expridx](https://www.sqlite.org/expridx.html)). Measured with an index on `json_extract(fields,'$.area')`:

```text
EXPLAIN QUERY PLAN SELECT id FROM issues WHERE json_extract(fields,'$.area')='cli';
`--SEARCH issues USING COVERING INDEX issues_area (<expr>=?)
EXPLAIN QUERY PLAN SELECT id FROM issues WHERE fields->>'$.area'='cli';
`--SCAN issues
```

An index on `fields->>'$.area'` was not used by `fields->>'area'`, though the JSON docs define the two as the same path. `ORDER BY fields->>'$.due'` fell back to `USE TEMP B-TREE FOR ORDER BY`.

**(b) A generated column fixes the spelling only if queries name the column.** With `area … GENERATED ALWAYS AS (json_extract(fields,'$.area')) VIRTUAL` indexed, `WHERE area='cli'` gave `SEARCH issues2 USING INDEX issues2_area (area=?)`, while `WHERE json_extract(fields,'$.area')='cli'` gave `SCAN issues2`. Adding a VIRTUAL column is allowed by `ALTER TABLE`, a STORED one is not ([gencol](https://www.sqlite.org/gencol.html)); either way a new Scheme field would be a DDL change.

**(c) EAV with one static index works if filters are joins, not `EXISTS`.** On 2,000 Issues after `ANALYZE`, two `EXISTS` filters scanned every Issue and probed the index twice per row (`SCAN i USING COVERING INDEX issues_due` then two `SEARCH f EXISTS USING COVERING INDEX fv_idx …`); the join form started from the selective lookup:

```text
|--SEARCH a USING COVERING INDEX fv_idx (field=? AND txt=?)
`--SEARCH b USING COVERING INDEX fv_idx (field=? AND txt=? AND issue_id=?)
```

**(d) FTS5 external content drifts silently, and only `integrity-check` with rank 1 detects it.** After dropping the update trigger and retitling row 1 from "crash" to "hang":

```text
stale search crash:|1|Fix login hang
stale search hang:|0
INSERT INTO docs_fts(docs_fts) VALUES('integrity-check');
INSERT INTO docs_fts(docs_fts, rank) VALUES('integrity-check', 1);
Error near line 67: fts5: checksum mismatch for table "docs_fts"
INSERT INTO docs_fts(docs_fts) VALUES('rebuild');
after rebuild hang:|1|Fix login hang
```

The plain check passed on the stale index, matching the docs: the index is compared with the content table "only … if the value specified for the rank column is 1" ([fts5](https://www.sqlite.org/fts5.html)). `'rebuild'` "is not available with contentless tables", so a contentless index (git-bug's choice) must be rebuilt by re-inserting from the projection.

**(e) A full rebuild of 100,000 operations into 10,000 Issues took about 52 ms once the log was indexed by Issue.** Without `ops(issue_id, seq)` the same SQL took `Run Time: real 49.719847`; with it, `INSERT` 0.010 s, `UPDATE` 0.040 s, `COMMIT` 0.0015 s, and Issue 1 ended `1|v8|v9|v10|10`, matching its last three set operations. This was a simplified last-write-wins in SQL; a Rust replay applying ADR 0006's concurrency rule, sets, comments and FTS is **unmeasured**.

**Bundled build flags.** `libsqlite3-sys-0.38.2/build.rs` compiles with `-DSQLITE_ENABLE_FTS5`, `-DSQLITE_ENABLE_JSON1` and `-DSQLITE_DEFAULT_FOREIGN_KEYS=1` (read from the file, not exercised). With foreign keys on by default, a rebuild that deletes and refills tables in arbitrary order will fail on cross-table references unless it orders the work or sets `PRAGMA defer_foreign_keys` (**inferred**, not run).

## Conclusions

### What the proposal gets right

- Typed columns for built-in fields and side tables for set members and comments match Jira, Plane and beads, and every source agrees that tables derived from a log can be dropped and rebuilt (Fossil, LiveStore, git-bug, Marten).
- Keeping per-field winners outside the `issues` row matches Actual, Evolu and cr-sqlite, which all keep versions in a side structure keyed by (row, column).

### What it is missing or should change

1. **The log itself needs query columns and indexes**, or every per-field decision, history view and undo scans payloads. Probe (e) went from 49.7 s to 52 ms with one index. Actual and Evolu decide winners directly from an index on (row, column, timestamp). The `ops` table needs `issue_uuid`, `field` and `elem` as plain columns beside the payload, with indexes for per-field lookup, per-Issue history, undo by Actor and pending operations.
2. **Field versions must cover set elements and hold enough to re-run ADR 0006.** "Which op wins" is not enough: a pending operation applies locally after every acknowledged one and is decided again on acknowledgement, and local writes are version-checked ("a conflicting change to the same field is refused with the current value", [design.md](../../specs/design.md)). The row needs `op_id`, `hlc`, `base_seq` and nullable `server_seq` per (Issue, field, element), including removed elements, so set members can drop removed rows.
3. **Losing values need a record.** ADR 0006 says "the losing value stays in history with a Notification". The log keeps the value; a `field_conflicts` row is what lets Notifications, `tm doctor` and history find it without re-deriving every merge.
4. **Keys are not columns on `issues`.** Keys are never reused even after purge, provisional Keys "remain aliases forever", and merged Workspaces keep "source-tagged aliases" (design.md). Jira (`moved_issue_key`), Plane (`IssueSequence` surviving deletion) and Fossil (stable `tkt_uuid` versus disposable `tkt_id`) all separate the two.
5. **The store needs meta rows and a projection stamp.** ADR 0006 puts HLC state in the store; design.md requires a "schema-version guard" so an older binary refuses a newer store. LiveStore's pattern (a fingerprint of the projection, plus a marker written only after a completed replay) is the clearest prior art. Adding a projection-code version to that fingerprint closes the gap where LiveStore misses materializer-only changes (**inferred** recommendation).
6. **Prefer EAV with typed value columns over one JSON column for Scheme fields.** This is a recommendation, not a decision. A JSON column needs either one expression index per filterable field, created as DDL when the Scheme changes and matched exactly by the filter emitter (probe a), or generated columns, which are also DDL (probe b). An EAV table (`field_values`, as Jira does) needs one static index, stores multi-select and file paths as one row per value, gives typed sort through `num`/`txt` columns, and probe (c) shows joins use the index. The cost is that `tm show` assembles fields from rows and the filter emitter must generate joins, not `EXISTS`. PowerSync's history (JSON plus views, then raw and typed tables that bypass the JSON layer) points the same way; its reason for adding them is not stated in the sources read.
7. **The projection depends on the Scheme, not only the log.** A status category, a field's type (for sort and comparison) and the select options all come from config. A rebuilt `status_category` or typed value is a function of (log, Scheme), so the rebuild property test must fix the Scheme as an input, and a Scheme change implies a rebuild or a `tm doctor` pass.
8. **History values should carry display text at write time.** Jira, Plane and GitHub all store an identifier and a display string together. Operations are never rewritten (ADR 0006), so whether a `set_field` payload for a select option or person carries its label must be settled before the first operation is written (**inferred** recommendation).
9. **Claims need a projection in Phase 1.** "Take-only-if-unclaimed" is an invariant in tm-u8g.4 and `claim` is a Phase 1 command, so a claims table cannot wait for Phase 3. Expiry is time-dependent, so the table stores `expires_utc_ms` and readers compare with now; a rebuild must not expire Claims itself.
10. **The ready queue should start as a query, not a table.** beads went from a recursive-CTE view to a cache table to a maintained `is_blocked` column; starting with the query and measuring before denormalizing avoids a third projection to keep correct (**inferred**).
11. **Rebuild must be side-effect free.** Axon's replay controls exist because replays re-fire handlers. If Notifications are derived during apply, a rebuild re-creates them; their read state must therefore live in the log or in a table rebuild does not touch (**inferred**).
12. **Search indexes need explicit maintenance.** External-content FTS5 needs triggers and drifts silently without them (probe d); `tm rebuild` should run `'rebuild'`, and `tm doctor` should run `integrity-check` with rank 1.

### Recommended tables

`ops` and `store_meta` are the source of truth and store state; everything else is a projection that `tm rebuild` drops and refills. Op kinds are ADR 0006's: `create`, `set_field`, `add`, `remove`, `comment`, `link`, `archive`, `claim`.

| Table | Purpose | Key columns | Written by | Needed by |
|---|---|---|---|---|
| `ops` | Operation log (source of truth, not a projection) | `local_seq INTEGER PRIMARY KEY`, `op_id UNIQUE`, `op_hash`, `v`, `kind`, `replica_id`, `actor`, `project_id`, `issue_uuid`, `field`, `elem`, `hlc`, `base_seq`, `utc_ms`, `offset_min`, `payload BLOB`, nullable `server_seq`, `server_received_ms`, `server_effects`, `chain_hash`, `status` (pending, acked, rejected); indexes on `(issue_uuid, local_seq)`, `(issue_uuid, field, elem)`, `(actor, local_seq)`, partial on `server_seq IS NULL` | every write | .3 |
| `store_meta` | Store-resident state: format version for the older-binary guard, `replica_id`, HLC state, projection fingerprint, rebuild-complete marker; `PRAGMA application_id` set once | key, value, including `projected_through_local_seq` (Commanded's `last_seen_event_number`) | every write (HLC); open; rebuild | .3 |
| `issues` | One row per Issue, built-in fields | `uuid PK`, `project_id`, `type`, `title`, `description`, `status`, `status_category` (from Scheme), `priority`, `assignee`, `reporter`, `parent_uuid`, `archived`, `created_utc_ms`, `updated_utc_ms`, `last_local_seq`; partial indexes `WHERE archived = 0` are used only when the query contains that literal term ([partialindex](https://www.sqlite.org/partialindex.html)), which .7's emitter must produce | `create`, `set_field`, `archive` | .3 table, .6 columns |
| `field_heads` | Winning operation per field and per set element, for version-checked writes and ADR 0006 re-decision | `(issue_uuid, field, elem)` PK (`elem = ''` for single values), `op_id`, `hlc`, `base_seq`, `server_seq`, `present` | `set_field`, `add`, `remove`, `link`, `archive`, `claim` | .3 |
| `issue_keys` | Every Key and alias ever assigned; never deleted, not even by purge | `key_text PK`, `project_id`, `number`, `issue_uuid`, `kind` (current, provisional, import, renumbered), `source` | `create` (local authority), import, later server effects | .5 (shape fixed in .3) |
| `key_counters` | Next number per Project; derivable as max over `issue_keys`, kept as a row for `BEGIN IMMEDIATE` assignment | `project_id PK`, `next_number` | `create` | .5 |
| `field_values` | Scheme field values, one row per value; a multi-value row is identified by its element, since sets merge per element | `(issue_uuid, field, elem)` PK (`elem = ''` for single values, else the element), `txt`, `num`, `ref_uuid`; index `(field, txt, issue_uuid)` and `(field, num, issue_uuid)` | `set_field`, `add`, `remove` on Scheme fields | .6 |
| `set_members` | Present elements of built-in sets (labels, file paths) | `(issue_uuid, set_name, elem)` PK; index `(set_name, elem)` | `add`, `remove` | .3 (labels), .6 |
| `links` | Present Links, stored once per deterministic key | `link_key PK` (hash of type, source, target), `link_type`, `src_uuid`, `dst_uuid`; index on `dst_uuid` | `link` (add and remove forms) | .4 (deterministic key), .8 (`ready`) |
| `comments` | Comment bodies, append-only, author edits applied | `comment_id` (= creating `op_id`) PK, `issue_uuid`, `actor`, `body`, `created_utc_ms`, `edited_op_id`, `deleted` | `comment` | .8 |
| `claims` | Current Claim per Issue | `issue_uuid PK`, `agent`, `on_behalf_of`, `claim_op_id`, `expires_utc_ms`, `provisional` | `claim` | .4, .8 |
| `external_refs` | `source:id` to Issue, for idempotent re-import | `(source, source_id) PK`, `issue_uuid` | `create` from import | .10 |
| `field_conflicts` | Losing values of concurrent edits, for history and Notifications | `(issue_uuid, field, elem, loser_op_id)`, `winner_op_id`, `decided_at_local_seq` | merge on acknowledgement | Phase 9 (reserve in .3) |
| `issue_fts` | FTS5 external content over titles, descriptions, comments | rowid mapped to an integer per Issue or comment | triggers on `issues`, `comments` | Phase 7 |
| `notifications` | Per-Actor notices; read state must survive rebuild | `id`, `actor`, `issue_uuid`, `cause_op_id`, `read` | apply of mentions, conflicts, Claim events | Phase 3 |
| `chain_heads`, `sync_state` | Per-Project chain head and pull cursor | `project_id PK`, `server_seq`, `chain_hash` | sync | Phase 9 (meta rows exist from .3) |
| `attachments` | Content-hash index of stored files | `hash PK`, `size`, `mime`, plus per-Issue rows | `add` on an attachments set | Phase 7 |

Left as queries, not tables: per-field history (an `ops` scan by `(issue_uuid, field)`), the ready queue (a recursive query over `links`, `issues` and `claims`), Views (config, not store).

## Implications / actions

**tm-u8g.3 must include:**

- `ops` with the indexed query columns above, so ADR 0006's decision and per-field history read an index, not payloads.
- `store_meta` with the format version, `replica_id`, HLC state, projection fingerprint and a rebuild-complete marker; `PRAGMA application_id`.
- `issues`, `field_heads`, `set_members`, `links`, `comments`, at least as empty shapes, so the rebuild property test covers every op kind from the start.
- A place in the log for an assigned Key. ADR 0006 puts a server-assigned Key in `server_effects`; the local authority must record its Key the same way (an effects column on the `create` operation, outside `op_hash`), or `issue_keys` cannot be rebuilt and the rebuild property test does not cover Keys. Purged Issues must keep this column so a Key is never reused.
- The `issue_keys` shape (filled by .5) and the `claims` shape (filled by .4 and .8), so later beads add rows, not a projection redesign.
- `tm rebuild` that runs inside one `BEGIN IMMEDIATE`, defers foreign keys, re-derives every projection with no side effects, and writes the marker last. Opening a store whose fingerprint differs rebuilds it; the read-only snapshot (.11) instead refuses with a message, since `immutable=1` cannot write.
- The rebuild property test taking the Scheme as a fixed input.
- A decision, before the first operation is written, on whether `set_field` payloads carry display labels as well as identifiers.

**Can wait:** `field_values` and the JSON-versus-EAV choice (.6, with .7's emitter), `external_refs` (.10), `field_conflicts` (Phase 9, reserved name), FTS (Phase 7), Notifications (Phase 3), chain heads and sync cursors (Phase 9), attachments (Phase 7).

**ADRs this could drive:** the JSON-versus-EAV choice for Scheme fields, once .6 and .7 test it, since reversing it later is a rebuild plus every filter query.

## Open questions

- **Purge versus an append-only log.** `tm purge` is "the only permanent removal", but the log is the source of truth and its payloads feed `op_hash`. Compaction already keeps `op_hash` when it drops payloads, so redacting a purged Issue's payloads may fit the same rule; not designed.
- **Archive and Link op kinds.** ADR 0006 calls archive "an ordinary field" but lists an `archive` kind, and `link` merges as a set by add and remove; whether `link` has add and remove forms, or Links use `add`/`remove` on a Link set, decides the `field_heads` key.
- **Scheme changes and rebuild.** Whether a Scheme edit triggers a full rebuild or a targeted re-derive of `status_category` and typed values; and how a field rename (an "explicit migration" in design.md) appears in the log.
- **Notification read state.** Whether marking read is an operation (synced, rebuildable) or local state outside the projection.
- **Manual ordering.** design.md has no rank field for Board order; Plane uses `sort_order = models.FloatField(default=65535)`. If one is added, it is a set-like or LWW field whose type affects `field_heads`.
- **Rebuild cost in Rust with the full ADR 0006 rule.** Probe (e) measured simplified SQL only; the cold-start budget in the build plan should include a rebuild measurement.
- **Two transactions or one under a crash.** LiveStore splits log and state across files; tm's single transaction avoids that, but the snapshot publisher (.11) copies both, and whether a snapshot ever captures a store mid-rebuild depends on publishing only after the marker is written.

## Raw data

Full probe output was captured from `sqlite3 m.db < projection-probes.sql` in a scratch directory; the excerpts above are verbatim from it. The script is [scripts/projection-probes.sql](scripts/projection-probes.sql).
