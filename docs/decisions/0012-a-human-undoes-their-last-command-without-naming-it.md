# A human undoes their last command without naming it

- Status: accepted
- Date: 2026-10-18
- Deciders: Jace Babin

## Context and Problem Statement

[ADR 0010](0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md) has `tm undo <batch>` revert exactly the batch a caller names, and no bare "undo my last command": parallel sub-agents share an Agent's name and session id, so "the last batch" of either can be another session's. A human at a terminal does not keep ids, and every undo tool they know, from an editor to jj, undoes the last thing without one. Batch ids are UUIDv7, whose first 12 hex digits are their time, so batches written within a minute share a prefix and only the end of an id is short and unique. Can a human undo without naming the batch, what does a repeated undo do, and what short form does tm print?

## Decision Drivers

- An Agent must never undo another session's work by accident (ADR 0010).
- A human writing as themself is one writer per name on a Replica; an Agent name is not.
- Every undo can itself be undone, so undo needs no prompt to be safe.
- A short id a human types must be unique among this Replica's batches.

## Considered Options

- **Who gets a bare undo**: the human only; or any writer, by name.
- **What a second bare undo does**: steps back to the next older batch; or toggles, redoing the first.
- **Whether a bare undo asks first**: never; or at a terminal.
- **The short form**: the first characters of the id; or the last.

## Decision Outcome

**A bare undo is the human's.**

- `tm undo` with no batch reverts the newest candidate on this Replica: a batch whose Actor is the current human's name with no `on_behalf_of`, which is not an undo, is not undone, and holds no Project write.
- An Agent writing a bare `tm undo` is refused with `batch_required`, whose fix is `tm undo <batch>`; ADR 0010's reasoning still holds for Agents.
- When the human has no candidate, the bare undo is refused with `not_found`, kind `batch`.
- The newest candidate is undone or refused as it stands, never skipped. A refusal's `fix` names the next older candidate, so the human chooses to step past it.

**A repeated undo steps back.**

- A local table `undo_batches(batch, undid)` records which batch each undo reverted. Like `op_batches`, it is not hashed, synced or replayed, and a rebuild leaves it in place.
- A batch is undone while an undo of it stands, that is, while that undo is not itself undone. Undo batches are never candidates, so each bare `tm undo` reverts the next older batch.
- Redo is undoing an undo by its id, which the undo prints. A redone batch is a candidate again.
- ADR 0010's conflict rule, for a named batch and a bare undo alike, ignores a later batch that is undone, together with the undo standing against it. An undo restores every target its batch wrote, so the pair changes nothing; without this, stepping back past two writes to one field, or undoing a redone batch, is always `undo_conflict`. A write by any other batch still conflicts.
- Only a later pair is ignored. A pair whose undone batch is older than the one being undone straddles it, and ignoring it lets a redo overwrite a newer value. Measured with [undo_pairs.py](../research/cli/scripts/undo_pairs.py) over 20,000 random sequences on one field: ignoring any undone pair left 71 with a value no replay of the writes in effect gives; ignoring only later pairs left 0 and applied 37,964 undoes, against 34,360 under ADR 0010's rule.

**No prompt.** A bare undo writes, lists what it restored, and prints how to undo it in turn. `--dry-run` previews it.

**Short ids are the end.**

- Every write's text output that can be undone ends with `undo this with: tm undo <id>`, giving the id's last 12 hex digits. `--json` keeps the full id in `batch`.
- `tm undo` takes a full id, or a unique start or end of at least 8 hex digits, and refuses a part that matches several batches.

### Consequences

- Good, because a human undoes as in any editor, without keeping ids.
- Good, because an Agent still cannot undo another session's batch, since it must name one.
- Good, because the last 12 hex digits of a UUIDv7 come from its random part, so a clash is unlikely among a Replica's batches, and one is refused, never guessed.
- Bad, because a human with two terminals open on one Replica undoes the newest batch of either, and an Agent named as the human is recorded as the human (ADR 0010), so its batches are candidates too.
- Bad, because the step-back record is local, so it is lost when a store is recreated, as `op_batches` is.
- Neutral, because jj steps back by restoring a whole earlier state, while tm restores only what each batch wrote, which is why the conflict rule has to see undone pairs as nothing.

### Confirmation

Two bare undoes after two moves of one Issue restore its first Status, skipping an Agent's batch between them. A bare undo by an Agent is `batch_required`. Undoing an undo by its printed id makes the batch it undid a candidate again, and a bare undo then reverts it.

## More Information

- Supersedes, for a human writing as themself, ADR 0010's "There is no bare 'undo my last command'", and narrows its conflict rule as above; the rest of ADR 0010 stands.
- Prior art: jj's `undo` walks back its operation log, and redo restores (`cli/src/commands/undo.rs`, in [core-commands.md](../research/cli/core-commands.md)).
