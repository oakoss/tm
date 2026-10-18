# Operations record who an Agent acts for, and undo names a batch

- Status: accepted
- Date: 2026-10-18
- Deciders: Jace Babin

## Context and Problem Statement

design.md records an Agent's writes as "agent, on behalf of human", and `tm undo` must revert one command's writes. The log has one `actor` slot, and no record of which operations one command wrote: `create_issue` writes four, and their timestamps are not unique to a command. [ADR 0006](0006-operation-merge-ordering-and-versioning.md) says `op_hash` covers every field the Replica writes, and that old operation versions are translated when read, but no translation exists yet. The `link` and `claim` kinds were added without a version bump, so a binary from before them fails on an unknown variant instead of asking for an upgrade. Where does "on behalf of" live, how does the log format change for the first time, how does tm tell an Agent from a human, and what does undo revert? The research is in [core-commands.md](../research/cli/core-commands.md).

## Decision Drivers

- Every field the Replica writes is hashed (ADR 0006), and a stored operation is never rewritten.
- The switch milestone imports this project's backlog during Phase 1, after which a format change is a permanent translation.
- A binary that meets an operation it cannot read must ask for an upgrade, not fail on parse.
- Parallel sub-agents of one Agent share its session id. Measured: a Claude Code sub-agent sees the parent's `CLAUDE_CODE_SESSION_ID`, and Codex's `CODEX_SESSION_ID` is "the shared root-session identity".
- A human in an IDE terminal must not be recorded as an Agent.

## Considered Options

- **Where "on behalf of" lives**: in the hashed envelope beside `actor`; or in each kind's payload.
- **How a command's operations are grouped**: a batch id in the hashed envelope; or a local table outside the log.
- **What undo targets**: the Actor's last batch; the session's last batch; or a batch the caller names.
- **How a Claude Code Agent is detected**: also by `CLAUDECODE`; or only by variables Claude Code sets for its own subprocesses.

## Decision Outcome

**Format v2.**

- `Op` gains `on_behalf_of`, and `CURRENT_VERSION` becomes 2. Nothing else joins the envelope.
- The envelope branches on `v`. A `v: 1` operation hashes to exactly the bytes it always did, with envelope format 1. From `v: 2`, envelope format 2 adds `on_behalf_of` after `actor`, with a presence byte.
- A stored `v: 1` operation reads as `on_behalf_of` empty; this is the first of ADR 0006's read-time translations. A `v: 1` operation carrying `on_behalf_of` is refused, because its hash would leave the field out.
- From `v: 2`, an `actor` or `on_behalf_of` that is blank or holds a control character is refused, on write and on replay; `v: 1` operations replay as they were written.
- **Every new `Change` kind bumps `CURRENT_VERSION`**, enforced by a test that ties each kind to the version that introduced it. A binary then refuses an unknown kind by its number, with "upgrade tm".

**Migrating the log.**

- A new operation field is a nullable column on `ops`, listed in the `ops_immutable` trigger.
- `STORE_FORMAT` becomes 2. Opening a format-1 store adds the column, recreates the trigger and stamps the format in one transaction; an older binary refuses the result through the existing newer-store guard. Later format changes follow the same pattern.

**Identity.**

- The human is `TM_ACTOR`, else the name set with `tm whoami`, else git `user.email`. `TM_ACTOR` names the human only; it never names an Agent.
- An Agent is detected only from variables its harness sets, in this order, and recorded under a fixed name:

  | Variable | Name |
  |---|---|
  | `CODEX_SESSION_ID` or `CODEX_THREAD_ID` | `codex` |
  | `CLAUDE_CODE_CHILD_SESSION` or `CLAUDE_CODE_SESSION_ID` | `claude` |
  | `GEMINI_CLI` | `gemini` |
  | `OPENCODE` | `opencode` |
  | `CURSOR_AGENT` (unverified) | `cursor` |

