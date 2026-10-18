# Claims, Links and the store invariants

- Date: 2026-10-17
- Author: Claude (research pass for Jace Babin)
- Scope: What bead tm-u8g.4 ("Enforce the store invariants") must build and test: take-only-if-unclaimed, deterministic Link keys, Keys never reused after purge and unique operation IDs, plus ADR 0006's op_hash, clock and immutability rules; and the operation shapes for `claim` and `link`.

Method note: sources were read on 2026-10-17. beads was cloned from `gastownhall/beads` at `4a67d0d0288c1b4ee23e17fc896f47616931ba2d` (2026-10-17) and is cited by path. Linear's GraphQL schema was fetched from `linear/linear` master at `9dd2ed80f93ae596f4f7fa21aa7cf80895c58514` (2026-10-15). Atlassian, GitHub and SQLite pages were read through WebFetch, which returns summaries, so their quotes are near-verbatim. The Atlassian link-type page was summarised without the inward description of "Blocks"; anything about Jira beyond the three link-type properties is **inferred**. Measurements ran on macOS 27.0.1 on an Apple M2 Max with Rust 1.99.1. The race probe uses `rusqlite` 0.40.2 with its bundled SQLite (`libsqlite3-sys` 0.38.2), the same versions as `tm-store`. The SQL probes ran on the system `sqlite3` 3.54.0. Nothing was run on Linux. Test-suite runs and mutations used a copy of the repo in a `mktemp -d` directory, never the working tree. Anything not quoted or measured is labeled **inferred**.

Scripts:

- [scripts/claim-race/](scripts/claim-race/): 50 processes racing to take Issues. Run `scripts/claim-race/run.sh <scratch-dir>`, which builds into `<scratch-dir>/target`.
- [scripts/server-effects-rewrite.sh](scripts/server-effects-rewrite.sh): checks whether server effects on a stored operation can be overwritten, using the real `LOG` DDL from `schema.rs`. Run `server-effects-rewrite.sh <repo> <scratch-dir>`.
- [scripts/link-cycle.sql](scripts/link-cycle.sql): the cost of a write-time blocking-cycle check at 10,000 Issues. Run `sqlite3 <scratch.db> < link-cycle.sql`.

## Question

1. What is a Claim in tm, how do comparable trackers and queues make "claim the next ready task" atomic, what must "a take succeeds only on an unclaimed Issue" mean locally (50 processes racing, exactly one winner) and under sync (a pulled operation is always applied), and what shape should the `claim` operation and store API have?
2. What shape should the `link` operation have, what makes a Link key deterministic (direction, inverse names, symmetric types), how are blocking cycles refused, what does the projection look like, and how much of that belongs in this bead?
3. What does purge mean for an append-only, hash-verified log, and what is the minimum test that a Key is never reused, given tm-u8g.5 builds Key assignment on the same branch?
4. For each invariant: is it enforced and tested today, what is the smallest change that enforces it, and which test fails when it breaks?
5. What in design.md, the ADRs or the glossary conflicts or leaves a decision open?

## Sources

Repo:

- [design.md](../../specs/design.md): "Architecture" (Invariants), "Links and gates", "Deletion", "Agents" (Identity, Claims)
- [GLOSSARY.md](../../../GLOSSARY.md): Claim, Agent, Actor, Link
- [ADR 0004](../../decisions/0004-one-authority-assigns-keys.md), [ADR 0005](../../decisions/0005-operation-log-is-the-source-of-truth.md), [ADR 0006](../../decisions/0006-operation-merge-ordering-and-versioning.md), [ADR 0007](../../decisions/0007-the-projection-does-not-depend-on-the-scheme.md)
- [projection-schema.md](../store/projection-schema.md), [scheme-and-workflow.md](../fields/scheme-and-workflow.md), [agent-first-trackers.md](../task-tracker-landscape/agent-first-trackers.md), [build-plan.md](../../contributing/build-plan.md)
- Code at `c3e2db2`: `crates/tm-core/src/op.rs`, `crates/tm-core/src/hlc.rs`, `crates/tm-core/src/scheme.rs`, `crates/tm-store/src/{lib,apply,validate,schema}.rs`, `crates/tm-store/tests/it/*.rs`

