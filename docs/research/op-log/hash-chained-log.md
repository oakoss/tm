# Hash-chained operation log: what to hash so `tm sync status` can prove agreement

- Date: 2026-10-17
- Author: Claude (research pass for Jace Babin)
- Scope: Should synced operations form a hash chain, and what do Mononoke's Bonsai changesets, Anthropic's Scion and other hash-linked logs teach about verifying that a client and `tm serve` hold the same history?

Method note: sources were fetched on 2026-10-17. Mononoke claims cite source and docs in `facebook/sapling` pinned at commit `79071c2a3ea20c36ed71332608535e5801101bba` (main as of 2026-10-14T20:23:55Z, from `gh api repos/facebook/sapling/commits/main`). Benchmark numbers were measured on this machine (Apple M2 Max, `cargo 1.99.1`, release build). Anything not quoted or measured is labeled **inferred**.

## Question

1. What does a Bonsai changeset contain, why does Mononoke derive trees instead of storing them, and how do its hashes and parents work?
2. What did Anthropic publish about Scion, and how does it relate to Bonsai?
3. How do other hash-linked logs (git, Certificate Transparency, Automerge, Hypercore, Merkle-CRDTs, Matrix, Fossil, jj) check that two replicas agree, and at what cost?
4. Which verification design fits tm: (a) no hashes, (b) a linear chain in `server_seq` order, (c) a Merkle tree with consistency proofs, or (d) a Bonsai-like Merkle DAG?
5. Which hash function, which serialization and which fields?

## Sources

- Mononoke Bonsai data model doc: <https://github.com/facebook/sapling/blob/79071c2a3ea20c36ed71332608535e5801101bba/eden/mononoke/docs/2.1-bonsai-data-model.md>
- Mononoke derived data doc: <https://github.com/facebook/sapling/blob/79071c2a3ea20c36ed71332608535e5801101bba/eden/mononoke/docs/2.3-derived-data.md>
- Mononoke pushrebase doc: <https://github.com/facebook/sapling/blob/79071c2a3ea20c36ed71332608535e5801101bba/eden/mononoke/docs/4.1-pushrebase.md>
- `BonsaiChangesetMut` and the changeset-id computation: <https://github.com/facebook/sapling/blob/79071c2a3ea20c36ed71332608535e5801101bba/eden/mononoke/mononoke_types/src/bonsai_changeset.rs>
- Typed hash contexts: <https://github.com/facebook/sapling/blob/79071c2a3ea20c36ed71332608535e5801101bba/eden/mononoke/mononoke_types/src/typed_hash.rs> and `.../mononoke_types/src/hash.rs`
- Derived data crate docs: <https://github.com/facebook/sapling/blob/79071c2a3ea20c36ed71332608535e5801101bba/eden/mononoke/derived_data/src/lib.rs>
- Anthropic, "Redesigning source control at Anthropic" (published 2026-10-13, updated 2026-10-15): <https://www.anthropic.com/engineering/redesigning-source-control>
- Rain (sunshowers), "Anthropic's Scion is heavily inspired by Mononoke" (2026-10-14): <https://sunshowers.io/posts/anthropic-mononoke/>
- Rain (sunshowers), "More on Anthropic's Scion ripping off Mononoke" (2026-10-15): <https://sunshowers.io/posts/anthropic-mononoke-2/>
- Nick G.'s reply on Hacker News: <https://news.ycombinator.com/item?id=50107559>
- RFC 9162, Certificate Transparency Version 2.0: <https://www.rfc-editor.org/rfc/rfc9162.html>
- Russ Cox, "Transparent Logs for Skeptical Clients" (2019-03-01): <https://research.swtch.com/tlog>
- Automerge binary format spec: <https://automerge.org/automerge-binary-format-spec/>
- Automerge sync module source: <https://github.com/automerge/automerge/blob/main/rust/automerge/src/sync.rs> and `.../sync/state.rs`
- Hypercore docs: <https://docs.pears.com/building-blocks/hypercore>
- Sanjuán et al., "Merkle-CRDTs: Merkle-DAGs meet CRDTs" (arXiv 2004.00107, 2020): <https://arxiv.org/abs/2004.00107>
- Matrix server-server API, Signing Events: <https://spec.matrix.org/latest/server-server-api/> and room version 4: <https://spec.matrix.org/latest/rooms/v4/>
- Fossil sync protocol: <https://fossil-scm.org/home/doc/trunk/www/sync.wiki> and file format: <https://fossil-scm.org/home/doc/trunk/www/fileformat.wiki>
- Jujutsu concurrency and the operation log: <https://docs.jj-vcs.dev/latest/technical/concurrency/>
- Git protocol v2: <https://git-scm.com/docs/gitprotocol-v2>
- SQLite `dbhash`: <https://www.sqlite.org/dbhash.html>
- RFC 8785, JSON Canonicalization Scheme: <https://www.rfc-editor.org/rfc/rfc8785.html>
- RFC 8949 §4.2, deterministic CBOR: <https://www.rfc-editor.org/rfc/rfc8949.html#name-deterministically-encoded-c>
- Crates `blake3` 1.8.7 and `sha2` 0.11.1 / 0.10.9 (`cargo info`, sources under `~/.cargo/registry/src/`)

