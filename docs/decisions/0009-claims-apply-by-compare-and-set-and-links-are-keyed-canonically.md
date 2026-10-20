# Claims apply by compare-and-set, and Links are keyed canonically

- Status: accepted
- Date: 2026-10-17
- Deciders: Jace Babin

## Context and Problem Statement

[ADR 0006](0006-operation-merge-ordering-and-versioning.md) says a pulled operation is always applied, that Claims are "decided by the authority, like Keys", and that Links merge as a set, per element. design.md adds that a take succeeds only on an unclaimed Issue, "never on 'already has that value'", that Links have deterministic keys, and that Links which would create a blocking cycle are refused. How must a take be checked and applied so that exactly one taker wins, locally and after sync, and how must a Link be keyed so that the same Link written from either end, on any Replica, is one element?

## Decision Drivers

- Exactly one of many concurrent takers wins, including takers that share an Agent name.
- A take that loses under sync stays in the log and is applied as lost, never refused or dropped.
- The outcome of replay is a function of log order alone, so every Replica rebuilds the same Claims.
- Per-element set merge through `field_versions` works for Links without a schema change.
- A blocking cycle empties the ready queue (design.md), and a cycle of Links or parents makes any walk of them loop. A merge can still produce one, so these checks follow design.md's rule for Workflow rules: checked when this Replica writes, never on replay.

## Decision Outcome

- **A Claim is its own operation kind, `claim`**, with an action of take or release; it is never `set_field`, whose last-writer-wins merge ADR 0006 rules out for Claims.
- **A take is checked inside the `BEGIN IMMEDIATE` transaction that appends it.** Measured on macOS with 50 processes over 20 runs, that gave one winner every run; a check outside the transaction gave two or more winners in every run ([research](../research/workspaces/claims-and-links.md)).
- **The rule is strict.** An Issue already held is refused even when the holder has the taker's own Agent name; the refusal names the holder. With one shared name, a probe applying beads' idempotent rule told all 50 sessions they held the Issue.
- **A take records `prev`, the Claim it saw as current** (none when unclaimed), **and applies by compare-and-set**: in canonical order it wins only if the current Claim is still `prev`. Server order is then the authority with no server logic; unlike a Key, a Claim is decided by replay order, not written as a server effect.
- **A take that loses is kept in the log and recorded in `unapplied` as lost**, naming the Claim that won; the loss Notification and `tm doctor` read it from there, and `tm doctor` lists lost Claims apart from operations that do not fit.
- **The Claim's id is its token.** A release names it, and Phase 3's protected fields will accept it, so of several sessions sharing an Agent name only the one holding the token acts as the holder.
- **`tm move` into a Status whose category is done or cancelled releases the Issue's Claim** in the same batch, so the Agent needs no `tm release` for finished work. A move to any other Status, review included, keeps it, so the Agent holds the Issue through review. Whoever moves writes the release, which needs no token, as no write is protected until Phase 3; the write is refused with `claim_changed` if the Claim changed after the move read it. Only `tm move` releases this way, and a take on a finished Issue is still allowed.
- **A take's payload carries its expiry duration**, so Phase 3's expiry needs no new format. design.md's "any write renews" may become an explicit operation; Phase 3 decides.
- **A Link is keyed by `(type, source, target)` in canonical form.** A directional type is stored only outward ("B is blocked by A" is "A blocks B"); a symmetric type orders its two Issue ids by bytes. The type is part of the key. The operation is filed under the canonical source, with element `<type>/<target>`, so both ends and every Replica write the same `field_versions` row.
- **A Link type is stored as text** and checked against the built-in types only when written, as Scheme ids are ([ADR 0007](0007-the-projection-does-not-depend-on-the-scheme.md)).
- **A `blocks` Link or a parent that would close a cycle is refused when this Replica writes it; a cycle produced by merging is kept.** The check walks reachable Issues with the join order pinned (`reach CROSS JOIN links`): left to the planner, the walk over 10,000 Issues took 10.78 s instead of 0.013 s. The parent check is not measured. Anything that walks Links or parents must stop on a cycle.

### Consequences

- Good, because the take rule needs no new mechanism locally (the existing write transaction) and none at the server (its order decides).
- Good, because Links merge per element with no change to `field_versions`.
- Bad, because two sessions sharing an Agent name are told apart only by the Claim token, which the caller must keep.
- Bad, because a take made against a Claim that looked expired can beat a renewal it never saw; Phase 3 decides how renewal is recorded.
- Bad, because a Link across Projects is filed in the source Project's chain only (see Open).

### Confirmation

Fifty processes taking one Issue get one success and refusals naming the winner, with distinct Agent names and with one shared name, on Linux and macOS. Two remote takes with no `prev`, acknowledged in order, rebuild with the first holding the Claim and the second recorded as lost. "A blocks B" written on A and "B is blocked by A" written on B rebuild as one Link, and a merged cycle survives a rebuild.

## More Information

- Research: [claims-and-links.md](../research/workspaces/claims-and-links.md).
- The glossary's Claim, "an Agent's exclusive, expiring hold on an Issue", still holds; the token decides which session under that Agent name is the holder.
- Where "on behalf of" lives, what a Claim records of it, and that only an Agent takes a Claim: [ADR 0010](0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md).
- Open: how renewal is recorded under sync; Links across Projects and to Findings; purge's redaction of payloads.
