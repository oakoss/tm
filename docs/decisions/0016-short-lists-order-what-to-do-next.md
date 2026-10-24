# Short-lists order what to do next

- Status: accepted
- Date: 2026-10-23
- Deciders: Jace Babin

## Context and Problem Statement

Priority says how much an Issue matters, not what to do next. `tm ready` lists every unblocked todo Issue, most urgent first, so an Agent picks from dozens, and two Agents told to work through the same items have no shared order to follow. The design spec plans Iterations, dated periods of work, and Views, saved filters, and neither holds an order a person arranged. How does a human say "these, in this order, by these Agents", and how does an Agent take the next one?

## Decision Drivers

- An Agent should get its next piece of work in one command that cannot collide with another Agent taking the same piece.
- Every change is an operation in the log ([ADR 0005](0005-operation-log-is-the-source-of-truth.md)): attributed, undoable, exported, and later synced and merged ([ADR 0006](0006-operation-merge-ordering-and-versioning.md)), so every rule must hold after a merge or an undo, not only after the write that made it.
- The Assignee stays the accountable human (GLOSSARY); a Short-list says who works through it, not who owns the work.
- No configuration that nothing yet needs.

## Considered Options

- **What holds the order**: a View or saved filter; an Iteration; a Short-list of its own.
- **Where membership lives**: a list record holding an ordered array of Issues; two fields on each Issue.
- **Taking work**: `tm next` prints the next Issue for `tm start`; `tm next` claims and starts it.

## Decision Outcome

Chosen options: **a Short-list of its own; membership in two fields on each Issue; `tm next` claims and starts the Issue it takes**, because only a list a person arranged holds an order no filter computes, per-Issue fields merge by the rules every other field follows, and taking in one step leaves no gap for two Agents to take the same Issue.

### What a Short-list is

- A **Short-list** is a named, ordered list of Issues in one Project with the Actors who work through it, such as `release-0.2` (jace, claude, codex) or `docs` (codex), and a size, 10 unless set when it is created or changed later. It has at least one Actor. Lists across Projects wait for a need.
- An Actor matches a list naming it: an Agent's name matches that Agent whoever it acts for, and a human's name matches that human, not their Agents.
- Names are unique within a Project, compared without case; a list is identified by its id, and a name two Replicas both chose is reported by `tm doctor`, and a command naming it is refused until one is renamed.
- It is neither a View nor an Iteration. A View is a filter computed on every read, an Iteration a dated period; a Short-list is an order someone chose. The filter qualifier `shortlist:NAME` selects a list's Issues, so a View or Board can show one.

### What may be on one

- An Issue sits on at most one Short-list; putting it on another takes it off the first.
- An add refuses an epic, an Issue in triage, an archived Issue and a finished one (done or cancelled): an epic's work is its children, an Issue in triage is not yet accepted, and the others are not work to do next.
- No member comes before an open blocker of it that is also on the list, and an Issue with an open blocker not on the list is refused. Every add and move is checked, in both directions: placing an Issue above its blocker, or a blocker below an Issue it blocks.
- An add that would put the list over its size is refused, naming the list and its size, with the fix to raise the size or remove an Issue. Size is checked only when adding: lowering it below the member count, a merge, an import or an undo may leave a list over it, which `tm shortlist` shows, and adds are refused until it is under again.
- A member that later becomes blocked, claimed, started, snoozed, archived or an epic, or returns to triage, stays and is marked with why, and `tm next` passes it over.

### How it is stored

- **Membership is two built-in Issue fields**: `shortlist`, the list's id, and `shortlist_rank`, a fractional ordering key. A single-value field holds one list at a time, so "at most one" needs no check of its own and survives any merge, and each field merges by per-field last writer wins.
- **Moving an Issue writes only its own rank**, a key between its new neighbours'. Ranks are compared as strings and tie-broken by Issue id, so every Replica sorts a list the same way. A move between two members whose ranks are equal, as two Replicas inserting at once can leave, or whose keys have grown long from repeated inserts in one gap, respaces those members in the same batch.
- **The list is a record of its own**, with its id as the subject of new operation kinds, filed under its Project, that create it, set its name, Actors or size (each merging per field), record its source from an import, and delete it. Deleting keeps the record as a tombstone and takes every member off in the same batch; undoing the delete restores both.
- **A membership naming a deleted or unknown list reads as no list.** A delete cannot clear a membership written concurrently on another Replica, nor one an undo restores after the delete, so every reader treats such a membership as absent and `tm doctor` reports it.
- **Finishing takes an Issue off its list in the same batch, whatever writes it**: when a batch puts an Issue's Status into a done or cancelled category, the store writes `shortlist` and `shortlist_rank` empty in that batch, so a move, `tm git scan` and a record import all do, it shows in history, and undoing the batch restores the Issue to its place.
- **Records carry membership** ([ADR 0013](0013-export-carries-the-log-and-issue-records.md)): an Issue record names its list by id and name with its position, a whole number, and a project line names each list with its id, Actors and size. A record import matches a list by that id first, to a list holding it or carrying it as its source, and by name only when no id matches; it creates a list when neither matches and updates a matched one as it updates fields. A list the import creates carries the recorded id as its source on its create, as an Issue does under ADR 0013; one it matches by name gains the recorded id as a source in an operation of its own, written even when no field differs. A list keeps every source it carries, so a later import from any of those sources still finds it. Matching by id looks for the source on its create and on every source operation, so renaming a list in tm does not split it on re-import. Which list an Issue is on is a field under ADR 0013's re-import rules, compared by the matched list. An Issue the import puts on a list is ranked by its recorded position within that list; a member already there keeps tm's rank, since a rank has no source value to compare, so a reorder made in the source is not carried over by a re-import.
- **An import places members as recorded**, without the refusals an add makes, since it restores state as a merge does: a member in triage, snoozed, an epic, archived or blocked stays and is marked, a list may end over its size or out of blocker order, and a finished Issue still leaves its list in the same batch. Members it adds to a list matched by name are placed by their recorded positions while members already on it keep tm's rank, and differing Actors or size are conflicts under ADR 0013. Its output, `--dry-run` included, names each list matched by name, counts by reason the members `tm next` will pass over, and names any list left over its size.