beads @ `4a67d0d0288c1b4ee23e17fc896f47616931ba2d` (<https://github.com/gastownhall/beads/tree/4a67d0d0288c1b4ee23e17fc896f47616931ba2d>):

- `issueops/claimer.go` (the Claimer contract), `internal/storage/issueops/claim.go` (the CAS), `internal/storage/issueops/claim_next.go`, `internal/storage/issueops/identity.go` (`actorMatches`)
- `internal/storage/issueops/lease.go` (`DefaultLeaseTTL`, `row_lock`), `issueops/leasereclaimer.go`
- `internal/types/types.go` lines 1443–1473 (dependency types), `internal/storage/issueops/dependencies.go` (`CheckDependencyCycleInTx`)
- `internal/storage/depid/depid.go` and `internal/storage/schema/migrations/0050_dependencies_deterministic_id.up.sql` (deterministic edge ids, beads#4259)

Other trackers and SQLite:

- Linear GraphQL schema, `packages/sdk/src/schema.graphql` @ `9dd2ed80f93ae596f4f7fa21aa7cf80895c58514`: `enum IssueRelationType` (line 21126), `type IssueRelation`, `Issue.relations` and `Issue.inverseRelations`
- Jira link types: <https://support.atlassian.com/jira-cloud-administration/docs/configure-issue-linking/>
- GitHub issue dependencies REST: <https://docs.github.com/en/rest/issues/issue-dependencies>; sub-issues: <https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/adding-sub-issues>
- SQLite transactions: <https://www.sqlite.org/lang_transaction.html>; RETURNING: <https://www.sqlite.org/lang_returning.html>

## Findings

### 1. Claims

**What tm says a Claim is.** The glossary defines a Claim as "an Agent's exclusive, expiring hold on an Issue while it works on it" and lists lock, lease and checkout as words to avoid. design.md ("Claims") adds:

- `tm start` claims the Issue for the Agent and keeps the human as Assignee.
- A Claim protects status, description, Checks and the Agent field. Another Actor is refused "with the claimant and expiry, unless it passes `--force`, which is recorded and notifies the claimant". Comments, links, Findings and fields such as priority stay open to anyone.
- A Claim expires after 2 hours by default, and any write renews it, as does `tm heartbeat`. `--for` and `--no-expiry` exist. "Expiry clears the Agent, leaves the status alone and sends a Notification."
- `tm handoff` transfers a Claim, and `tm ready --claim` "atomically takes the next ready Issue".
- Under "Identity", Agent writes are recorded as "agent, on behalf of human".

The Architecture invariant reads: "a take succeeds only on an unclaimed Issue, never on 'already has that value'". ADR 0006's merge table says: "Claims: decided by the authority, like Keys; an offline Claim is provisional and its Agent is notified if it loses."

The build plan divides the work. Phase 1 has the invariant and `claim`. Phase 3 has `--force`, protected fields, version-checked writes, expiry and heartbeat, `tm start`, `tm handoff`, `ready --claim` and Path Claims.

**What exists in code.** The `claims` projection table already exists with columns `issue_id PK, agent, actor, op_id, expires_ms` (`schema.rs:170`), but nothing writes to it. The Scheme marks `agent` and `claim` as `derived()` (`scheme.rs:275-276`). That closes off `set_field` as a way to claim twice in code, not only by preference: `validate::settable` refuses derived fields (`validate.rs:205`), and replay's `target()` refuses `"agent" | "claim"` as "computed and cannot be set" (`apply.rs:258`).

**How beads claims.** The `Claimer` contract (`issueops/claimer.go`) is "one atomic compare-and-set mutation". It sets the assignee to the actor and the status to `in_progress` "only while the issue's status is … open … and the issue is unassigned, assigned to Actor, or assigned to a configured claim pool". It adds: "StatusInProgress held by the same Actor is an idempotent success with Changed false." A foreign holder gets `ErrAlreadyClaimed`. The refusal is a typed `ClaimConflictError` that carries the assignee and status "read inside the attempt that lost" (`claim.go` at the repo root explains why the older message-parsing shim remains).

The CAS is a conditional `UPDATE … WHERE id = ? AND row_lock = ? AND status IN (…)`. Every ownership-changing path rewrites the `row_lock` cell with a random value, because "Dolt has no real row locking and merges concurrent commits cell-by-cell" (`internal/storage/issueops/lease.go`). beads' leases are a separate, uncommitted table with `DefaultLeaseTTL = 5 * time.Minute`; heartbeats extend them, and `bd reclaim` reverts stale ones. Same-actor matching goes through `actorMatches(a, b)`, which canonicalises spellings (`identity.go:82`). `bd ready --claim` runs `ClaimReadyIssueInTx` inside one transaction (`claim_next.go`).

**What Agents see.** In tm, `tm prime` prints "the caller's Claims" (design.md "Interface"), and a refused take should come back as a structured `--json` error carrying the holder and expiry, in the style of design.md's `missing_fields` example. Because two sessions can share an Agent name, that error is how a session learns that the holder carrying its own name is a different session.

**How others do it.** Linear and GitHub have no claim. Ownership there is an assignee field that the last writer sets (**inferred** from the absence of any claim API in Linear's schema and in GitHub's issue docs; not exhaustively checked). SQLite job queues use either a single conditional statement or a write transaction. SQLite supports `RETURNING` "since version 3.35.0 (2021-03-12)", so `UPDATE … WHERE claimed IS NULL RETURNING id` returns no row when it loses. The docs caution that "a RETURNING clause for an UPSERT reports both inserted and updated rows" (lang_returning).

The read-then-write trap in SQLite is documented: "If some other database connection has already modified the database … upgrading to a write transaction is not possible and the write statement will fail with SQLITE_BUSY". By contrast, "IMMEDIATE causes the database connection to start a new write immediately" (lang_transaction).

**Measured: 50 processes racing to take one Issue.** [claim-race](scripts/claim-race/) runs 20 runs per variant. Each child opens its connection, waits for a start file, then takes the Issue. Outcomes are summed over 1,000 process attempts. The rightmost column counts runs in which more than one process believed it held the Issue.

| Variant | Outcomes (of 1,000) | Runs with >1 holder |
|---|---|---|
| Check outside any transaction, then `BEGIN IMMEDIATE` write | WON 842, HELD 158 | **20 of 20** |
| Same, with 1 ms of work between check and write | WON 988, HELD 12 | **20 of 20** |
| `BEGIN DEFERRED`, check and write inside | WON 20, HELD 425, **BUSY 555** | 0 |
| `BEGIN IMMEDIATE`, check and write inside, 5 s busy timeout | WON 20, HELD 980 | 0 |
| Same, with 1 ms of work | WON 20, HELD 980 | 0 |
| Conditional `INSERT … ON CONFLICT DO NOTHING RETURNING` first, then log | WON 20, HELD 980 | 0 |
| `BEGIN IMMEDIATE`, strict rule, all 50 use Agent name `claude` | WON 20, HELD 980 | 0 |
| `BEGIN IMMEDIATE`, beads' idempotent rule, all 50 use `claude` | WON 20, **MINE 980** | **20 of 20** |
| Take next of 10 ready Issues, check outside transaction | WON 1,000 | **20 of 20** (each run: 50 claim ops logged, all on Issue 1) |
| Take next of 10 ready Issues, `BEGIN IMMEDIATE` | WON 200, NONE 800 | 0 (each run: 10 ops, 10 distinct Issues) |

What this shows:

- **The check has to sit inside the `BEGIN IMMEDIATE` transaction that writes.** `Store::append_all` already opens one (`lib.rs`), so the claim check belongs in `validate::check`, which runs inside it. That is the existing write path, not a new mechanism.
- **`BEGIN DEFERRED` is safe but unusable.** 55% of attempts failed with BUSY immediately: the slowest run took 9 ms, so the 5 s busy timeout was not waited out. Every agent would have to retry. SQLite's [busy handler documentation](https://www.sqlite.org/c3ref/busy_handler.html) states that when invoking the handler could deadlock, "it will go ahead and return SQLITE_BUSY to the application instead of invoking the busy handler"; the 4–9 ms timings are consistent with that.
- **"Never on 'already has that value'" is the rule that separates tm from beads, and it matters.** When 50 sessions share one Agent name, beads' idempotent rule told all 50 they held the Issue in every run. tm's strict rule told exactly one. The design's own identity examples (`CLAUDECODE=1`, `AI_AGENT`) make shared names likely: two Claude Code sessions both identify as Claude unless `TM_ACTOR` differs.
- **Racy logs can be repaired on replay if the take records what it saw.** In the check-outside-transaction runs, the log held 50 claim operations for one Issue, and the live projection ended with the *last* writer (`INSERT OR REPLACE`). The probe also replays each log, keeping a take only when the Claim it recorded as current (`prev_claim`) is still current. That replay kept exactly 1 take in every run, both for the one-Issue race and for the take-next race. Recording the previous Claim therefore makes the outcome a pure function of log order. This is what sync needs (below).

**Under sync.** ADR 0006 says a pulled operation is always applied and that Claims are decided by the authority. Taken together, a remote take that loses cannot be refused or dropped: it stays in the log, and applying it must decide that it lost. Two ways to decide:

- **Per-field last writer wins** (how `set_field` merges, `apply.rs` `Version::beats`). Concurrent takes would go to the later capped clock. That contradicts "decided by the authority", and it lets a later take silently replace an earlier holder's Claim.
- **Compare-and-set on the previous Claim** (recommended). Each take records `prev`, the `op_id` of the Claim it saw as current, or none. Applying it in canonical order (acknowledged by `server_seq`, then pending by `hlc`; `apply.rs` header) wins only if `claims.op_id` still equals `prev`. Two offline takes on an unclaimed Issue both record `prev = none`. The server orders them, the first applies and the second finds a head that is no longer none, so it loses. That is "decided by the authority" with no new server logic, because server order is the authority. A pending local take that the server places second loses on the next rebuild, which is the provisional Claim whose Agent is notified.

  Within one Replica, expiry does not need wall-clock time at replay: a writer that saw an expired Claim records that Claim as `prev`, and replay only compares ids. Across Replicas this is not settled (see Open questions): renewal by an ordinary write does not change the Claim's `op_id`, so a take made offline against a Claim that looked expired can win even though the holder renewed it. A forced steal is a take whose `prev` is the current holder, with a `force` flag, so it is recorded as ADR 0006 and design.md require. This mirrors beads' `row_lock` CAS, and the design's "every write carries the version it was based on", applied to the Claim.

**Expiry and renewal.** projection-schema.md already says "a rebuild must not expire Claims itself". Store `ttl_ms` (or none for `--no-expiry`) in the claim payload. The projection keeps `expires_ms = renewed_ms + ttl_ms`, where `renewed_ms` is the latest `utc_ms` of an operation by the holder on that Issue (any write renews), and readers compare it with now. All inputs come from the log, so rebuilds stay deterministic. Expiry is then a reading, not an operation. The "Claim expiry releases the Claim" automation rule, if switched on, appends an ordinary release. This is Phase 3 work; Phase 1 only needs the payload field to exist so that later operations do not need a new format version (**inferred** design).

### 2. Links

**How trackers model link types.**

| Tracker | Storage | Direction and names |
|---|---|---|
| Jira | `issuelink(LINKTYPE, SOURCE, DESTINATION, SEQUENCE)` and `issuelinktype(LINKNAME, INWARD, OUTWARD)` (cited in projection-schema.md) | Each type has "Name", an "Outward description" ("how a work item affects other work items") and an "Inward description" ("is affected by"). Example: causes / is caused by. Stored once and read from both ends. |
| Linear | `IssueRelation { issue, relatedIssue, type }`. `IssueRelationType` is `blocks, duplicate, related, similar` | One stored direction. "Blocked by" is not a type; it is the same row read through `Issue.inverseRelations`. |
| GitHub | Dependencies: `…/dependencies/blocked_by` (GET, POST, DELETE) and `…/dependencies/blocking` (GET). Sub-issues: "up to 100 sub-issues per parent", "up to eight levels" | One relationship with two named endpoints. Sub-issues are parent and child, separate from dependencies, as in tm. |
| beads | `dependencies(issue_id, depends_on_id, type …)`. Types include `blocks`, `conditional-blocks`, `waits-for`, `related`, `relates-to`, `discovered-from`, `duplicates`, `supersedes`, `caused-by` and others (`types.go:1443-1473`) | Stored as "issue depends on target". The edge id is deterministic: `uuid.NewSHA1(Namespace, issueID + "\x1f" + target)` (`depid.go`), and `type` is "deliberately NOT part of the identity". |

beads' history is the case for deterministic keys. Its edge ids were once a database `DEFAULT (UUID())`, which "mints a *different* random value on every clone". Two clones that created the same edge then made `bd dolt pull` break "unrecoverably" (beads#4259, `depid.go` and migration 0050). tm faces the same failure through `field_versions`: its primary key is `(issue_id, field, elem)`, and per-element add and remove merge only if every Replica writes the same Link onto the same tuple.

**What makes a key deterministic in tm.** "A blocks B" written on A and "B is blocked by A" written on B are one Link. If each operation were filed under the Issue it was written on, they would occupy two `field_versions` rows and their adds and removes would never meet. So the key must be canonical:

- **Directional types** (blocks, duplicates, derived from, causes, supersedes) are stored only in the outward direction: `(type, source, target)`. An inward name ("is blocked by") is a way to read the Link, never a separate type. This is how Linear and Jira work.
- **Symmetric types** (relates to) have no direction. Order the two UUIDs, so `source = min(A, B)` and `target = max(A, B)` by bytes.
- **The type is part of the identity.** "A blocks B" and "A relates to B" are different Links. beads chose otherwise because its unique key had no type column; tm has no such constraint.
- **No hash is needed.** The canonical triple is already deterministic and readable. projection-schema.md proposed `link_key = hash(type, source, target)`, but a hash buys only a fixed width, which matters to beads' CHAR(36) column and not to SQLite's `WITHOUT ROWID` composite keys.

**Operation shape.** ADR 0006 lists a `link` kind, and Links are a set ("add and remove per element; the later operation for an element wins"). There are two candidates:

- `add` and `remove` on a `links` field. This reuses existing variants, but `validate::member` accepts only label and multi-select Scheme fields, and `apply::set_member` writes to `set_members`. Links need different validation (target exists, not self, cycle check, canonical direction) and their own table, so every branch would special-case `links`.
- **A `link` kind** (recommended), for example `Change::Link { link_type, target, present: bool }` (or an `action` of add or remove). The operation is filed under the canonical source Issue and that Issue's Project. `field()` returns `Some("links")` and `elem()` returns `Some("<type>/<target uuid>")`, so `ops_target` and `field_versions` line up with no schema change, and set-element merge reuses `wins` and `record` unchanged. One kind with a `present` flag keeps ADR 0006's list as written.

**Projection.** `links(type TEXT, src BLOB, dst BLOB, PRIMARY KEY (type, src, dst)) WITHOUT ROWID`, plus an index on `(dst, type, src)` for reading inward names and for "what this would unblock". Present Links only; removal history stays in `field_versions` and the log.

**Measured: cycle refusal at write time.** [link-cycle.sql](scripts/link-cycle.sql) has 10,000 Issues, 29,968 `blocks` Links (an acyclic graph in which Issue 1 reaches nearly everything) and 5,000 `relates_to` Links. "X blocks Y" closes a cycle when Y already reaches X.

| Check | Result | Time |
|---|---|---|
| Full walk from Issue 2, written `links JOIN reach` (planner's choice) | 9,999 reached | **10.78 s** |
| Same walk, written `reach CROSS JOIN links` | 9,999 reached | **0.013 s** |
| Refused: "10000 blocks 1" | cycle | 0.28 ms |
| Refused: "5000 blocks 4990" | cycle | 0.14 ms |
| Allowed: "1 blocks 10000" | no cycle | 0.03 ms |
| Allowed, worst case: "10001 blocks 1" (walks all of 1's reach) | no cycle | 12.9 ms |

Without `ANALYZE` statistics, the planner put `links` on the outside and scanned every `blocks` row per recursive step (`EXPLAIN QUERY PLAN`: `SEARCH l USING PRIMARY KEY (type=?)` then `SCAN r`). `CROSS JOIN` fixes the order (`SCAN r`, then `SEARCH l USING PRIMARY KEY (type=? AND src=?)`). At this scale a write-time check costs at most about 13 ms. beads does the same check with a "recursive CTE across both dependency tables" before insert (`CheckDependencyCycleInTx` in `dependencies.go`).

**Cycles from merges.** design.md says Workflow rules are "checked when this Replica writes, never on replay: a merge can produce a state no single write allowed, and it is kept". Two Replicas can each add half of a cycle offline. Replay must therefore apply both, and the ready queue and `tm doctor` must cope with a cycle rather than assume none exists (**inferred**; not measured).

### 3. Keys never reused, and purge

- **The log already keeps what reuse prevention needs.** `ops.assigned_key` sits outside `op_hash` as a server effect (ADR 0006 `server_effects`). The `keys` projection table "outlive[s] purged Issues, so [it carries] no foreign key" (`schema.rs:162`). If the local authority writes the Key into `assigned_key` on the `create` operation (projection-schema.md, Implications), the next number is the highest ever assigned in the Project's log, and nothing that removes an Issue from the projection can lower it.
- **Measured gap: server effects can be rewritten.** The `ops_immutable` trigger exempts `server_seq`, `server_received_ms` and `assigned_key` so the server can fill them in. With the real `LOG` DDL ([server-effects-rewrite.sh](scripts/server-effects-rewrite.sh)), the first assignment succeeded (changes=1), and so did a second one from `CPL-1` to `CPL-2` and a clear back to NULL. `guards::stored_operations_cannot_be_edited_or_deleted` asserts only that the first `UPDATE` succeeds. A Key could therefore be overwritten or cleared, which breaks both "never reused" and "a stored operation is never rewritten"; `chain_hash` covers `server_effects`. A set-once trigger closes the gap: with it, the first assignment succeeded and `CPL-2`, NULL and a changed `server_seq` were refused with "server effects are set once", while re-writing the same value was allowed.
- **What purge can mean.** ADR 0006 makes purge final ("later operations on a purged Issue are rejected visibly"), and design.md calls it "the only permanent removal". The log, though, is append-only (trigger `ops_append_only`), immutable, and rebuild refuses any operation whose stored hash does not match its content (`guards::rebuild_names_an_operation_whose_content_no_longer_matches_its_hash`). The rule that fits ADR 0006's own compaction rule ("keeps `(server_seq, op_id, op_hash, server_effects, chain_hash)` … collapses only superseded payloads") is: purge appends a purge operation, then *redacts* the payloads of the Issue's operations and keeps each row's envelope fields, `op_hash` and `assigned_key`.

  That requires three changes: a sanctioned way past `ops_immutable` for `payload` alone, a redacted marker, and rebuild skipping the content hash check for redacted rows while still trusting the stored `op_hash` for the chain. None of this exists yet, and it is a decision for a superseding ADR or an amendment, not for this bead. Whatever form it takes, it must keep `assigned_key`, which is all Key reuse prevention depends on.
- **Minimum test now.** Purge does not exist, and tm-u8g.5 assigns Keys on this branch, so the test belongs with tm-u8g.5, written against the invariant rather than the purge command. Create CPL-1 to CPL-3. Remove CPL-3 from the projection the way any purge would have to (delete its `issues`, `keys` and dependent rows), then archive CPL-2 and rebuild. The next Issue must still be CPL-4. That test fails if numbering is computed as `count(issues) + 1` or `max(keys.number)` over live Issues. Pair it with the set-once trigger test above.

### 4. The invariants, one by one

Baseline: `cargo test -p tm-core -p tm-store` printed `66 passed` (tm-core unit tests) and `71 passed` (tm-store integration tests). Each mutation below was made in a scratch copy, and the named test was observed failing or not.

| Invariant | Enforced today? | Tested today? (mutation run) | Smallest change | Test that fails when broken |
|---|---|---|---|---|
| Every operation has a unique ID | Yes: `op_id BLOB NOT NULL UNIQUE` (`schema.rs:19`), `Uuid::now_v7()` per operation (`lib.rs:409`) | **No.** Dropping `UNIQUE`: all 71 store tests still passed | None in code | Insert a stored operation again with the same `op_id` and different content (as sync would) and assert the insert is refused and the log is unchanged. Later, sync needs "same `op_id` and same `op_hash` is a no-op; same `op_id` with a different hash is corruption". |
| `op_hash` covers every field a Replica writes | Yes (`Op::envelope`) | Yes: `op_hash_covers_every_written_field`, `length_prefixes_keep_adjacent_fields_apart`, `op_hash_has_a_fixed_value` | None | Already present. New `claim` and `link` payloads are covered automatically, since the payload is hashed as bytes. |
| `op_hash` is over the bytes as stored | Yes: `payload` is a `String` "never re-serialized" (`op.rs`); rebuild re-checks it (`lib.rs` `replay`) | Partly: `the_stored_hash_matches_the_operation` and the guard test catch a changed payload. No test feeds a payload whose JSON is not in serde's canonical form | None | A remote operation whose payload has reordered keys or extra whitespace still rebuilds. That fails if anyone hashes `serde_json::to_string(&change)` instead of the stored text. |
| Clock never moves backwards | Yes (`Hlc::tick`, `observe`; `last_hlc` in `meta`) | Yes: proptests `tick_never_decreases` and `observe_never_decreases`, `the_clock_never_moves_backwards_across_reopens`, three clock-recovery guard tests | None | Already present |
| Counter rolls at 0xFFFF | Yes (`Self(self.0 + 1)` carries into the millisecond) | Yes. Mutating to wrap within the millisecond failed `full_counter_rolls_into_next_millisecond`. The proptest did not catch it (random inputs rarely hit 0xFFFF) | None | Already present |
| A stored operation is never rewritten | Yes for content (`ops_immutable`, `ops_position_frozen`, `ops_append_only`, `recursive_triggers`) | Yes for content. Disabling the triggers failed `guards::stored_operations_cannot_be_edited_or_deleted`. **No for server effects**: measured above, they can be overwritten and cleared | A set-once trigger on `server_seq`, `server_received_ms` and `assigned_key` | Assign each server effect, then assert that changing it and clearing it both fail while the first assignment succeeds |
| A take succeeds only on an unclaimed Issue | No: no `claim` operation exists | No | `claim` kind, check inside `append_all`'s `BEGIN IMMEDIATE`, compare-and-set on `prev` in `apply` | (a) 50 processes, as in `concurrency.rs`, take one Issue: exactly one `Ok` and 49 `AlreadyClaimed` naming the winner (the probe's check-outside-transaction variant failed this in 20 of 20 runs). (b) The same with one shared Agent name (the idempotent rule failed it in 20 of 20). (c) Two remote takes with `prev = none` acknowledged at `server_seq` 1 and 2: after rebuild the first holds the Claim and the second is recorded as lost. (d) The rebuild property test with claim actions added. |
| Links have deterministic keys | No: no `link` operation or `links` table | No | `link` kind with canonical direction, `links` projection | "A blocks B" written on A and "B is blocked by A" written on B produce one `field_versions` row and one `links` row; "A relates to B" and "B relates to A" likewise. An add on one Replica and a later remove on another leave no Link after rebuild. Fails if the key includes the Issue the Link was written on. |
| Blocking Links never form a cycle on write | No | No | Recursive check with pinned join order in `validate::check` for `blocks` | Adding the closing Link of a 3-cycle is refused, naming the path. A cycle produced only by merging two Replicas is kept by rebuild, not refused. |
| A Key is never reused, even after purge | Partly: `keys` has no foreign key; numbering does not exist yet | No | tm-u8g.5 numbers from the log's highest `assigned_key`, plus the set-once trigger | Section 3's test |

### 5. Store API shape (recommendation)

- **`Store::take(actor, issue, TakeOptions { agent, on_behalf_of, ttl, force }) -> Result<Claim, StoreError>`**, and `Store::take_next(filter, …)` for `ready --claim`. Each opens one `append_all` transaction: read the current Claim, decide, append a `claim` operation recording `prev`, apply.
- **`StoreError::AlreadyClaimed { issue, holder: Agent, on_behalf_of, since_ms, expires_ms: Option<i64>, claim: Uuid }`**. It names the holder even when the holder has the caller's own Agent name, matching design.md's "refused with the claimant and expiry" and its structured-error style. It reports everything beads' `ClaimConflictError` does, typed rather than string-parsed.
- **`Store::release(actor, issue, claim: Uuid)`**, a `claim` operation with an action of release and `prev = claim`. Heartbeat and renewal come in Phase 3.
- **`Change::Claim { action: take | release, agent, on_behalf_of, ttl_ms: Option<u64>, prev: Option<Uuid>, force: bool }`**, with kind `"claim"`, `field() = Some("claim")` and `elem() = Some("")`. It is applied by compare-and-set on `prev`, not by `Version::beats`.
- **`Change::Link { link_type, target, present }`**, with kind `"link"`, filed under the canonical source. Link types are a fixed built-in list in Phase 1: `blocks`, `relates_to`, `duplicates`, `derived_from`, `causes`, `supersedes`, as Scheme-style ids per ADR 0007. Each has outward and inward display names and a symmetric flag.

## Conclusions

- **The take invariant is a transaction-placement rule plus a replay rule.** Locally, the check must run inside the same `BEGIN IMMEDIATE` transaction as the append. Measured, that gave exactly one winner in 20 of 20 runs, while a check outside it gave 20 of 20 runs with two or more winners. Under sync, recording the previous Claim and applying takes by compare-and-set makes server order the authority, as ADR 0006 requires, and kept exactly one take when replaying the racy logs.
- **The `claim` operation should be its own kind, not `set_field`.** `set_field` is last writer wins, which ADR 0006 rules out for Claims, and the code already refuses it on the derived `agent` and `claim` fields.
- **tm's strict rule is deliberately stricter than beads'.** With one shared Agent name, beads' idempotent re-claim let 50 of 50 sessions believe they held the Issue. `AlreadyClaimed` must name the holder even when the name matches the caller's.
- **Link keys must be canonical across both ends and both Replicas.** Store only the outward direction, order symmetric pairs, include the type, and file the operation under the canonical source. The `(issue_id, field, elem)` key of `field_versions` then handles the merge with no change.
- **Most of ADR 0006's invariants are enforced and tested.** Two gaps remain: no test protects `op_id` uniqueness (removing it broke nothing), and server effects, including `assigned_key`, can be overwritten.

## Implications / actions

Recommended scope for tm-u8g.4. These are recommendations, not decisions.

- Add `Change::Claim` and `Change::Link`, extending `kind_names_match_the_serialized_tag` and the `field()` and `elem()` accessors.
- Take: check in `validate::check`, apply by compare-and-set on `prev`, `StoreError::AlreadyClaimed`, the 50-process test (distinct names and one shared name), and the two-remote-takes replay test.
- Links: canonicalisation, the `links` table, set-element merge through `field_versions`, the write-time `blocks` cycle check with `reach CROSS JOIN links`, and the determinism tests. Bump `PROJECTION_VERSION`.
- Add a set-once trigger on server effects and its test; add a duplicate-`op_id` test and a non-canonical-payload rebuild test.
- Add claim and link actions to the rebuild property test (`rebuild.rs`).
- Leave for later beads: expiry, renewal and heartbeat, `--force` notifications, protected fields, handoff and Path Claims (Phase 3); readiness behaviour per link type, gates, the ready queue and "what this would unblock" (tm-u8g.8 and Phase 4); custom link types (Phase 4); the purge command and redaction design (needs an ADR); Key numbering and the Key-reuse test (tm-u8g.5).
- ADR work, for the user to decide: an amendment or a superseding ADR that settles the purge kind and redaction, how a lost Claim is recorded, and where "on behalf of" lives (Open questions).

## Open questions

- **"On behalf of" is not in the operation envelope.** design.md records Agent writes as "agent, on behalf of human", but `Op` has one `actor` field, and `op_hash` covers a fixed envelope (`ENVELOPE_FORMAT = 1`). Adding the field later means a new envelope format. Putting it in the claim payload covers Claims only. This should be decided before operations exist outside development stores.
- **How is a lost take recorded?** The `unapplied` table holds operations that "do not fit". A take that lost the compare-and-set is well formed and expected under sync. A separate outcome, or a `lost` reason in `unapplied`, both work, and the choice decides what drives the "Agent is notified if it loses" Notification.
- **ADR 0006's kind list against its merge table.** It lists an `archive` kind but calls archive "an ordinary field", and the code uses `set_field archived`. It has a Purge merge row but no `purge` kind. projection-schema.md raised both points.
- **Purge against an append-only, hash-verified log** (section 3): payload redaction needs a hole in `ops_immutable` and a rebuild exception for redacted rows. Not designed.
- **Cross-Project Links.** An operation is filed under one Project's chain, so the target's Project chain never contains a Link that blocks its Issue. ADR 0006 already lists moving Issues between Projects as undesigned; Links across Projects have the same problem.
- **Link to a Finding.** design.md has a deferred Finding create an Issue "linked as *derived from*" the Finding, but the glossary defines a Link as "a typed relationship between two Issues", and a Finding is not an Issue. Either Links gain non-Issue targets or the Finding reference is a field.
- **Who may hold a Claim.** The glossary says a Claim is an Agent's hold. Whether a human running `tm ready --claim` without an Agent identity takes a Claim, or is refused, is unstated.
- **Identifying a session within one Agent name.** The strict rule stops two sessions named `claude` from both taking an Issue, but Phase 3's protected fields need to tell the holding session from another session with the same name. The Claim's `op_id` as a token, or `CLAUDE_CODE_SESSION_ID`, are candidates (**inferred**).
- **Expired-Claim takes across Replicas.** Suppose A takes with a 2-hour expiry and renews by writing an hour later. B, offline and unaware of the renewal, judges the Claim expired and takes with `prev` set to A's Claim. Server order is A's take, A's write, B's take, so replay compares ids only and B wins a Claim that was live. Either renewal becomes an operation that moves the Claim's head, or takes against an expired Claim are decided by the authority's clock (`server_received_ms`). This is Phase 3 work, but it decides whether "any write renews" stays implicit.
- **Server effects restored from backup.** The set-once trigger assumes the server assigns `server_seq` and Keys once and for all. ADR 0006's "behind" state, where a server is restored from backup, is where that assumption breaks; recovery from it is undesigned.
- **Expiry default.** design.md says 2 hours; beads uses `DefaultLeaseTTL = 5 * time.Minute` with heartbeats. Not a conflict, but the renewal cadence for `tm heartbeat` follows from it.
- **Linux.** The race was measured on macOS only. The build plan's 50-writer check runs on both, and the take test should too.

## Raw data

`cargo test -p tm-core -p tm-store` (scratch copy, Rust 1.99.1):

```text
test result: ok. 66 passed; 0 failed; ...   (tm-core unit tests)
test result: ok. 71 passed; 0 failed; ...   (tm-store tests/it)
```

Mutations (scratch copy; each restored afterwards):

```text
op_id UNIQUE removed            -> test result: ok. 71 passed; 0 failed
ops_immutable RAISE -> SELECT 1 -> test guards::stored_operations_cannot_be_edited_or_deleted ... FAILED
Hlc::tick wraps within ms       -> test hlc::tests::full_counter_rolls_into_next_millisecond ... FAILED
```

`claim-race/run.sh` (50 processes, 20 runs each):

```text
autocommit rule=strict agents=distinct procs=50 issues=1 gap_us=0 runs=20: outcomes [("HELD", 158), ("WON", 842)]; runs where an Issue was held by >1 process: 20; slowest run 724 ms
autocommit rule=strict agents=distinct procs=50 issues=1 gap_us=1000 runs=20: outcomes [("HELD", 12), ("WON", 988)]; runs where an Issue was held by >1 process: 20; slowest run 395 ms
deferred rule=strict agents=distinct procs=50 issues=1 gap_us=0 runs=20: outcomes [("BUSY", 555), ("HELD", 425), ("WON", 20)]; runs where an Issue was held by >1 process: 0; slowest run 9 ms
immediate rule=strict agents=distinct procs=50 issues=1 gap_us=0 runs=20: outcomes [("HELD", 980), ("WON", 20)]; runs where an Issue was held by >1 process: 0; slowest run 175 ms
immediate rule=strict agents=distinct procs=50 issues=1 gap_us=1000 runs=20: outcomes [("HELD", 980), ("WON", 20)]; runs where an Issue was held by >1 process: 0; slowest run 294 ms
conditional rule=strict agents=distinct procs=50 issues=1 gap_us=0 runs=20: outcomes [("HELD", 980), ("WON", 20)]; runs where an Issue was held by >1 process: 0; slowest run 399 ms
immediate rule=strict agents=same procs=50 issues=1 gap_us=0 runs=20: outcomes [("HELD", 980), ("WON", 20)]; runs where an Issue was held by >1 process: 0; slowest run 235 ms
immediate rule=idempotent agents=same procs=50 issues=1 gap_us=0 runs=20: outcomes [("MINE", 980), ("WON", 20)]; runs where an Issue was held by >1 process: 20; slowest run 305 ms
next-autocommit rule=strict agents=distinct procs=50 issues=10 gap_us=0 runs=20: outcomes [("WON", 1000)]; runs where an Issue was held by >1 process: 20; slowest run 598 ms
next-immediate rule=strict agents=distinct procs=50 issues=10 gap_us=0 runs=20: outcomes [("NONE", 800), ("WON", 200)]; runs where an Issue was held by >1 process: 0; slowest run 402 ms
```

Per-run detail (separate runs of the same binary):

```text
autocommit:       run 0: claim ops logged=50 claims rows=1 Issues held by >1 process=1 takes a prev-matching replay keeps=1
next-autocommit:  run 0: claim ops logged=50 claims rows=1 Issues held by >1 process=1 takes a prev-matching replay keeps=1
immediate:        run 0: claim ops logged=1 claims rows=1 Issues held by >1 process=0 takes a prev-matching replay keeps=1
deferred:         run 0: claim ops logged=1 claims rows=1 Issues held by >1 process=0 takes a prev-matching replay keeps=1
```

`server-effects-rewrite.sh`:

```text
first assignment (NULL -> value):
  changes=1
second assignment (value -> other value):
  changes=1
clearing (value -> NULL):
  changes=1
  final: server_seq=7 received=99 assigned_key=NULL

with a set-once trigger added:
  first assignment changes=1
  UPDATE ops SET assigned_key = 'CPL-2' -> Error in 3rd command line argument: server effects are set once
ok, changes=0
  UPDATE ops SET assigned_key = NULL -> Error in 3rd command line argument: server effects are set once
ok, changes=0
  UPDATE ops SET server_seq = 7 -> Error in 3rd command line argument: server effects are set once
ok, changes=0
  UPDATE ops SET assigned_key = 'CPL-1' -> ok, changes=1
```

`link-cycle.sql`:

```text
links: 34968 (blocks: 29968)
-- planner-chosen order (links JOIN reach), full walk from Issue 2
reachable from 2: 9999
Run Time: real 10.778092 user 10.763022 sys 0.012717
-- pinned order (reach CROSS JOIN links), full walk from Issue 2
reachable from 2: 9999
Run Time: real 0.013003 user 0.012946 sys 0.000054
-- refused: adding "10000 blocks 1" (1 reaches 10000)
closes a cycle: 1
Run Time: real 0.000279 user 0.000265 sys 0.000015
-- refused: adding "5000 blocks 4990" (4990 reaches 5000)
closes a cycle: 1
Run Time: real 0.000142 user 0.000131 sys 0.000012
-- allowed: adding "1 blocks 10000" (10000 reaches nothing)
closes a cycle: 0
Run Time: real 0.000030 user 0.000022 sys 0.000009
-- allowed, worst case: adding "10001 blocks 1" walks everything 1 reaches
closes a cycle: 0
Run Time: real 0.012851 user 0.012813 sys 0.000040
```

Query plans for the walk:

```text
links JOIN reach:        RECURSIVE STEP: SEARCH l USING PRIMARY KEY (type=?) ; SCAN r
reach CROSS JOIN links:  RECURSIVE STEP: SCAN r ; SEARCH l USING PRIMARY KEY (type=? AND src=?)
```
