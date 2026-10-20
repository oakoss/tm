# Invariants

Rules that override agent and tool defaults. `tm` must obey them, and so should anyone changing this repository.

**tm writes only what it owns.** It never runs `git commit`, edits `AGENTS.md` or `CLAUDE.md` only to add its own lines, shown first and confirmed by a human at a terminal ([ADR 0014](../decisions/0014-tm-adds-its-instruction-lines-only-on-a-humans-yes.md)), never changes git config (including `core.hooksPath`), never writes per-machine values (hosts, URLs, tokens) into a tracked file, and writes anything outside its Workspace only after a confirmed diff. Anything else is printed, never performed.

**Never collapse "we didn't look" into "it's fine."** A check that could not run reports that it could not, distinctly from a check that ran and found nothing. `tm sync status` that cannot reach the server is not "synced"; a Finding whose code could not be read is not "fixed".

**A new test proves nothing until it has failed.** Revert the fix on a copy and watch the test go red before shipping; a test that passes either way is reading something other than the fix.

**Config expresses preference; facts are derived.** Before adding a config key, establish that it describes a preference rather than something readable from the repository or the store. A key that restates a fact will rot.

**Decisions are superseded, not ignored.** Code that contradicts a record in [docs/decisions/](../decisions/) needs a new decision that supersedes it.
