# Operations record who an Agent acts for, and undo names a batch

- Status: accepted; its bare-undo and conflict rules are amended by [ADR 0012](0012-a-human-undoes-their-last-command-without-naming-it.md)
- Date: 2026-10-18
- Deciders: Jace Babin

## Context and Problem Statement

design.md records an Agent's writes as "agent, on behalf of human", and `tm undo` must revert one command's writes. The log has one `actor` slot, and no record of which operations one command wrote: `create_issue` writes four, and their timestamps are not unique to a command. [ADR 0006](0006-operation-merge-ordering-and-versioning.md) says `op_hash` covers every field the Replica writes, and that old operation versions are translated when read. Where does "on behalf of" live, when must a format change keep old stores readable, how does tm tell an Agent from a human, and what does undo revert? The research is in [core-commands.md](../research/cli/core-commands.md).

## Decision Drivers

- Every field the Replica writes is hashed (ADR 0006), and a stored operation is never rewritten.
- tm is unreleased: every store is a development store, and keeping old formats readable now buys nothing but code to keep.
- A binary that meets an operation format it does not know must ask for an upgrade, not fail on parse.
- Parallel sub-agents of one Agent share its session id. Measured: a Claude Code sub-agent sees the parent's `CLAUDE_CODE_SESSION_ID`, and Codex's `CODEX_SESSION_ID` is "the shared root-session identity".
- A human in an IDE terminal must not be recorded as an Agent.

## Considered Options

- **Where "on behalf of" lives**: in the hashed envelope beside `actor`; or in each kind's payload.
- **How a command's operations are grouped**: a batch id in the hashed envelope; or a local table outside the log.
- **What undo targets**: the Actor's last batch; the session's last batch; or a batch the caller names.
- **How a Claude Code Agent is detected**: also by `CLAUDECODE`; or only by variables Claude Code sets for its own subprocesses.

## Decision Outcome

**The format until v1.**

- `Op` gains `on_behalf_of`, hashed after `actor` behind a presence byte, and is a nullable `ops` column covered by the `ops_immutable` trigger. Nothing else joins the envelope except an imported operation's `source` ([ADR 0013](0013-export-carries-the-log-and-issue-records.md)).
- An `actor` or `on_behalf_of` that is blank or holds a control character is refused, on write and on replay.
- Operations keep their format version `v`, and stores their `STORE_FORMAT`; a binary still refuses a newer one with "upgrade tm".
- Until tm v1 is released, the format changes in place: `CURRENT_VERSION` and `STORE_FORMAT` stay 1, nothing translates old operations or migrates old stores, and a development store an older build wrote is recreated. This defers ADR 0006's read-time translations to v1. Data that must outlive a format change before then, such as this project's backlog after the switch milestone, moves through `tm export` and `tm import`, as its Issue records ([ADR 0013](0013-export-carries-the-log-and-issue-records.md)).
- From v1, every format change bumps `CURRENT_VERSION` (every new `Change` kind included, so an older binary refuses it by number), old operations are translated when read, and old stores are migrated when opened.

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
- Sub-agents of one Claude Code session share its variables, so detection names them all `claude`. `tm setup claude` installs a PreToolUse hook that names each sub-agent from the `agent_type` and `agent_id` Claude Code passes to hooks, as `<agent_type>-<first 8 of agent_id>`: for a sub-agent's Bash command that runs tm, it prefixes `export TM_AGENT=<name>;`, and the heartbeat and SubagentStop hooks act for the sub-agent their input names. Measured on Claude Code 2.1.311: no agent id reaches a sub-agent's Bash environment, and permission rules match the `export …;` form but not a `TM_AGENT=… cmd` prefix. Only commands naming tm are rewritten, since two hooks rewriting one command do not merge and one is dropped; a tm a script runs, or a rewrite another hook wins, writes as `claude`. The hook runs tm once per Bash call, main thread included; a release build answered in about 0.01 s. A hook exiting 2 blocks the Bash call, or keeps a sub-agent from stopping, so the hooks a tm older than them would not know end in `|| exit 1`.
- `CLAUDECODE` and `AI_AGENT` are not read. IDE extensions set `CLAUDECODE` in their integrated terminals, and Claude Code's own tools also see its session id; `AI_AGENT`'s values carry versions and are inherited from parent agents.
- With an Agent, `actor` is the Agent's name and `on_behalf_of` is the human, unless the two are equal. Without one, `actor` is the human and `on_behalf_of` is empty. `on_behalf_of` is the human running the Agent, not the Issue's Assignee.
- `@me` resolves to the human in both cases.
- Only an Agent takes a Claim, and the take names the Agent writing it; a human's own work is tracked through the Assignee. Both are checked when written, so takes already in the log replay. The claims table records `on_behalf_of` in place of `actor`, so a Claim shows the Agent and the human it works for. A take with no `on_behalf_of` was written by an Agent named as its human, so the column falls back to the writer.

