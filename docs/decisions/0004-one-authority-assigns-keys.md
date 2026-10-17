# One authority assigns Keys

- Status: accepted
- Date: 2026-10-16

Every Issue has a permanent UUID; its Key (`CPL-42`) is a sequential display identifier assigned by exactly one authority: the local store in a local-only Workspace, or the sync server once sync is on. A synced machine that is offline gives new Issues a provisional Key (`CPL-~k3f`) that is replaced on the next sync and kept as a permanent alias, and returning to local numbering needs an explicit `tm sync detach` so two machines never number at once. This keeps Jira-style ordered, short Keys without the duplicate-Key collisions Backlog.md hit with per-clone sequential numbering.

## Considered Options

- **Hash-suffix Keys, as beads uses**: never collide and work offline, but lose creation ordering.
- **Number blocks reserved per machine**: work offline, but leave gaps and break ordering across machines.

## Consequences

- Issues created offline on two machines are numbered in the order they reach the server; created-at timestamps stay exact.
- Links, parents and history reference UUIDs, so renumbering on a Workspace merge never breaks structure.
