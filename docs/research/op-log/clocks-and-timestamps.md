# Clocks and timestamps for the operation log

- Date: 2026-10-17
- Author: research pass for the operation log ([ADR 0005](../../decisions/0005-operation-log-is-the-source-of-truth.md))
- Scope: what time values each operation carries, so that unsynced operations have an order and history can say "10:42 your time / 15:42 theirs".

Method note: sources were fetched on 2026-10-17 (web pages, `gh api` against GitHub, the crates.io API). Measurements ran on this machine (macOS, system zone `America/Chicago`) with git 2.56.0, cargo 1.99.1, jiff 0.2.38, uhlc 0.9.0, uuid 1.28.0 and Homebrew `sqlite3`. Probe code lived in a scratch directory and is reproduced in [Raw data](#raw-data); nothing was added to this repo besides this note. Claims are cited, quote a command and its output, or are labeled **inferred**.

## Question

Already agreed (from [sync-engines.md, "Custom design"](../sync-engine-spike/sync-engines.md#custom-design-op-log-in-sqlite--http-sync)): a hybrid logical clock (HLC) orders operations locally, `server_seq` is authoritative once synced, and fields resolve per-field last-writer-wins. Open:

1. Store a single UTC instant, a wall-clock time plus offset, or both?
2. How to encode the HLC, and what breaks ties?
3. Can the UUIDv7 `op_id` double as the order?
4. What bounds the damage when a machine's clock is wrong?

## Takeaway

Each operation carries three time values with three different jobs, and none can stand in for another:

| Field | Job | Source |
|---|---|---|
| `hlc` (INTEGER, `ms << 16 \| counter`) + `node_id` | order before `server_seq` exists; causal order across machines | in-house HLC, state persisted in SQLite |
| `utc_ms` (INTEGER) + `offset_min` (INTEGER, nullable) | what the Actor's clock read, for history | `jiff::Zoned::now()` |
| `server_seq` (+ `server_received_ms`, both NULL until acked) | authoritative order; independent check on `utc_ms` | sync server |

Store the offset, not an IANA zone name. Write the HLC in-house (about 50 lines) instead of taking a crate, because tm runs as many short-lived processes and every HLC crate examined keeps its state in process memory. Use `jiff` for all time handling. Details and reasons follow; the field list is in [Recommendation](#recommendation).

## Sources

- Kulkarni, Demirbas, Madeppa, Avva, Leone, "Logical Physical Clocks and Consistent Snapshots in Globally Distributed Databases", University at Buffalo tech report 2014-04: <https://cse.buffalo.edu/tech-reports/2014-04.pdf>
- CockroachDB: <https://docs.cockroachlabs.com/docs/stable/architecture/transaction-layer>, <https://docs.cockroachlabs.com/docs/stable/operational-faqs>, `pkg/util/hlc/timestamp.proto` in <https://github.com/cockroachdb/cockroach>
- YugabyteDB source: `src/yb/common/hybrid_time.h`, `src/yb/util/physical_time.cc` in <https://github.com/yugabyte/yugabyte-db>
- MongoDB server parameters: <https://www.mongodb.com/docs/manual/reference/parameters/>
- Actual Budget CRDT clock: `packages/crdt/src/crdt/timestamp.ts` in <https://github.com/actualbudget/actual>
- RFC 9562 (UUIDs): <https://www.rfc-editor.org/rfc/rfc9562.html>
- RFC 3339: <https://www.rfc-editor.org/rfc/rfc3339.html>; RFC 9557 (IXDTF): <https://www.rfc-editor.org/rfc/rfc9557.html>
- git date formats: <https://git-scm.com/docs/git-commit#_date_formats>
- Temporal: `docs/timezone.md` in <https://github.com/tc39/proposal-temporal>; <https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Temporal>; `finished-proposals.md` in <https://github.com/tc39/proposals>
- GitHub REST: <https://docs.github.com/en/rest/using-the-rest-api/getting-started-with-the-rest-api>; Linear SDK schema `packages/sdk/src/schema.graphql` in <https://github.com/linear/linear>; Jira public instance <https://jira.atlassian.com>
- Automerge `rust/automerge/src/transaction/commit.rs`, Loro `crates/loro/src/lib.rs` and `crates/loro-internal/src/change.rs`, Yjs `src/utils/ID.js`
- Crate sources from the crates.io registry: `uhlc` 0.9.0, `hlc-gen` 2.0.0, `uuid` 1.28.0, `jiff` 0.2.38; jiff comparison doc <https://docs.rs/jiff/0.2.38/jiff/_documentation/comparison/index.html>

## Findings

### 1. Hybrid logical clocks

**The paper** (Kulkarni et al. 2014, text extracted from the PDF above on 2026-10-17):

- Problem statement, §3.1: assign each event `l.e` such that "e hb f ⇒ l.e < l.f", space is O(1) integers, bounded, and "|l.e − pt.e| is bounded". The timestamp is the pair `(l, c)` compared lexicographically.
- Guarantees: Theorem 1, "e hb f ⇒ (l.e, c.e) < (l.f, c.f)"; Theorem 2, "l.f ≥ pt.f"; Corollary 1, "|l.f − pt.f| ≤ ε" where ε is the clock-synchronisation uncertainty; Corollary 3, "c.f ≤ N ∗ (ε + 1)"; Corollary 4, under a minimum message delay `d`, "c.f is at most ε/d + 1".
- Encoding, §6: "restricting l to track only the most significant 48 bits of pt", rounding up, which "still gives us microsecond granularity"; "16 bits remain for c and allows it room to grow up to 65536". The 48 bits are the top of a 64-bit NTP timestamp (32-bit seconds, 32-bit fraction), so `l` ticks in 2⁻¹⁶ s (about 15 µs), not Unix milliseconds.
- Clocks far ahead, §4.1: "We simply ignore reception of messages that cause l value to diverge too much from pt", and on extreme errors "we take the physical clock as the authority, and reset l and c values to pt and 0". §4.2 sets a conservative bound ∆ on `l − pt`, "even on the order of seconds". It notes that masking "makes it especially useful for last write wins (LWW) database systems like Cassandra".

**Production use** (fetched 2026-10-17):

| System | Encoding | Far-ahead clock handling | Source |
|---|---|---|---|
| CockroachDB | `wall_time` int64 nanoseconds + `logical` int32 ("effectively bounded by (maximum clock skew)/(minimal ns between events)") | default max offset 500 ms (`--max-offset`); a node out of sync with at least half the cluster by 80% of it "crashes immediately" | `timestamp.proto`, transaction-layer and operational-FAQ docs |
| YugabyteDB | one u64: physical microseconds shifted left 12 bits, `kBitsForLogicalComponent = 12` | `max_clock_skew_usec` default `500 * 1000` | `hybrid_time.h`, `physical_time.cc` (from source) |
| MongoDB cluster time | logical time "used for ordering of operations" | `maxAcceptableLogicalClockDriftSecs`, "Default: 31536000 (1 year)"; "You cannot advance the cluster time to a new value if the new cluster time differs from the current cluster time by more than" it | parameters page (the md rendering prints the default line directly above the description) |
| Actual Budget (local-first budgeting app) | string `<ISO-8601 ms>-<4 hex counter>-<16 char node>`, `MAX_COUNTER = 0xFFFF` | `maxDrift: 5 * 60 * 1000`; throws `ClockDriftError` on send and receive | `timestamp.ts` |
| Eclipse Zenoh, via `uhlc` (README: "currently used in Eclipse zenoh") | NTP64 with the last 4 bits of the fraction as counter (`CSIZE = 4`) | default 500 ms, overridable by `UHLC_MAX_DELTA_MS`; `update_with_timestamp` returns `Err` | uhlc 0.9.0 `src/lib.rs` |

Measured, `uhlc` 0.9.0 with a peer whose clock is two hours ahead (probe in Raw data):

```text
default max delta = 500ms
remote (2h ahead) = 2026-10-17T15:53:49.713225000Z
update rejected: incoming timestamp from 7 exceeding delta 500ms is rejected: 2026-10-17T15:53:49.713225000Z vs. now: 2026-10-17T13:53:49.713225998Z
lenient after accepting 2h-ahead remote, next local ts = 2026-10-17T15:53:49.713225001Z (wall now 2026-10-17T13:53:49.713233Z)
```

The last line shows the cost of accepting a rusher: every later local timestamp sits two hours ahead of the wall clock until real time catches up (Theorem 2 permits this; only ∆ prevents it).

**Tie-break.** `(l, c)` alone can tie across two machines; every implementation above appends a node identity (uhlc `Timestamp { time, id }`, Actual's 16-character node). The tie-breaker must identify the clock, not the Actor: in tm, a human and several Agents on one machine share one clock, and one human uses several machines. The existing design's `actor` column ("device/agent id") conflates the two (**inferred** from the design's own wording); the glossary's Actor means whoever made the change.

### 2. UUIDv7 (RFC 9562)

- §5.7: `unix_ts_ms` is a "48-bit big-endian unsigned number of the Unix Epoch timestamp in milliseconds", followed by 12-bit `rand_a` and 62-bit `rand_b`.
- §6.2 offers three monotonicity methods (a dedicated counter, "monotonic random", or extra clock precision in up to 12 bits) and says "Counter rollovers MUST be handled by the application"; §6.1 says implementations "MAY alter the actual timestamp" when the clock moves backwards.
- §6.12: "treat UUIDs as opaquely as possible" and "avoid this or other parsing unless absolutely necessary".
- `uuid` 1.28.0 source: `now_v7()` guarantees order only for "UUIDs generated through this method by the same process". `ContextV7` "is incremented when the counter wraps within a time interval", so the embedded timestamp can run ahead of the clock.

Measured (same process): `u1=01a14a23-e891-70f3-846d-1541953b1f30 u2>u1=true embedded unix=1792245229.713000000`.

Conclusion: `op_id` must not be the order. Its ordering holds only within one process (tm is a fresh process per command), its random bits make cross-machine ties arbitrary, it never advances on seeing a remote operation (so it records no causality), and RFC 9562 discourages reading the timestamp out. It is still a reasonable identifier: time-prefixed keys keep B-tree inserts local (**inferred**, not measured here). If that locality is not wanted, UUIDv4 does the same job; either way the note should say the embedded time is never read.

### 3. How other systems store event time

**git** stores epoch seconds plus the author's offset. The docs: "It is `<unix-timestamp> <time-zone-offset>` … For example CET … is `+0100`." Measured with two commits, both given local time 10:42, one under `TZ=America/Los_Angeles`, one under `TZ=Asia/Kolkata`:

```text
$ git cat-file -p HEAD~1 | grep author ; git cat-file -p HEAD | grep author
author t <t@e> 1792258920 -0700
author t <t@e> 1792213920 +0530

$ TZ=Europe/London git log --format='%ai | %aI | %at'
2026-10-17 10:42:00 +0530 | 2026-10-17T10:42:00+05:30 | 1792213920
2026-10-17 10:42:00 -0700 | 2026-10-17T10:42:00-07:00 | 1792258920

$ TZ=Europe/London git log --date=local --format='%ad'
Sat Oct 17 06:12:00 2026
Sat Oct 17 18:42:00 2026
```

`%ai` renders the author's own wall clock ("theirs") regardless of the viewer's `TZ`; `--date=local` renders the viewer's ("yours"). A run of `--date=format-local:'%H:%M %Z'` without `TZ` set printed `00:12 CDT` and `12:42 CDT`, the machine's zone. git keeps no zone name, only the offset.

**RFC 3339 / RFC 9557.** RFC 3339 §4.2: offsets are "local time minus UTC", and "numeric offsets represent only time zones that differ from UTC by an integral number of minutes". §4.3 used `-00:00` for "UTC known, local offset unknown". RFC 9557 (April 2024, Standards Track, updates RFC 3339) moves that meaning to `Z`, keeps `+00:00` as "UTC is the preferred reference", and adds a bracketed IANA suffix, e.g. `2022-07-08T00:14:07+01:00[Europe/Paris]`, with a `!` critical flag that obliges an application to act on an offset/zone inconsistency.

**JavaScript Temporal.** Listed in `tc39/proposals` `finished-proposals.md` with "temporal-for-stage-4" meeting notes from 2026-03 and expected publication year 2027 (fetched 2026-10-17). MDN: `Instant` is "a unique point in time … without any time zone or calendar system"; `ZonedDateTime` is "a combination of an instant, a time zone, and a calendar system"; `PlainDateTime` is wall-clock time "without a time zone". Temporal's `timezone.md`:

- Exact → local is 1:1; local → exact can be ambiguous near transitions ("1:30AM happened twice on Sunday, 4 November 2018 in California").
- The offset-versus-zone conflict arises for stored future times: "When date/time data for future times is stored with both the offset and the time zone, and if the time zone definition changes, then it's possible that the new time zone definition may conflict with previously-stored data", resolved by the `offset` option (`use`, `ignore`, `prefer`, `reject`; default `reject`).
- Time zone definitions change, and "occasionally fixes are made to past ranges too".

**Trackers' APIs** (fetched 2026-10-17):

- GitHub REST: "All timestamps return in UTC time, ISO 8601 format: `YYYY-MM-DDTHH:MM:SSZ`." Observed: `gh api 'repos/git/git/commits?per_page=1'` gave `commit.author.date=2026-10-16T14:47:04Z`, so the git author's offset is not exposed in that field; `rust-lang/rust` issue `created_at=2026-10-17T13:37:16Z`.
- Jira (Atlassian's public instance `jira.atlassian.com`, REST v2 search; whether it runs Cloud or Data Center was not checked): `'created': '2026-10-16T15:43:19.000+0000'`, ISO 8601 with a numeric offset, here UTC. Jira Cloud was not sampled.
- Linear SDK schema: `scalar DateTime` "Represents a date and time in ISO 8601 format"; zones live on profiles, not events: `User.timezone` "The local timezone of the user", `Team.timezone` "Defaults to \"America/Los_Angeles\"".

**CRDT libraries** (source read 2026-10-17):

- Automerge `CommitOptions.time`: "The unix timestamp (in seconds) of the commit (purely advisory, not used in conflict resolution)", optional.
- Loro: a `Lamport` (u32) orders changes; `timestamp` is Unix seconds, recorded only after `set_record_timestamp(true)` (default `false`, "not serialized into updates or snapshots"); "Timestamps are forced to be in ascending order".
- Yjs: an item ID is `(client, clock)`, where clock is a per-client "continuous number"; `ID.js` carries no wall-clock field.

The pattern: ordering never uses wall time, wall time is advisory metadata in UTC seconds, and none of the three record an offset.

### 4. Offset or IANA zone name

Measured with jiff 0.2.38:

```text
America/Chicago  Zoned=2026-10-17T10:42:00-05:00[America/Chicago]  offset_secs=-18000
Asia/Kolkata     Zoned=2026-10-17T21:12:00+05:30[Asia/Kolkata]  offset_secs=19800
fixed -05 Zoned=2026-10-17T10:42:00-05:00[-05:00]

== DST fall-back ambiguity, America/New_York 2026-11-01 01:30 ==
ambiguous? true
compatible -> 2026-11-01T01:30:00-04:00[America/New_York]
later      -> 2026-11-01T01:30:00-05:00[America/New_York]
reject     -> Err(... datetime is ambiguous since it falls into a fold between offsets -04 and -05)
utc 2026-11-01T05:30:00Z -> local 01:30 offset_min -240
utc 2026-11-01T06:30:00Z -> local 01:30 offset_min -300
```

- **What each captures.** UTC instant plus offset fully determines the wall clock the Actor saw, including which 01:30 in a DST fold (last two lines). A zone name adds the rules for converting other instants, which history does not need: it only ever converts the stored instant.
- **Ambiguity** appears only when going from local time to an instant. An operation is stamped from the instant, so neither choice is ambiguous for history.
- **Zone name is needed** for future or recurring local times (Temporal's example is a reminder; its conflict rules exist for stored future times). In tm, that is the automation rule trigger "schedule" in [design.md](../../specs/design.md#automation-rules) and any future due-date field. Those are configuration or Issue fields, not operation metadata, and belong in their own type (`PlainDate` or zone-qualified) when designed.
- **Zone names can mislead in history.** Rendering a past instant through a zone name uses today's tzdb, and Temporal's doc notes past ranges are occasionally corrected; the stored offset is what the clock actually showed (**inferred** consequence of that statement).
- **Privacy.** An offset narrows an Actor to a band of zones; an IANA name such as `America/Boise` names a city. git's design records the offset on every commit and so does every public repository; a web search on 2026-10-17 found only forum threads (Lemmy, Mastodon mirrors) discussing commit offsets as a de-anonymising signal, no primary study, so the risk is **inferred**. tm is single-human per machine ([design.md](../../specs/design.md)) and the log is shared only with the Actor's own sync server, so the exposure is small; it grows if logs are ever exported or shared. RFC 9557's `Z` gives a standard way to record "UTC known, offset withheld", which maps to `offset_min = NULL`.

### 5. Rust ecosystem

crates.io API, 2026-10-17 (latest version, release date, licence, all-time / recent downloads):

| Crate | Version | Released | Licence | Downloads (all / recent) | Notes |
|---|---|---|---|---|---|
| `jiff` | 0.2.38 | 2026-10-06 | Unlicense OR MIT | 230M / 87M | repo pushed 2026-10-14, 2,958 stars; pre-1.0 |
| `chrono` | 0.4.45 | 2026-06-04 | MIT OR Apache-2.0 | 889M / 205M | IANA zones via `chrono-tz` (bundled tzdb) |
| `time` | 0.3.55 | 2026-08-01 | MIT OR Apache-2.0 | 982M / 207M | offsets only |
| `uuid` | 1.28.0 | 2026-10-10 | Apache-2.0 OR MIT | 890M / 217M | `v7` feature, `ContextV7` |
| `uhlc` | 0.9.0 | 2026-01-12 | EPL-2.0 OR Apache-2.0 | 6.8M / 2.2M | Zenoh's HLC; 61 stars; in-memory state |
| `hlc-gen` | 2.0.0 | 2025-09-09 | MIT | 17k / 1.7k | 1 star; 42-bit ms since 2024-01-01 + 22-bit counter; `DriftTooLarge` error; in-memory state |
| `hlc` | 0.1.1 | 2015-08-21 | MIT | 5k | no release since 2015-08-21 |

No other HLC crate appeared in a crates.io search for "hybrid logical clock" sorted by downloads; the other hits (`surrealkv`, `magnetar-*`, `asupersync`) are not HLC libraries by name.

- **jiff**: the jiff author's comparison page (written against chrono 0.4.38 and time 0.3.36, so dated) says chrono "only serializes the offset", the `time` crate has "no time zone aware datetime type" and is "fundamentally incapable of doing daylight saving time safe arithmetic", and jiff reads the system tzdb on Unix and embeds `jiff-tzdb` on Windows. Measured: `Zoned::now()` picked up `America/Chicago`, and with `TZ=Asia/Kolkata` reported `Some("Asia/Kolkata") offset_min=330`; RFC 9557 strings round-trip (`2026-10-17T10:42:00-05:00[America/Chicago] -> 2026-10-17T15:42:00Z`).
- **HLC crates**: `uhlc` and `hlc-gen` both hold `last_time` in a `Mutex` inside the process. tm runs one process per command, with many Agents in parallel ([sync-engines.md requirements 1 and 6](../sync-engine-spike/sync-engines.md#requirements-used-as-the-scoring-rubric)), so a crate's state would reset every invocation. The state must be read and written inside the same `BEGIN IMMEDIATE` transaction as the operation insert; SQLite's write lock then serialises local writers. Neither crate exposes a "load state, step, return state" API suited to that (`uhlc` builds from a clock function; read from source). `uhlc`'s 4-bit counter, process-wide environment knob and EPL option are further mismatches.
- **Packed integer**: measured, `(ms << 16) | counter` fits a positive i64 until `6429-10-17T02:45:55.327Z`, and SQLite keeps it an `integer`: `select (1792245229713 << 16) | 7` → `117456583374471175`, `>> 16` → `1792245229713`.

### 6. Clock skew in practice

- **Clock behind.** Theorem 2 keeps `l ≥ pt`; a straggler's new operations take `l` from operations it has seen, so its edits still sort after what it read (§4.2 "robust to stragglers"). Its history times (`utc_ms`) are wrong by the skew.
- **Clock ahead by hours.** With per-field LWW on HLC, every edit from that machine beats any edit made elsewhere in the next few hours, and peers that accept its timestamps inherit the jump (measured above with `uhlc`). With LWW on `server_seq`, the HLC no longer decides synced conflicts; it still decides the order of that machine's unsynced operations and the `utc_ms` shown in history.
- **Arrival order has its own failure.** LWW by `server_seq` means the last to sync wins: an Actor offline for a week who edits a field and then syncs overwrites a newer edit made by someone else. [sync-engines.md](../sync-engine-spike/sync-engines.md#open-risks) says the opposite ("Users offline for weeks will see their edits lose to later server edits on the same field"), which only holds if conflicts are decided by edit time. The two statements in that note are inconsistent (**inferred** from reading the rebase description: pending operations are rebased on top of pulled ones and then pushed, so they receive later `server_seq`).
- **Mitigations seen in production**: reject (uhlc `Err`, Actual `ClockDriftError`), crash (CockroachDB), or refuse to advance (MongoDB). tm cannot reject: a pulled operation already has a `server_seq` and must be applied. Claims also depend on clocks: "Claims expire after 2 hours by default" ([design.md](../../specs/design.md)), so a machine two hours ahead would treat every live Claim as expired (**inferred**).

## Conclusions

- Keep ordering time and display time separate. `hlc` may run ahead of the wall clock (Theorem 2, measured), and UUIDv7's embedded time is per-process and discouraged from being read, so neither can supply "10:42 your time".
- Offset beats zone name for operations: it is exactly what git keeps, it renders both "yours" and "theirs", it is unambiguous in DST folds, and it reveals less. Zone names belong to future and recurring times, which are not operation metadata.
- The HLC must be persisted and stepped in SQLite, which rules out the existing crates as-is; the algorithm is small (Figure 5 of the paper).
- Clock skew cannot be fully fixed client-side; the server's own receive time is the only independent clock in the system and is cheap to record.

## Recommendation

**Operation fields** (additions to the `ops` table in [sync-engines.md](../sync-engine-spike/sync-engines.md#data-model)):

| Column | Type | Meaning |
|---|---|---|
| `op_id` | BLOB/TEXT, UUIDv7 | identity only; embedded time never read |
| `node_id` | BLOB, random per Workspace store on a machine, generated once | HLC tie-break; which clock |
| `actor` | TEXT | glossary Actor (human, or Agent on behalf of human); not a tie-break |
| `hlc` | INTEGER, `(l_ms << 16) \| c` | local and causal order |
| `utc_ms` | INTEGER | Unix ms from the machine clock at write |
| `offset_min` | INTEGER, nullable | UTC offset of the writing machine at write; NULL when the Actor opts for UTC-only |
| `server_seq` | INTEGER, NULL until acked | authoritative order |
| `server_received_ms` | INTEGER, NULL until acked | server clock at push, returned in the ack and in pulls |

Order: `ORDER BY server_seq IS NULL, server_seq, hlc, node_id, op_id`. Pending operations sort after every acked one, as the rebase model requires.

**HLC rules** (in-house, in the shared core so client and server agree):

- State is one row per Workspace store (`last_hlc`), read and written inside the operation's `BEGIN IMMEDIATE` transaction.
- Mint: `pt = now_ms`; if `pt > l` then `(pt, 0)`, else `(l, c + 1)`; on `c == 0xFFFF`, use `(l + 1, 0)` (the uuid crate's "adjusting" behaviour).
- Receive (pull): apply the operation regardless; advance `last_hlc` from its `hlc` only up to `now_ms + ∆`, and log a skew warning when clamped. This follows §4.1's "ignore out of bounds messages" for the clock, without dropping data.
- A local clock that ran ahead and was then corrected is not reset to `pt`, unlike §4.1 of the paper: local order must stay monotonic. `l` stays ahead and `c` counts up until wall time catches up; that lag is harmless because `server_seq` decides synced conflicts and `utc_ms` is separate, and `tm sync status` surfaces it. (Superseded in part: [ADR 0006](../../decisions/0006-operation-merge-ordering-and-versioning.md) breaks concurrent conflicts by `hlc` capped at `server_received_ms` + ∆, and names `node_id` `replica_id`.)
- ∆ = 5 minutes to start (Actual's value; production databases use 500 ms, but laptops sleeping and resuming are not datacenter nodes). **Inferred** starting point; revisit with data.
- At sync, the client compares its clock with the server's (an operation pushed immediately has `server_received_ms − utc_ms` near zero); `tm sync status` reports that difference as measured skew and warns past ∆. An operation whose `utc_ms` is later than its `server_received_ms` by more than ∆ was stamped by a fast clock, and history can mark it.

**History rendering**: "theirs" = `utc_ms` shown at `offset_min` as a fixed offset (`10:42 −05:00`, no zone abbreviation, since none was recorded); "yours" = `utc_ms` in the viewer's current system zone via `jiff::Zoned`. Show one when they agree. `offset_min = NULL` renders UTC with a marker. For Agent operations ("agent, on behalf of human", [design.md](../../specs/design.md)), "theirs" is the offset of the machine the Agent ran on, which may differ from the human's.

**Crates**: `jiff` for time, `uuid` with `v7` for `op_id`, no HLC crate.

**Needs the user's decision** (not settled by this research): what per-field LWW compares for synced conflicts. Options: (a) `server_seq`, last to sync wins, immune to skew, stale offline edits win; (b) `hlc`, last edit wins, a fast clock wins within ∆; (c) operations carry the `server_seq` they were based on, so the server detects concurrent writes, keeps arrival order for non-concurrent ones and breaks concurrent ties by `hlc` (clamped by `server_received_ms + ∆`), surfacing the losing value as Automerge-style conflicts. (c) matches the existing open-risk text; it costs one more column (`base_seq`).

## Implications / actions

- A decision record for the operation's time fields and the LWW key, once the open choice above is made.
- Correct the contradiction in [sync-engines.md](../sync-engine-spike/sync-engines.md) between "LWW by `server_seq`" and "offline edits lose" (not edited here, per scope).
- Glossary: no term exists for the machine-level identity (`node_id` here); add one or reuse an existing word before code lands.
- Claims: expiry should compare against the clock that granted the Claim, or be enforced by the server once synced.

## Open questions

- Does any operation need a zone name? Only if an operation records a future local time; none does in [design.md](../../specs/design.md) today.
- ∆ value and whether a laptop resuming from sleep with a stale clock (before NTP corrects it) trips it routinely; not measured.
- Whether `node_id` should be per machine or per Workspace store; per store is simpler (it lives in the database), per machine survives `tm rebuild` only if stored outside the log (**inferred**).
- Cross-process UUIDv7 ordering was not measured here; the claim relies on the `uuid` crate's documentation.
- Jira Cloud and Linear payloads were not sampled live; Linear's format comes from its schema.

## Raw data

Probe (`cargo new clockprobe`, edition 2024, dependencies `jiff = "0.2.38"`, `uhlc = "0.9.0"`, `uuid = { version = "1.28.0", features = ["v7"] }`). The scripts live here rather than in a sibling `scripts/` folder because this pass was scoped to one file. `src/main.rs`:

```rust
use jiff::{civil::date, tz::TimeZone, Timestamp, Zoned};
use std::time::Duration;
use uhlc::{HLCBuilder, Timestamp as HTs, ID, NTP64};

fn real_clock() -> NTP64 {
    uhlc::system_time_clock()
}
fn two_hours_ahead() -> NTP64 {
    uhlc::system_time_clock() + NTP64::from(Duration::from_secs(7200))
}

fn main() {
    println!("== jiff: instant + offset vs zone ==");
    let ts: Timestamp = "2026-10-17T15:42:00Z".parse().unwrap();
    for tz in ["America/Chicago", "Europe/Berlin", "Asia/Kolkata"] {
        let z = ts.to_zoned(TimeZone::get(tz).unwrap());
        println!("{tz:16} Zoned={z}  offset_secs={}", z.offset().seconds());
    }
    let fixed = ts.to_zoned(TimeZone::fixed(jiff::tz::offset(-5)));
    println!("fixed -05 Zoned={fixed}");
    let parsed: Zoned = "2026-10-17T10:42:00-05:00[America/Chicago]".parse().unwrap();
    println!("parsed RFC 9557 back: {} -> {}", parsed, parsed.timestamp());

    println!("== jiff: DST fall-back ambiguity, America/New_York 2026-11-01 01:30 ==");
    let ny = TimeZone::get("America/New_York").unwrap();
    let amb = ny.to_ambiguous_zoned(date(2026, 11, 1).at(1, 30, 0, 0));
    println!("ambiguous? {}", amb.is_ambiguous());
    println!("compatible -> {}", amb.clone().compatible().unwrap());
    println!("later      -> {}", amb.clone().later().unwrap());
    println!("reject     -> {:?}", amb.clone().unambiguous().map(|z| z.to_string()));
    let a: Timestamp = "2026-11-01T05:30:00Z".parse().unwrap();
    let b: Timestamp = "2026-11-01T06:30:00Z".parse().unwrap();
    for t in [a, b] {
        let z = t.to_zoned(ny.clone());
        println!("utc {t} -> local {} offset_min {}", z.strftime("%H:%M"), z.offset().seconds() / 60);
    }

    println!("== uhlc 0.9 ==");
    let local = HLCBuilder::new().with_clock(real_clock).build();
    let t1 = local.new_timestamp();
    let t2 = local.new_timestamp();
    println!("t1={t1} ({:#})", t1.get_time());
    println!("t2>t1 {} ; raw u64 t1={} delta(t2-t1 raw)={}", t2 > t1, t1.get_time().as_u64(), t2.get_time().as_u64() - t1.get_time().as_u64());
    println!("default max delta = {:?}", local.get_delta().to_duration());
    let fast = HLCBuilder::new().with_clock(two_hours_ahead).with_id(ID::try_from([7u8]).unwrap()).build();
    let remote: HTs = fast.new_timestamp();
    println!("remote (2h ahead) = {:#}", remote.get_time());
    match local.update_with_timestamp(&remote) {
        Ok(()) => println!("update accepted"),
        Err(e) => println!("update rejected: {e}"),
    }
    let lenient = HLCBuilder::new().with_clock(real_clock).with_max_delta(Duration::from_secs(3 * 3600)).build();
    lenient.update_with_timestamp(&remote).unwrap();
    let after = lenient.new_timestamp();
    println!("lenient after accepting 2h-ahead remote, next local ts = {:#} (wall now {})", after.get_time(), Timestamp::now());

    println!("== uuid v7 ==");
    let u1 = uuid::Uuid::now_v7();
    let u2 = uuid::Uuid::now_v7();
    let (s, ns) = u1.get_timestamp().unwrap().to_unix();
    println!("u1={u1} u2>u1={} embedded unix={s}.{ns:09}", u2 > u1);
}
```

`examples/sys.rs`:

```rust
fn main() {
    let z = jiff::Zoned::now();
    let ms = z.timestamp().as_millisecond();
    let packed: i64 = (ms << 16) | 7;
    println!("system tz={:?} offset_min={} utc_ms={} packed_hlc={} unpacked=({}, {})", z.time_zone().iana_name(), z.offset().seconds()/60, ms, packed, packed >> 16, packed & 0xffff);
    let max_ms: i64 = i64::MAX >> 16;
    println!("max positive i64 hlc ms={} -> {}", max_ms, jiff::Timestamp::from_millisecond(max_ms).map(|t| t.to_string()).unwrap_or_else(|e| e.to_string()));
    println!("year at 2^47 ms ~ {:.0}", 1970.0 + (1u64<<47) as f64 / 1000.0 / 86400.0 / 365.2425);
}
```

Full output of `cargo run -q`:

```text
== jiff: instant + offset vs zone ==
America/Chicago  Zoned=2026-10-17T10:42:00-05:00[America/Chicago]  offset_secs=-18000
Europe/Berlin    Zoned=2026-10-17T17:42:00+02:00[Europe/Berlin]  offset_secs=7200
Asia/Kolkata     Zoned=2026-10-17T21:12:00+05:30[Asia/Kolkata]  offset_secs=19800
fixed -05 Zoned=2026-10-17T10:42:00-05:00[-05:00]
parsed RFC 9557 back: 2026-10-17T10:42:00-05:00[America/Chicago] -> 2026-10-17T15:42:00Z
== jiff: DST fall-back ambiguity, America/New_York 2026-11-01 01:30 ==
ambiguous? true
compatible -> 2026-11-01T01:30:00-04:00[America/New_York]
later      -> 2026-11-01T01:30:00-05:00[America/New_York]
reject     -> Err(error converting datetime to instant in time zone America/New_York: datetime is ambiguous since it falls into a fold between offsets -04 and -05)
utc 2026-11-01T05:30:00Z -> local 01:30 offset_min -240
utc 2026-11-01T06:30:00Z -> local 01:30 offset_min -300
== uhlc 0.9 ==
t1=7697634648030274464/96ce3aed67676179ca4014eb41f76b5c (2026-10-17T13:53:49.713216998Z)
t2>t1 true ; raw u64 t1=7697634648030274464 delta(t2-t1 raw)=4304
default max delta = 500ms
remote (2h ahead) = 2026-10-17T15:53:49.713225000Z
update rejected: incoming timestamp from 7 exceeding delta 500ms is rejected: 2026-10-17T15:53:49.713225000Z vs. now: 2026-10-17T13:53:49.713225998Z
lenient after accepting 2h-ahead remote, next local ts = 2026-10-17T15:53:49.713225001Z (wall now 2026-10-17T13:53:49.713233Z)
== uuid v7 ==
u1=01a14a23-e891-70f3-846d-1541953b1f30 u2>u1=true embedded unix=1792245229.713000000
```

Packed-HLC and system-zone probe (`examples/sys.rs`), three runs (no `TZ`, `TZ=Asia/Kolkata`, `TZ=UTC`); each run also printed `year at 2^47 ms ~ 6430`, and the repeated `max positive` lines are omitted:

```text
system tz=Some("America/Chicago") offset_min=-300 utc_ms=1792245505255 packed_hlc=117456601432391687 unpacked=(1792245505255, 7)
max positive i64 hlc ms=140737488355327 -> 6429-10-17T02:45:55.327Z
system tz=Some("Asia/Kolkata") offset_min=330 utc_ms=1792245505411 packed_hlc=117456601442615303 unpacked=(1792245505411, 7)
system tz=Some("UTC") offset_min=0 utc_ms=1792245505544 packed_hlc=117456601451331591 unpacked=(1792245505544, 7)
```

git fixture: `git init`, then two commits made with `GIT_AUTHOR_DATE="2026-10-17T10:42:00"` under `TZ=America/Los_Angeles` and `TZ=Asia/Kolkata`, signing disabled with `-c commit.gpgSign=false`; outputs quoted in Findings §3.
