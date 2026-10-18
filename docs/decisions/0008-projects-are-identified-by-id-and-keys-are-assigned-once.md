# Projects are identified by id, and Keys are assigned once

- Status: accepted
- Date: 2026-10-17
- Deciders: Jace Babin

## Context and Problem Statement

Every operation's hashed envelope names its Project ([ADR 0006](0006-operation-merge-ordering-and-versioning.md)), and stored operations are never rewritten. With the Project key (`CPL`) in that slot, a Project could never be renamed. Project identity also has to survive a rebuild and reach other Replicas once sync exists, and a Key, once given, must never be given again ([ADR 0004](0004-one-authority-assigns-keys.md)). How are Projects identified and recorded, and how are Keys kept stable?

## Decision Drivers

- Logged operations are immutable and hash-verified, so whatever they reference must never need to change.
- A rebuild must reproduce every Project and every Key exactly.
- A Key must never be reused, including after an Issue is removed from view or purged.
- No change to the hashed envelope's layout.

## Decision Outcome

- **A Project has a permanent id**, a UUID. Operations put that id, in its canonical text form, in the envelope's Project slot. The Project key and name are labels that can change.
- **An operation has a subject**: the envelope slot ADR 0006 calls the Issue UUID names whatever the operation changes, an Issue or a Project. The kind says which: Project operations have their own kinds (`create_project`, `rename_project`), and for them the subject and the Project are both the Project's own id. The bytes the hash covers are unchanged.
- **Projects are recorded in the log.** Creating a Project and changing its key or name are operations, so a rebuild and, later, sync carry them like any other change.
- **A Project key** matches `^[A-Z][A-Z0-9]{1,9}$`, is accepted in any case and stored uppercase, and is unique within a Workspace, retired keys included, so an old key keeps resolving and never passes to another Project.
- **A Key is assigned once, when its Issue is created**, in the same `BEGIN IMMEDIATE` transaction, and recorded on the `create` operation outside the hash (`assigned_key`, the column server effects use). Replay reads the stored Key and never recomputes it: recomputing from another log order changed 490 of 500 Keys in the research. Once written, the column cannot change.
- **Renaming a Project key** prefixes new Issues' Keys with the new key; existing Keys keep resolving as permanent aliases, as provisional Keys do.
- **Purge keeps each purged Issue's `create` operation and its Key**, so the number is never handed out again. The rest of purge needs its own decision.
- **ADR 0006's kind list is clarified:** archiving is `set_field` on the built-in `archived` field, and a `purge` kind is defined with purge itself.

### Consequences

- Good, because Projects can be renamed without touching the log, and history keeps naming the Project correctly.
- Good, because Keys survive rebuilds, renames and purges.
- Bad, because every command resolves a typed Project key to its id, and reading history shows the key current when read, not the one used when written, unless the operation also records it.
- Neutral, because the envelope keeps its layout; the slot's meaning is what changes.

### Confirmation

A rebuild test reproduces every Project and Key; a test removes the highest-numbered Issue from the tables and checks the next Key still counts past it; and the write-once trigger refuses a changed `assigned_key`.

## More Information

- Research: [workspaces-and-keys.md](../research/workspaces/workspaces-and-keys.md), [claims-and-links.md](../research/workspaces/claims-and-links.md).
- Supersedes ADR 0006's reading of the Project slot as the Project key and of the Issue slot as always an Issue.
