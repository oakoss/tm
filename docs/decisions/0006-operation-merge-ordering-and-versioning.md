# Operation merge, ordering and versioning

- Status: accepted
- Date: 2026-10-17

The operation log is the source of truth ([ADR 0005](0005-operation-log-is-the-source-of-truth.md)), so these rules must be fixed before the first operation is written: what one operation is, how Replicas order and merge them, how time is recorded, how a Replica proves it agrees with the server, and how the format evolves. The research is in [clocks-and-timestamps.md](../research/op-log/clocks-and-timestamps.md) and [hash-chained-log.md](../research/op-log/hash-chained-log.md).

## Operations

- An operation is one typed, field-level change: `create`, `set_field`, `add` or `remove` on a set, `comment`, `link`, `archive`, `claim`. Edits to different fields never conflict.
- Each operation carries `kind` and a format version `v`. The code keeps a translation from every old version to the current shape, applied when reading; a stored operation is never rewritten. A version newer than the binary understands stops sync with an upgrade message instead of being skipped. Table schema changes need no operation version, because tables are rebuilt from the log. Until tm v1 is released the format changes in place instead ([ADR 0010](0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md)).

## Order and time

Each operation records three kinds of time, none standing in for another:

| Field | Purpose |
|---|---|
| `hlc` | Hybrid logical clock as one sortable integer (`ms << 16 \| counter`, 16-bit counter), tie-broken by `replica_id` |
| `utc_ms`, `offset_min` | When it happened, and the UTC offset where the Actor wrote it (the writing Replica's offset), for display; for an imported operation, when it happened at the source ([ADR 0013](0013-export-carries-the-log-and-issue-records.md)) |
| `server_seq`, `server_received_ms` | Authoritative order and server receive time, set when the server acknowledges it |

- The clock is written in-house and its state lives in the store, read and written in the same `BEGIN IMMEDIATE` transaction as the operation. Every examined HLC crate keeps state in process memory, and `tm` runs as many short-lived processes.
- When the counter would pass 0xFFFF, the clock moves to the next millisecond with counter 0, so the integer stays ordered.
- A pulled operation is always applied, but advances the local clock only up to now + 5 minutes; the clock never moves backwards. `tm sync status` reports measured skew.
- `op_id` is a UUIDv7 used as identity and as the final tie-break; its embedded time is never read, because it is ordered only within one process.
- History shows "theirs" as `utc_ms` at the recorded offset and "yours" in the viewer's zone. Offsets, not zone names, are recorded.

## Merge rules

| Data | Rule |
|---|---|
| Single values (status, title, priority, Assignee, single-value fields) | Per-field last writer wins, as below |
| Sets (labels, multi-select, file paths, Links) | Add and remove per element; the later operation for an element wins, decided as for single values |
| Long text (description, plan) | Whole-field last writer wins; the losing text stays in history with a Notification |
| Comments | Append-only; only the author's edits apply |
| Archive | An ordinary field; edits to an archived Issue still apply |
| Purge | Final; later operations on a purged Issue are rejected visibly |
| Claims | Decided by the authority, like Keys; an offline Claim is provisional and its Agent is notified if it loses |

"Last writer" is decided by concurrency, not arrival. Each operation records `base_seq`, the server position it was based on. An operation with no concurrent change to the same field since its `base_seq` applies in server order. Concurrent changes to the same field go to the later capped `hlc`, `min(hlc, (server_received_ms + 300000) << 16 | 0xFFFF)`, so a fast clock's operation never ranks later than 5 minutes after the server received it, and the losing value stays in history with a Notification. A stale offline edit therefore never silently overwrites newer work.

A pending operation has no `server_received_ms` yet. On its own Replica it applies after every acknowledged operation, so the local edit shows until sync. When the server acknowledges it, the conflict is decided again by the rule above; if the pending edit loses, the Replica takes the server's result and sends a Notification.

A merge that leaves an Issue breaking its Workflow is applied and flagged by `tm doctor`, not rejected; rejection is reserved for permissions and purged Issues.

## Verifying agreement

Synced operations form a linear hash chain in `server_seq` order, one chain per Project starting from a fixed genesis value:

- `op_hash` is the operation's identity, computed when it is created over a versioned, fixed binary envelope of every field the Replica writes: `v`, `op_id`, `replica_id`, Actor, Project, Issue UUID, `kind`, `hlc`, `base_seq`, `utc_ms`, `offset_min` and the payload bytes stored as written.
- `chain_hash[n] = H(prefix ‖ chain_hash[n−1] ‖ server_seq ‖ op_hash ‖ server_effects)`, computed by the server, where `server_effects` holds `server_received_ms` and any Key it assigned. Between the two hashes, every input to a merge is covered.
- SHA-256 with distinct one-byte prefixes for the two hash kinds.
- `tm sync status` reports **synced** (nothing pending; same cursor and head), **pending**, **behind** (for example a server restored from backup) or **forked** (histories differ at a named `server_seq`).
- Compaction keeps `(server_seq, op_id, op_hash, server_effects, chain_hash)` for every operation and collapses only superseded payloads, so every chain link can still be recomputed.

A chain shows a Replica that the server did not rewrite history it already saw; it proves nothing to a third party, which would need signed heads and is out of scope.

## Considered Options

- **Last to sync wins (`server_seq` alone)**: immune to clock skew, but a week-old offline edit overwrites today's.
- **Latest clock wins (`hlc` alone)**: a fast clock wins every conflict within the skew bound.
- **Hash over the tables instead of a chain**: cannot tell a server that is behind from a fork, or say where histories differ, and costs a table scan per check; kept only as a secondary check for apply bugs.
- **Merkle proofs or a parent-linked history**: proofs serve Replicas that do not download the log, and with one server ordering everything each parent is just the previous operation.

## Consequences

- TypeScript code that creates operations itself must produce byte-identical `op_hash` values; routing its writes through the Rust server avoids this.
- SHA-256 stays fast on aarch64 only with `sha2` 0.11 or later, which detects the hardware instructions at run time (0.10 needs its `asm` feature).
- Moving an Issue between Projects spans two chains and is not yet designed.

## Prior art

The split between an operation's identity and its position, and between stored and derived data, follows Mononoke's Bonsai changesets and derived data ([facebook/sapling](https://github.com/facebook/sapling)). Domain-separated hashing follows RFC 9162; the behind-versus-forked distinction follows Hypercore; rebuilding tables from the log follows Fossil. Anthropic's Scion applies the Bonsai model, by its own account.
