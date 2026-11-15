# Custom Link types are directional, and the built-in types stay fixed

- Status: accepted
- Date: 2026-11-15
- Deciders: Jace Babin

## Context and Problem Statement

Phase 4 lets a Scheme define Link types, each with an outward name, an inward name and readiness behaviour (design.md). A Link is filed under its canonical source when written, a symmetric type ordering its two Issue ids by bytes ([ADR 0009](0009-claims-apply-by-compare-and-set-and-links-are-keyed-canonically.md)), and apply refuses a symmetric Link filed otherwise, so applying a Link reads whether its type is symmetric. Replay must not read the Scheme ([ADR 0007](0007-the-projection-does-not-depend-on-the-scheme.md)). How can a Scheme define Link types without making replay depend on it?

## Decision Drivers

- A rebuild produces the same projection from the same log, whatever the Scheme (ADR 0007).
- The same Link written from either end, on any Replica, is one element (ADR 0009).
- Operations are never rewritten, so a Link operation's format is fixed once written.
- The readiness behaviours design.md lists (blocks, waits for all children, conditional on failure, none) each read from one end to the other.

## Considered Options

- **Directional only**: a custom Link type is always directional; the six built-in types, the only symmetric one among them included, stay defined in code.
- **Symmetry in the log**: each Link operation records whether its type is symmetric, and apply reads that.
- **Reopen ADR 0007**: let apply read the Link types from the Scheme.

## Decision Outcome

Chosen option: **directional only**, because it keeps replay free of the Scheme with no change to the Link operation, and every readiness behaviour design.md asks for has a direction.

- **A Link type a Scheme defines is directional.** It is stored outward only, as `blocks` is, so apply never needs to know it; symmetry belongs to the built-in types alone, which apply reads from code (`tm_core::link::BUILT_IN`), as it does today.
- **The six built-in types are fixed in code**: blocks, relates to, duplicates, derived from, causes, supersedes. A Scheme may not redefine them or their readiness: `blocks` holds an Issue back and the rest do not. A built-in type added later is directional, or takes an id no Scheme can hold, so no Link already written changes how it applies.
- **No two types share a name**, built-in or custom, as `tm_core::link::by_name` reads names: its id, outward name or inward name, without case and with `_` or `-` read as a space. A name always means one type.
- **A custom Link type's readiness behaviour comes from the Scheme when read**, as a Status category does (ADR 0007); it is never stored in the projection.
- **The Scheme checks a Link type when this Replica writes the Link**, as it checks other Scheme ids. A pulled Link whose type the Scheme does not define is kept and read as a directional Link with no readiness behaviour.

### Consequences

- Good, because the Link operation's format and the projection's `links` table do not change.
- Good, because apply's rule stays the one it has today: only a built-in type is known to be symmetric.
- Bad, because a team cannot add a symmetric custom type, such as "pairs with"; `relates to` is the one symmetric type.
- Neutral, because a custom type that reads the same both ways can still be defined with equal outward and inward names; it is stored outward, so "A pairs with B" and "B pairs with A" are two Links.

### Confirmation

Revisit if a Preset needs a symmetric type that `relates to` cannot stand for; recording symmetry in the Link operation is the path then, as a new operation version.

## More Information

- The readiness behaviours, and which Link types hold an Issue back, are Phase 4's Link story (TM-194's review, candidate 4).