**Undo.**

- Every write gets a batch id, its first operation's id, kept in a local table `op_batches(op_id, batch)` outside the log and the projections. It is not hashed or synced, and a rebuild neither reads nor drops it; taskchampion's local `UndoPoint` is the precedent.
- Every write's output carries its batch id, and `tm undo <batch>` reverts exactly that batch by appending operations that restore each touched field and set element to its state before the batch. It is refused with `undo_conflict`, naming the current value, when a later batch wrote the same field or element. There is no bare "undo my last command"; the caller keeps the id, as it keeps a Claim's. [ADR 0012](0012-a-human-undoes-their-last-command-without-naming-it.md) supersedes this for a human writing as themself, and has the conflict rule ignore a later batch that is undone, together with the undo standing against it.
- Until sync is designed, undo is refused with `cannot_undo` for a batch holding an acknowledged operation. Undo after sync is designed with sync.
- What has no prior state is undone by its nearest reversible write. Undoing a create archives the Issue, since only `tm purge` removes one. Undoing a comment retracts it, which only the Actor who wrote it, or the human an Agent wrote it for, may do; undoing the retraction shows it again. A batch that only released a Claim, a Project write, and a batch whose writes all hold their prior state already are refused with `cannot_undo`. A release made with other writes, as a move to done makes ([ADR 0009](0009-claims-apply-by-compare-and-set-and-links-are-keyed-canonically.md)), is left in place: undo reverts the other writes and says the Claim stays released, since only its Agent can take the Issue again.

### Consequences

- Good, because no code keeps old formats readable before anyone holds data in them.
- Bad, because until v1 a binary that predates a kind, such as `link` or `claim`, fails on parse instead of asking for an upgrade, and a store from an older build must be recreated.
- Good, because undo works the same for humans, detected Agents, undetected Agents and parallel sub-agents.
- Bad, because an Agent tm cannot detect, including one writing from a tmux session Claude Code started, records as the human unless it sets `TM_AGENT`.
- Bad, because without `tm setup claude`'s hooks, and in other harnesses, sub-agents of one Agent share its name unless each sets `TM_AGENT`. Claims still tell them apart by token.
- Bad, because an Agent launched by another may be recorded under its parent's name; the warning says so, and `TM_AGENT` corrects it. Codex's default `shell_environment_policy.inherit` is `all` (`codex-rs/config/src/shell_environment_policy.rs`, v0.163.0), so an Agent launched from another sees both sets by default (inferred from source for Codex). Measured: a Codex launched from Claude Code with `inherit = "core"` saw only Codex's variables.
- Bad, because a caller that loses the batch id cannot undo.
- Neutral, because batches are lost when a store is recreated from the server, which is acceptable while undo is local.

### Confirmation

A human's and an Agent's operation each have a pinned hash, and the hash changes with `on_behalf_of`. An Agent's write stores and replays its `on_behalf_of`. `tm undo` of a batch that wrote a duplicate Link add leaves the Link in place.

## More Information

- Research: [core-commands.md](../research/cli/core-commands.md), sections 3 and 4.
- Answers ADR 0009's open question of where "on behalf of" lives.
- Open: what undo means once operations sync.