- `TM_AGENT=<name>` names an Agent detection misses, and overrides a detected name, so an orchestrator can give each sub-agent its own.
- When variables from more than one harness are present, as when one Agent launches another, the first in the order wins, and a write carries a warning naming both and `TM_AGENT`. Variables are inherited, so only the inner Agent sees both sets, and no order is right for every nesting. Codex comes first because the expected nesting is Claude Code launching Codex, as a reviewer; Claude Code launched from a Codex shell is the case it misnames.
- Sub-agents of one Claude Code session share its variables, so detection names them all `claude`. In Phase 3, `tm setup claude` installs a hook that names each sub-agent from the `agent_id` Claude Code passes to hooks; until then an orchestrator sets `TM_AGENT` per sub-agent.
- `CLAUDECODE` and `AI_AGENT` are not read. IDE extensions set `CLAUDECODE` in their integrated terminals, and Claude Code's own tools also see its session id; `AI_AGENT`'s values carry versions and are inherited from parent agents.
- With an Agent, `actor` is the Agent's name and `on_behalf_of` is the human, unless the two are equal. Without one, `actor` is the human and `on_behalf_of` is empty. `on_behalf_of` is the human running the Agent, not the Issue's Assignee.
- `@me` resolves to the human in both cases.
- A take's `agent` is the resolved Agent name, the operation's `actor`. The claims table records `on_behalf_of` in place of `actor`, so a Claim shows the Agent and the human it works for. A take with no `on_behalf_of` was written by that human, as every `v: 1` take was, so the column falls back to the writer.

**Undo.**

- Every write gets a batch id, its first operation's id, kept in a local table `op_batches(op_id, batch)` outside the log and the projections. It is not hashed or synced, and a rebuild neither reads nor drops it; taskchampion's local `UndoPoint` is the precedent.
- Every write's output carries its batch id, and `tm undo <batch>` reverts exactly that batch by appending operations that restore each touched field and set element to its state before the batch. It is refused with `undo_conflict`, naming the current value, when a later batch wrote the same field or element. There is no bare "undo my last command"; the caller keeps the id, as it keeps a Claim's.
- Until sync is designed, undo is refused for a batch holding an acknowledged operation. Undo after sync is designed with sync.

### Consequences

- Good, because every stored v1 hash stays valid, and the translation is exercised before the backlog import makes it permanent.
- Good, because `link`, `claim` and every later kind, once written as `v: 2` or later, are refused by version number by a binary that predates them. `v: 1` ones exist only in development stores.
- Good, because undo works the same for humans, detected Agents, undetected Agents and parallel sub-agents.
- Bad, because an Agent tm cannot detect, including one writing from a tmux session Claude Code started, records as the human unless it sets `TM_AGENT`.
- Bad, because until Phase 3's hook, sub-agents of one Agent share its name unless each sets `TM_AGENT`. Claims still tell them apart by token.
- Bad, because an Agent launched by another may be recorded under its parent's name; the warning says so, and `TM_AGENT` corrects it. Codex's default `shell_environment_policy.inherit` is `all` (`codex-rs/config/src/shell_environment_policy.rs`, v0.163.0), so an Agent launched from another sees both sets by default (inferred from source for Codex). Measured: a Codex launched from Claude Code with `inherit = "core"` saw only Codex's variables.
- Bad, because a caller that loses the batch id cannot undo.
- Neutral, because batches are lost when a store is recreated from the server, which is acceptable while undo is local.

### Confirmation

A `v: 1` sample keeps the pinned hash, and removing the `v: 1` envelope branch fails that test. A `v: 2` sample has its own pinned hash, and the hash changes with `on_behalf_of`. A `v: 1` operation carrying `on_behalf_of` is refused. A format-1 store opens under the new binary, upgrades, rebuilds with `on_behalf_of` empty and keeps every `op_hash`. A kind added without a version bump fails the build. `tm undo` of a batch that wrote a duplicate Link add leaves the Link in place.

## More Information

- Research: [core-commands.md](../research/cli/core-commands.md), sections 3 and 4.
- Answers ADR 0009's open question of where "on behalf of" lives.
- Open: whether a human with no Agent identity may take a Claim; how `create` and `comment` are undone; what undo means once operations sync.
