# Memories are records of their own

- Status: accepted
- Date: 2026-10-30
- Deciders: Jace Babin

## Context and Problem Statement

An Agent's session ends and its next one starts cold. Some of what it learned is not about one Issue but about the Project: the tests need `-race`, deploys go through staging, a file is generated and never edited by hand. The design spec plans `tm remember "<insight>"`, stored as project memory that `tm prime` shows, as beads' `bd remember` does. Where does a memory live, how is it named and changed, and how much of it reaches every session?

## Decision Drivers

- Every change is an operation in the log ([ADR 0005](0005-operation-log-is-the-source-of-truth.md)): attributed, undoable, exported, and later synced.
- Everything is soft-deleted; nothing is summarised away.
- `tm prime` stays within its budget at any size.
- An Agent writes most memories, so a person must be able to see who wrote one and when, and nothing an Agent writes may silently replace what someone else wrote.

## Considered Options

- **What a memory is**: an Issue of a memory type; a comment on the Project; a record of its own, as a Short-list is ([ADR 0016](0016-short-lists-order-what-to-do-next.md)).
- **How one is named**: by its id; by a key made from its first words, which a named key may replace.

## Decision Outcome

Chosen options: **a record of its own in one Project, named by a key**, because an Issue would land in `tm list`, `tm ready` and `tm next` and need a Status and type that mean nothing for a fact, a Project has no comments, and a key reads in `tm prime` and types back into `tm forget` where an id does not.

### What a memory is

- A **memory** holds a key and a body in one Project, with who last wrote the body and when. Its key and body each merge as an Issue's fields do; a forgotten mark keeps its record, so `tm undo` brings it back.
- A key is 1 to 64 lowercase letters, digits or `-`. Without `--key`, `tm remember` makes one from the body's first words and, when a live memory holds it, numbers it (`tests-need-race-2`), so an unnamed write never replaces anything. With `--key`, it replaces the body of the live memory holding that key, saying what it replaced and who wrote it, or brings back a forgotten one.
- Keys are unique among a Project's live memories; a write refuses one another holds, and a merge that leaves two holding one keeps both until one is renamed or forgotten.
- A body is at most 500 characters and not blank: `tm prime` shows its first line, so a longer note belongs in a comment or a doc.
- A memory that reads much like another (most of its words shared) is written all the same, and the command names the other, so the writer can update it instead.

### The commands

- `tm remember "insight" [--key KEY]`, `tm memories [WORDS | KEY]` (one in full by its key, or those holding every word) and `tm forget KEY`.
- **`tm prime`** shows the 10 most recently written, each as its key, its first line, who wrote it and how long ago, and counts the rest.
- **Export** carries each live memory on its Project's line, and record import finds it as it finds a Short-list: by source, on the memory's own id or an earlier import's create or source operation, then by a key one memory holds on each side, recording the source on a memory it matched by key so a later import finds it even forgotten or rekeyed. A memory forgotten here is not made again, an import may bring one sharing a key as a merge does, and a key change to a memory already here must still leave its key unshared; a body tm and the source both changed keeps tm's and is reported.

### Consequences

- Good, because a fact an Agent learns reaches every later session in the Project, attributed and dated, so a stale one reads as old.
- Good, because a memory is undone, exported and merged by the rules every record already follows.
- Bad, because the operation format gains kinds for memories, which a tm built before them cannot read.
- Bad, because memories stay in this machine's store, not in git: until sync, another machine sees them only through `tm export` and `tm import`, and a fact the whole repository needs still belongs in AGENTS.md.
- Bad, because an important memory falls out of `tm prime` once ten newer ones are written (TM-113 pins one).
- Neutral, because memories shown only where they apply (TM-114) and memories across a Workspace's Projects (TM-115) can be added without changing what is stored.

### Confirmation

A memory written by one session shows in the next session's `tm prime`. An unnamed write never changes another memory. Forgetting and undoing restores the memory, and memories survive `tm rebuild`, `tm export` and a record import into a fresh store, a second import writing nothing.