### Who changes it

- Any Actor may create a list, add, move or remove Issues, and change a list's name, Actors or size, Agents included. Each change is an operation, so `tm history` shows who made it and `tm undo` reverses it, and every new kind has an inverse. Refusing Agents would block the common case of a human telling their Agent to arrange a list; `tm guide` tells Agents to change a list only when their human asks.

### The commands

- `tm shortlist` shows the session Actor's lists, each with its Issues in order, each member `tm next` passes over marked with why, and any list over its size. `tm shortlist create NAME --actor NAME... [--size N]`, `add NAME KEY... [--top | --before KEY | --after KEY]` (the end by default; several Issues are one batch), `move KEY (--top | --before KEY | --after KEY)`, `remove KEY...`, `edit NAME` and `delete NAME` change them. `tm list` keeps its meaning, listing Issues.
- **`tm next`** takes the first eligible Issue on the session Actor's lists: in the todo category, not an epic, not archived, not snoozed, not blocked and not held by a live Claim. It starts it as `tm start` does, so an Agent claims it and a human starts it without a Claim. A take another Agent wins in between moves on to the next eligible Issue rather than failing. `--peek` shows it without taking it; `--json` gives the Issue and its Claim. With none eligible, it says why, counting the members it passed over by reason (`--json` lists each with its reason), and names `tm ready`. It reads the lists on every call, so a reorder lands mid-session. A session whose Actor no list names takes from its Project's ready queue instead, most urgent first, passing over Issues on any Short-list, which another Actor works through; with none eligible there, it names `tm shortlist --all` or `tm list` rather than `tm ready` (amended 2026-10-23, replacing the build plan's `ready --claim`).
- **Which lists, in what order.** The session's lists are those naming its Actor, oldest first. `TM_SHORTLIST`, set in a terminal as `TM_AGENT` is, binds that session to one list by name, so parallel Agents each work their own. One naming none of the session's lists, as after a rename or delete, is an error naming it and the session's lists, and never falls back to all of them.
- **`tm prime` and `tm ready`** show the session's lists first, in their order: each list's next eligible Issue, the first few after it and a count of the rest, with members `tm next` passes over marked with why, so the section fits `tm prime`'s budget.
- **In a sandbox**, where an Agent reads the Snapshot and cannot write the store, `tm next --peek` works and a take does not, as with `tm start`.

### Consequences

- Good, because an Agent asks "what now?" with one command that cannot take what another Agent took.
- Good, because a list's order survives export, restore and, later, sync, by the rules every field already merges by, and rules a merge or undo can break are read rules, not write checks.
- Good, because finishing work keeps lists current with no separate step, and undo puts it back.
- Bad, because the operation format gains kinds for list records, which a tm built before them cannot read.
- Bad, because ranks are ordering keys a reader never sees and respacing writes must keep short.
- Bad, because a re-import places only the members it adds, so a list reordered in another tool keeps tm's order.
- Neutral, because a Workspace-wide default size, lists across Projects, a note on each item, overlap warnings between items, and a human's "waiting on me" view (TM-62) can each be added later without changing what is stored.

### Confirmation

Two Agents running `tm next` against one list at once take different Issues, and neither takes an Issue a human already started. Lists, their order and their members survive `tm export` and `tm import`, and a list renamed in tm keeps its members through a re-import of an unchanged file. Reordering the same list on two Replicas merges to one order every Replica agrees on, and an add merged with a concurrent delete leaves the Issue on no list. A finished Issue leaves its list, whether a move, a scan or an import finished it, and undoing the batch puts it back.

## Pros and Cons of the Options

### A View or saved filter

- Good, because Views are planned anyway.
- Bad, because a filter computes membership and order from fields; "this one, then that one" is an order nobody can express as a filter.

### An Iteration

- Bad, because an Iteration is dated and Project-wide; a Short-list is undated, and several run at once for different Actors.

### An ordered array on the list record

- Good, because one read gives the whole order.
- Bad, because two concurrent reorders of one list conflict as a whole field, and one is lost; "at most one list" would need a check across every list.

### `tm next` only prints the Issue

- Bad, because two Agents read the same next Issue and race `tm start`, and the loser must know to ask again.

## More Information

- The research behind binding an Agent to an ordered list: [parallel-agent-coordination.md](../research/task-tracker-landscape/parallel-agent-coordination.md).