## Findings

### 1. Mononoke: Bonsai changesets and derived data

**What a changeset holds.** The struct at the pinned commit (`bonsai_changeset.rs` lines 44–59) is `parents: Vec<ChangesetId>`, `author`, `author_date`, `committer`, `committer_date`, `message`, `hg_extra`, `git_extra_headers`, `file_changes: SortedVectorMap<NonRootMPath, FileChange>`, `is_snapshot`, `git_tree_hash`, `git_annotated_tag`, `subtree_changes`. There is no root tree. The data model doc says file changes are "a flat list of path-to-change mappings", each a tracked change ("content identifier, file type, size, and optional copy-from information"), a deletion, or an untracked change for snapshots ([2.1-bonsai-data-model.md](https://github.com/facebook/sapling/blob/79071c2a3ea20c36ed71332608535e5801101bba/eden/mononoke/docs/2.1-bonsai-data-model.md)).

**How it is hashed.** The doc: "A Bonsai changeset is serialized (using Thrift compact protocol) and hashed with Blake2b. The resulting hash becomes the changeset identifier (`ChangesetId`). This identifier depends on: All metadata fields [...] Parent changeset hashes [...] All file changes". In source, `changeset_id()` serializes with `compact_protocol::serialize` and feeds a `ChangesetIdContext`; `typed_hash.rs` line 647 declares `context_key => "changeset"`, the `impl_typed_hash!` macro passes it on to `impl_typed_context!` (lines 581–585), which calls `Context::new($key.as_bytes())` (line 533), and `hash.rs` line 153–158 turns a non-empty key into `Blake2bMac<U32>`. So every typed hash (changeset, content, and so on) is a keyed BLAKE2b-256 with its type name as the key: domain separation by type. `BonsaiChangeset::from_bytes` (line ~325) computes the id over the stored bytes as loaded, then deserializes; it does not re-canonicalize.

**Inherent versus derived.** The doc splits data into "Inherent data [which] constitutes the core Merkle DAG and serves as the source of truth" (changesets, content blobs, original VCS bytes) and "Derived data [...] not included in content-addressed hashes and can be regenerated from inherent data". The reason given: "The write path stores only inherent data, keeping the critical section minimal. Derived data is computed asynchronously off the critical path." Directory manifests (trees), blame, file history and Git/Mercurial formats are all derived ([2.3-derived-data.md](https://github.com/facebook/sapling/blob/79071c2a3ea20c36ed71332608535e5801101bba/eden/mononoke/docs/2.3-derived-data.md)). The crate doc states the derivation rule: "Each type of derived data can be derived from a changeset when given: The bonsai changeset (including its changeset ID). The derived data of the same type for the immediate parent changesets." ([derived_data/src/lib.rs](https://github.com/facebook/sapling/blob/79071c2a3ea20c36ed71332608535e5801101bba/eden/mononoke/derived_data/src/lib.rs)).

**Server-side rebase rewrites identity.** Pushrebase, for each pushed commit, will "Create a new commit with the same changes but with updated parents", and "Sapling clients receive obsmarkers mapping original commits to rebased commits"; the server also records "the original commit hash (pushrebase mutation mapping)" ([4.1-pushrebase.md](https://github.com/facebook/sapling/blob/79071c2a3ea20c36ed71332608535e5801101bba/eden/mononoke/docs/4.1-pushrebase.md)). Because parents are inside the hash, a server reorder mints a new id and needs a mapping back.

**Integrity checking is a walk, not a head compare.** Mononoke's walker "traverses this graph to validate data consistency, verify storage durability across multiple blobstore backends" ([6.2-walker-and-validation.md](https://github.com/facebook/sapling/blob/79071c2a3ea20c36ed71332608535e5801101bba/eden/mononoke/docs/6.2-walker-and-validation.md)).

Rain's description (2026-10-15) matches: a Bonsai changeset holds "metadata (author, commit message etc)" plus "hashes of parents, plus the list of files changed in it and hashes of blobs", and "is still a Merkle DAG, just with a different model" ([anthropic-mononoke-2](https://sunshowers.io/posts/anthropic-mononoke-2/)). Rain's first post says derived data "can be computed and stored asynchronously, outside of the main critical section" ([anthropic-mononoke](https://sunshowers.io/posts/anthropic-mononoke/)). The 2022 Oxide materials Rain mentions are not linked from either post; not located.

### 2. Scion

Anthropic's post ([redesigning-source-control](https://www.anthropic.com/engineering/redesigning-source-control), fetched 2026-10-17):

> In Scion, the unit of work is a "changeset," an entry in a write-ahead log. A changeset stores most of what you'd expect from a commit in git. But where a git commit stores a pointer to the tree, a changeset stores a list of the files that changed relative to its first parent, along with content-addressed pointers to the compressed file contents in blob storage.

On derived data: "We call everything that isn't directly part of the changeset "derived data," and we compute it all through one generic mechanism. Each type of derived data is defined recursively, so its value for a changeset is computed from its values for that changeset's parents." On refs: "moving one is a compare-and-swap on a single Spanner row that happens separately from creating changesets." On content addressing: "All source control data is content-addressed, so a piece of data never changes once it's stored". The post says nothing about how replicas or clients verify they agree.

Attribution, factually:

- 2026-10-13: post published; prior art paragraph credited "Google's Piper and Meta's Sapling" for "hard-won lessons".
- 2026-10-14: Rain, who worked on Mononoke, argued the changeset and derived-data design is Mononoke's and that crediting only Sapling "borders on plagiarism" ([anthropic-mononoke](https://sunshowers.io/posts/anthropic-mononoke/)).
- 2026-10-15: the post was "Updated October 15 to include more detail on the projects that pioneered many of the techniques we use in Scion" and now reads: "Specifically, the server component of Sapling, Mononoke. It pioneered many of the techniques we use in Scion, in particular the Bonsai data model." Nick G. (`nickg-anthropic`) replied on HN: "We were inspired by many things while building Scion, such as the great ideas in Mononoke's Bonsai data model" ([HN 50107559](https://news.ycombinator.com/item?id=50107559)).
- 2026-10-15: Rain's follow-up notes "sufficient attribution has been added" and that they "stand by" the earlier posts ([anthropic-mononoke-2](https://sunshowers.io/posts/anthropic-mononoke-2/)).

So the Bonsai lineage is stated by Anthropic itself as of 2026-10-15, not only reported by third parties.

### 3. Other hash-linked logs and how replicas check agreement

| System | What is hashed | How two replicas check agreement | Cost |
|---|---|---|---|
| Git | Commit object = tree + parents + metadata (`git cat-file -p HEAD` in this repo printed `tree 38f7867…`, `parent d14c12d…`, `author …`) | Compare ref tips (`ls-refs` returns `obj-id SP refname`); fetch negotiates with `want <oid>` / `have <oid>` "so the server [can] make a packfile which only contains the objects that the client needs" ([gitprotocol-v2](https://git-scm.com/docs/gitprotocol-v2)) | One hash per object; equality is one hash compare, missing-set discovery is rounds of `have` |
| Certificate Transparency (RFC 9162) | Merkle tree: leaf `HASH(0x00 ‖ d)`, node `HASH(0x01 ‖ left ‖ right)`; prefixes are "domain separation [...] required to give second preimage resistance" (§2.1.1) | Signed tree head = `tree_size`, `root_hash`, timestamp, signature (§4.9–4.10). Consistency proof "prove[s] the append-only property"; size "bounded above by ceil(log2(n)) + 1" (§2.1.4) | log n hashes per proof; lets a client verify append-only without downloading entries |
| Go checksum DB / tlog (Cox 2019) | Same tree, stored as tiles | "the client caches the most recently observed log size N and top-level hash T" and checks each new head includes it; proof is "O(lg N)" ([tlog](https://research.swtch.com/tlog)) | As CT; tiles make proofs cacheable |
| Automerge | Change hash = "32-byte [SHA256] hash" of the change chunk bytes; a change lists its dependency hashes, actor, seq, start op, time, message, ops ([format spec](https://automerge.org/automerge-binary-format-spec/)) | Sync message carries `heads`, `need`, `have` (Bloom-filter summary); per-peer `State` keeps `shared_heads`; loop ends when neither side has a message ([sync.rs](https://github.com/automerge/automerge/blob/main/rust/automerge/src/sync.rs), `sync/state.rs`). Based on arXiv 2012.00472 per the source header | Several round trips; head sets, not one hash |
| Hypercore | Append-only log with a Merkle tree; tree hash is "A 32-byte BLAKE2b hash of the Merkle tree roots at the given length"; Ed25519 signature over key, tree hash, length, fork id ([docs](https://docs.pears.com/building-blocks/hypercore)) | Readers verify blocks and upgrades with proofs against the signed root. Truncation is signed and `core.fork` is "incremented on each truncation" | Single writer; signatures make heads transferable proof |
| Merkle-CRDTs | Merkle-DAG of CRDT deltas; "Merkle-DAGs can act as logical clocks" ([arXiv 2004.00107](https://arxiv.org/abs/2004.00107)) | Exchange root ids, fetch missing nodes (**inferred** from the abstract; full text not read) | Leaderless; DAG walk |
| Matrix | Event id (room v4+) is "the reference hash of the event encoded using URL-safe unpadded Base64"; reference hash = redact, remove `signatures`/`unsigned`, Canonical JSON, SHA-256 ([s2s](https://spec.matrix.org/latest/server-server-api/), [v4](https://spec.matrix.org/latest/rooms/v4/)) | `prev_events` form a DAG; gaps filled by `/get_missing_events`, a "breadth-first walk of the `prev_events`" | Every implementation must reproduce Canonical JSON bit-for-bit |
| Fossil | Each artifact "is identified by a cryptographic hash of its content" (SHA1 or SHA3-256); ticket changes are artifacts with D/J/K/U/Z cards ([fileformat](https://fossil-scm.org/home/doc/trunk/www/fileformat.wiki)) | Exchange `igot` hash lists, request with `gimme`; no whole-repo hash for artifacts (only a `uv-hash` for unversioned files) ([sync.wiki](https://fossil-scm.org/home/doc/trunk/www/sync.wiki)) | Hash lists proportional to unclustered artifacts |
| Jujutsu op log | Operations and views "are stored in content-addressed storage just like Git commits are"; an operation points to its view and parent operation(s) ([concurrency](https://docs.jj-vcs.dev/latest/technical/concurrency/)) | Local only; concurrent ops leave multiple op heads, merged 3-way on the next command | Local |
| SQLite `dbhash` | "the SHA1 hash of the schema and content", constant across VACUUM, page size, journal mode, backup API ([dbhash](https://www.sqlite.org/dbhash.html)) | Compare one hash of content | Full table scan per check |

No SQLite-based sync engine reviewed in [sync-engines.md](../sync-engine-spike/sync-engines.md) (cr-sqlite, sqlite-sync, Turso Sync, libSQL) was found to hash-chain its change log; that note did not look for it, so this is **unverified**.

### 4. Design options for tm

Context from the repo: ops carry `op_id`, Actor, `project`, `entity_id`, `kind`, `payload`, `hlc`, `server_seq` (NULL until acked) and `status` ([sync-engines.md, Custom design](../sync-engine-spike/sync-engines.md)); one authority orders ops and assigns Keys ([ADR 0004](../../decisions/0004-one-authority-assigns-keys.md)); compaction "must never drop content, only collapse superseded operations" ([ADR 0005](../../decisions/0005-operation-log-is-the-source-of-truth.md)); every machine keeps a full local copy (requirement 1).

The scenario that separates the options: **the server is restored from a backup taken at `server_seq` K, then other machines push exactly N−K new ops before this client syncs again.** The client's cursor is N and the server's is N, but positions K+1..N hold different ops.

| | (a) cursor + hash over materialized rows | (b) linear chain in `server_seq` order | (c) Merkle tree + consistency proofs | (d) Merkle DAG with parents (Bonsai-like) |
|---|---|---|---|---|
| Cheap equality check | Yes, but computing it is a table scan (as `dbhash`) | Yes, compare `(cursor, head)` | Yes, compare `(size, root)` | Compare head sets |
| Detects the restore-then-diverge scenario | Probably (rows differ), cannot say where | Yes; a client that stored `chain_hash` per acked op finds K by comparing per-seq hashes (bisect or a server lookup). Head-only storage detects but does not locate | Yes, and a consistency proof from the client's size fails | Yes, heads differ |
| Detects a lagging server vs a forked one | No | Yes: server head at seq K equals client's stored hash at K means "behind"; not equal means "forked" (Hypercore's fork counter is the same distinction) | Yes | Yes |
| Tamper evidence | None | Against a server that rewrites history behind a client that remembered an earlier head (the CT remembered-head model) | Same, plus proofs without downloading | Same |
| Partial sync (one Project, or a sandbox snapshot) | Per-table hashes possible | Per-Project chain works, since `server_seq` is per Project | Proofs of inclusion for subsets | Natural |
| Storage per op | 0 | 32 B (`chain_hash`), plus 32 B `op_hash` if kept separately | 32 B leaf plus interior nodes (~2× leaves, or tiles) | 32 B id plus parent list |
| CPU per op | 0 per op; O(rows) per check | One hash of ~300 B: 0.13–0.41 µs measured below | log n node updates | One hash |
| Complexity | Low; but catches projection bugs, not history divergence | Low | Medium (tree code, proof API) | High: parents are always `server_seq − 1` under one authority, so the DAG carries no information, and a server reorder rewrites ids (the pushrebase mapping problem) |
| Compaction | Unaffected | Must not rewrite chain hashes; see recommendation | Same constraint, plus tree nodes | Collapsing ops rewrites ids of every descendant |
| Pending ops | Invisible | No chain position until acked; identity hash only | Same | Natural (local heads), at the cost of rewrite on rebase |

Why not (c): CT's log-sized proofs exist so a client can confirm append-only without fetching entries (RFC 9162 §2.1.4; Cox's "skeptical clients"). A tm client downloads every op anyway, so it can recompute the chain from its stored head over the pulled ops; the check is free during pull. (c) earns its place only if partial sync or a client that does not keep the log becomes a requirement. If it does, the per-op `op_hash` values are the same leaves a tree would use, so the switch would not change op identity (**inferred**).

Why not (d): ADR 0004 already imposes one total order per Project. A DAG would record parents that are always the previous `server_seq`, and putting parents inside the hash means a server reorder of pending ops changes their identity, which is the problem Mononoke solves with obsmarkers and a mutation mapping. The Bonsai idea that does transfer is the split between **identity** (a hash of what the op says) and **position** (where the authority put it).

Why not (a) alone: it cannot tell "behind" from "forked", cannot locate divergence, and costs a table scan per check. It still has a job (below).

### 5. Hash function and serialization

**Measured speed.** 100,000 synthetic ops of 300 bytes, best of 5 runs, release build, Apple M2 Max. Source in Raw data. Run with `sha2 = "0.10"` (lock: `blake3 1.8.7`, `sha2 0.10.9`):

```text
blake3 per-op digest         best of 5:    33.69 ms  (   337 ns/op)
sha256 per-op digest         best of 5:    83.40 ms  (   834 ns/op)
blake3 chain H(prev||op)     best of 5:    41.17 ms  (   412 ns/op)
sha256 chain H(prev||op)     best of 5:   100.49 ms  (  1005 ns/op)
```

Run with `sha2 = "0.11"` (lock: `blake3 1.8.7`, `sha2 0.11.1`):

```text
blake3 per-op digest         best of 5:    34.56 ms  (   346 ns/op)
sha256 per-op digest         best of 5:    12.74 ms  (   127 ns/op)
blake3 chain H(prev||op)     best of 5:    41.11 ms  (   411 ns/op)
sha256 chain H(prev||op)     best of 5:    17.47 ms  (   175 ns/op)
```

A second 0.11.1 run gave 132 / 175 ns/op for SHA-256 and 350 / 412 for BLAKE3. The 6× SHA-256 gap is the backend: in `sha2-0.10.9/src/sha256.rs` the aarch64 path is `#[cfg(all(feature = "asm", target_arch = "aarch64"))]`, so a default 0.10 build used the software implementation, while in `sha2-0.11.1/src/sha256.rs` (lines 44–72) the default branch declares `cpufeatures::new!(sha2_hwcap, "sha2")` on aarch64 and calls `aarch64_sha2::compress` when `sha2_hwcap::get()` is true, with no feature flag. Cross-check: `openssl speed -seconds 1 -bytes 320 -evp sha256` (OpenSSL 4.0.3) printed `sha256 1565024.64k`, about 204 ns per 320-byte block, in line with the 0.11.1 figure. BLAKE3's NEON code is compiled by default on little-endian aarch64 (`blake3-1.8.7/build.rs` lines 366–371), but `CHUNK_LEN` is 1024 (`src/lib.rs` line 166) and BLAKE3's SIMD speedup comes from hashing several chunks in parallel, so a 300-byte op is one chunk on the portable compressor (**inferred** from the chunk size; not profiled). x86 machines without SHA extensions were not measured.

Conclusion: either function hashes 100k ops in under 50 ms here. Speed does not decide it.

**Choosing.** SHA-256 is what CT, Automerge and Matrix use, `shasum` ships on this macOS machine (`which shasum` printed `/usr/bin/shasum`) and Linux has `sha256sum` in coreutils (**inferred**, not checked here), while `b3sum` is not installed here (`which b3sum` printed `b3sum not found`), and it was faster on this machine for op-sized inputs. BLAKE3 offers `derive_key(context, …)` / `Hasher::new_derive_key(context)` for built-in domain separation; with SHA-256 the same comes from a fixed prefix, as RFC 9162 does with `0x00`/`0x01` and Mononoke does with a per-type key.

**Serialization pitfalls.** Canonical JSON (RFC 8785) sorts keys by UTF-16 code units, formats numbers as ECMAScript does, requires I-JSON, rejects NaN/Infinity, and does not normalize Unicode, so "equivalent strings with different code point sequences will not canonicalize to the same output". Deterministic CBOR (RFC 8949 §4.2.1) requires preferred serialization, no indefinite lengths and bytewise-sorted map keys, and §4.2.2 leaves float, NaN and −0 rules to the protocol. Matrix shows the cost of hashing a re-canonicalized form: every implementation must reproduce redaction plus Canonical JSON exactly. Mononoke (`from_bytes` hashes stored Thrift bytes) and Automerge (hash of the change chunk bytes) instead hash the bytes they store. Other pitfalls to design out: map key order, optional versus explicit null, integer width, timestamp precision, and schema evolution (a new field silently changes the hash unless the format carries a version).

## Conclusions

**Recommendation: option (b), a linear hash chain in `server_seq` order, with op identity hashed separately from chain position.** Proposed, not decided; it would need a decision record.

What goes in the hashes:

1. `op_hash` is computed by the client when the op is created, and is the op's identity. Input: a fixed binary envelope with a format version byte, then length-prefixed fields in a fixed order: `op_id` (UUIDv7, 16 bytes), Actor, Project, `entity_id` (Issue UUID), `kind`, `hlc`, and the payload bytes. The payload is JSON written once by the client and stored as opaque TEXT/BLOB; neither side re-serializes it (in particular, not through SQLite's `json()` or JSONB, which re-encode). Excluded: `server_seq`, `status`, local row ids, received-at times, and any provisional Key display (derived).
2. `chain_hash[n] = H(prefix ‖ chain_hash[n−1] ‖ server_seq ‖ op_hash ‖ server_effects)`, computed by the server inside the push transaction. `server_effects` covers decisions only the server makes, at minimum the Key assigned to a `create`; without it two replicas could agree on history and disagree on Keys. Genesis is a fixed value per Project.
3. SHA-256, with distinct one-byte prefixes for `op_hash` and `chain_hash` inputs (CT-style domain separation). BLAKE3 with two `derive_key` contexts is equally sound; the measurements do not favor it for this input size.

Sync rules:

- Push: the server recomputes `op_hash` from the received bytes and rejects a mismatch, and rejects an `op_id` it already holds with a different `op_hash` (idempotency plus a corruption check). Rejected ops never enter the chain. The ack returns `(server_seq, chain_hash)` per op.
- Pull: each op comes back as `(op bytes, server_seq, server_effects such as the assigned Key, chain_hash)`, so the client can recompute the chain from its stored head and compare it with the server's value. It refuses to advance on a mismatch. It stores `chain_hash` per acked op (32 B/op) so it can locate a divergence.
- Pending ops have an `op_hash` but no chain position. Rebasing pending ops on top of pulled ops re-applies them; it never rewrites them, so their identity does not change (the property Bonsai lacks and pushrebase patches with a mapping).
- `tm sync status` reports synced only when pending is zero and the server's `(cursor, chain_head)` equals the client's. When the server's head at the client's cursor differs, it reports a fork at the first differing `server_seq`; when the server's cursor is lower and its head matches the client's stored hash at that seq, it reports the server as behind (restore from backup) and the client can re-push.
- A content hash over materialized rows (dbhash-style: content, not pages) stays useful as a secondary check: same chain head with different tables means a non-deterministic or buggy apply function. This is Mononoke's inherent/derived split applied to ADR 0005's projections; the command shape is not decided here.

Compaction (ADR 0005): chain hashes are never rewritten. Two shapes work:

- **Skeleton kept forever.** Keep `(server_seq, op_id, op_hash, chain_hash)` for every op and collapse only payloads of superseded ops. About 100 B/op, so 100k ops is about 10 MB (**inferred** arithmetic). The chain stays checkable; the collapsed ops' `op_hash` can no longer be recomputed from content.
- **Epochs.** The server closes a chain at seq S and starts a new one whose genesis commits to `chain_hash[S]` and a hash of the snapshot. Smaller, but a client bootstrapping from the snapshot trusts `chain_hash[S]` and can verify only from S onward.

The skeleton shape is simpler and fits tracker scale; the epoch shape is the escape hatch. Either way the trust boundary is stated plainly.

What this does not give: proof to a third party. A chain head shows a client that the server did not rewrite history the client already saw; it does not prove anything to someone who never held an earlier head. That would need server signatures over heads (Hypercore signs with Ed25519, CT signs tree heads). Out of scope for one human and their Agents.

**Crediting prior art.** The decision record and the module docs for the op log should name: Mononoke's Bonsai model and derived data (identity vs position, inherent vs derived, pushrebase's id mapping), with links to the pinned `facebook/sapling` files; RFC 9162 for domain-separated hashing and the remembered-head check; Hypercore for the behind-versus-forked distinction; Fossil, which ADR 0005 already cites. Anthropic's Scion post is a later application of the Bonsai model by its own 2026-10-15 account and is worth citing as such, not as the origin.

## Implications / actions

- A decision record "Synced operations form a hash chain" (proposed), citing this note; it should state the hash inputs, the compaction shape and the trust boundary.
- The op format spec needs the envelope layout and format version before the first synced op is written, because changing what is hashed later is a chain migration.
- The sync spec should define the `tm sync status` outcomes: synced, pending, behind, forked.

## Open questions

- **TypeScript web UI.** If it ever creates ops itself, its `op_hash` must be bit-identical to the Rust one. A fixed binary envelope with opaque payload bytes is easy to port; canonical JSON would not be. Simplest: the web UI submits ops through the Rust server, which hashes them. Undecided.
- **Op format migrations.** A versioned envelope keeps old hashes valid, but an op whose payload is upgraded in place would change its hash. Upgrades must be new ops or read-time translations.
- **Per-Project chains and moves.** `server_seq` is per Project; an Issue moving between Projects spans two chains. Not designed.
- **Projection hash cost** at realistic Workspace sizes is unmeasured.
- **x86 performance** without SHA extensions is unmeasured (irrelevant at these sizes, **inferred**).
- Whether any SQLite-based sync engine hash-chains its change log was not checked.

## Raw data

The benchmark crate lived in the session scratchpad (`hashbench/`), not the repo; this note is the only file written. `Cargo.toml` dependencies: `blake3 = "1"`, `sha2 = "0.11"` (first run `"0.10"`); `[profile.release] opt-level = 3`. `src/main.rs`:

```rust
use sha2::{Digest, Sha256};
use std::time::Instant;

const N: usize = 100_000;
const OP_LEN: usize = 300;
const RUNS: usize = 5;

fn ops() -> Vec<Vec<u8>> {
    let mut x: u64 = 0x9e3779b97f4a7c15;
    (0..N)
        .map(|_| {
            (0..OP_LEN)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    x as u8
                })
                .collect()
        })
        .collect()
}

fn chain_blake3(ops: &[Vec<u8>]) -> [u8; 32] {
    let mut prev = [0u8; 32];
    for op in ops {
        let mut h = blake3::Hasher::new();
        h.update(&prev);
        h.update(op);
        prev = *h.finalize().as_bytes();
    }
    prev
}

fn chain_sha256(ops: &[Vec<u8>]) -> [u8; 32] {
    let mut prev = [0u8; 32];
    for op in ops {
        let mut h = Sha256::new();
        h.update(prev);
        h.update(op);
        prev = h.finalize().into();
    }
    prev
}

fn per_op_blake3(ops: &[Vec<u8>]) -> u8 {
    ops.iter().fold(0, |a, op| a ^ blake3::hash(op).as_bytes()[0])
}

fn per_op_sha256(ops: &[Vec<u8>]) -> u8 {
    ops.iter().fold(0, |a, op| a ^ Sha256::digest(op)[0])
}

fn time<T: std::fmt::Debug>(name: &str, f: impl Fn() -> T) {
    let mut best = f64::MAX;
    let mut out = None;
    for _ in 0..RUNS {
        let t = Instant::now();
        let r = f();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        out = Some(r);
    }
    let _ = std::hint::black_box(out);
    let ns_per_op = best * 1e6 / N as f64;
    println!("{name:<28} best of {RUNS}: {best:8.2} ms  ({ns_per_op:6.0} ns/op)");
}

fn main() {
    let ops = ops();
    println!("{N} ops x {OP_LEN} bytes = {} bytes", N * OP_LEN);
    time("blake3 per-op digest", || per_op_blake3(&ops));
    time("sha256 per-op digest", || per_op_sha256(&ops));
    time("blake3 chain H(prev||op)", || chain_blake3(&ops));
    time("sha256 chain H(prev||op)", || chain_sha256(&ops));
}
```
