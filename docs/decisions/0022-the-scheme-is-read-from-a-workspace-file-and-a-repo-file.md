# The Scheme is read from a Workspace file and a repo file

- Status: accepted
- Date: 2026-11-15
- Deciders: Jace Babin

## Context and Problem Statement

Every command runs on the Scheme compiled into `Scheme::built_in()`. Phase 4 reads it from config ([ADR 0007](0007-the-projection-does-not-depend-on-the-scheme.md)): design.md layers Workspace defaults under Project overrides from the repo file, and [ADR 0003](0003-issue-data-lives-outside-code-repos.md) lets the repo file carry a Project's Scheme overrides. Where do the two layers live, in what shape, how does an override combine with what it overrides, and are fields defined per Issue type or for the whole Scheme?

## Decision Drivers

- A binary that predates the Scheme file keeps working in a repo that has one. `RepoFile` refuses keys it does not know: a `[scheme]` table added to `.config/tm.toml` made the installed `tm list` exit 1 with "unknown field `scheme`".
- One repo file can map paths to several Projects (`paths`), so repo overrides name the Project they change.
- `field_values` stores a value by `(issue_id, field)`, with no type, so a field id means one field across every type.
- design.md lets any default type be removed, so an override must be able to drop what it inherits.
- The Scheme change preview (TM-218) compares the Scheme before and after an edit, so each layer resolves to one whole Scheme.
- Per-machine values never go in a tracked file.

## Considered Options

- **Two files**: Workspace defaults in the Workspace directory, Project overrides in a file of their own beside the repo file.
- **A key in the repo file**: the overrides as a `[scheme]` table in `.config/tm.toml`.
- **Workspace only**: one Scheme file in the Workspace directory, with per-Project sections.

## Decision Outcome

Chosen option: **two files**, because it is the layering design.md describes, it keeps older binaries working, and a Project's Scheme travels with its repo and is reviewed in its pull requests. It amends ADR 0003, whose repo holds one config file carrying the Scheme overrides: the overrides move to a file of their own.

- **Workspace defaults live in `scheme.toml` in the Workspace directory** (`~/.tm/workspaces/<id>/scheme.toml`), a repo-scoped Workspace's included. With no file the built-in Scheme is the Workspace's.
- **Project overrides live in `.config/tm/scheme.toml`** under the directory the repo file maps from: the one holding `.tm.toml`, or holding the `.config/` that holds `tm.toml`. `.config/tm.toml` gains no key, so an older binary ignores the file. A repo mapped only by its git remote has no repo file, and no overrides are read for it. Its top-level tables are keyed by Project key (`[projects.TM]`), since one repo file can map several Projects; a Project the repo file does not map is refused when the file is read.
- **The repo's Scheme file is tracked, so per-machine values never go in it**, as for the repo file: no Scheme key holds a host, URL, token or path that differs between machines.
- **Both files share one shape**: tables of `types`, `workflows`, `fields` and `link_types`, each keyed by id, and a `remove` table listing inherited ids to drop. The Workspace file holds them at the top level; the repo file holds them under each Project. The built-in Link types are fixed in code and can be neither redefined nor removed ([ADR 0021](0021-custom-link-types-are-directional.md)).
- **Each layer merges over the one below it by id**: the Workspace file over the built-in Scheme, a Project's tables over the Workspace's. A layer adds an id it does not inherit, replaces one it does, whole, and drops inherited ids it lists in its `remove` table (`remove.types = ["spike"]`). The result is one resolved Scheme per Project, which the preview diffs.
- **Fields are defined once for the whole Scheme**, with Scheme-wide ids, and each Issue type lists the fields it carries. A field a type does not carry is not offered for it.
- **The Scheme a command uses** is the Workspace layer merged with the overrides of the Project the command acts on, read from the repo it runs in. A command outside any repo uses the Workspace layer.
- **Two repos mapping one Project may disagree.** Each applies its own overrides when run there. Since the Scheme is checked when this Replica writes and read when shown (ADR 0007), disagreement shows as Violations, never as a different projection.
- **A nested Project inherits its parent's resolved Scheme** before its own overrides. Projects do not nest yet; the rule holds once they do.
- **A malformed Scheme stops every command that reads it but `tm doctor`**, naming the file, line and fault, rather than falling back to the built-in Scheme and writing under rules nobody configured.

### Consequences

- Good, because a Project's Scheme is versioned and reviewed with the code that relies on it.
- Good, because an older binary still runs in a repo with a Scheme file; it applies the built-in Scheme, and what it writes against the configured one shows as Violations.
- Good, because the Workspace layer gives commands outside any repo, and Projects with no override, one Scheme.
- Bad, because every command reads up to two more files before it acts, inside the cold-start budget `mise run ci` holds.
- Bad, because two checkouts of one Project can apply different Schemes when their branches differ.
- Neutral, because the Workspace file is per Replica until sync carries it.

### Confirmation

Revisit if reading the Scheme pushes a command past the cold-start budget, or if Projects mapped from several repos routinely disagree.

## Pros and Cons of the Options

### A key in the repo file

- Good, because there is one file to find.
- Bad, because a binary that predates the key refuses the whole repo file, as measured above.

### Workspace only

- Good, because every directory applies the same Scheme.
- Bad, because it overrides ADR 0003's repo-carried overrides, is not reviewed with the code, and does not travel with the repo.

## More Information

- The Scheme file is found by path, with no `include` key; design.md's `include` for splitting other config is not decided here.
- Open: how the Workspace file reaches another Replica, and what `tm sync status` says when two Replicas' Schemes differ ([research](../research/fields/scheme-and-workflow.md), open questions). Sync is not built.
- The keys inside each table are settled by the stories that use them (TM-205, TM-208 onward).
