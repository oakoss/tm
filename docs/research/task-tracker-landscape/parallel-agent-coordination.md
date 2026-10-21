# Coordinating several AI coding agents at once

- Date: 2026-10-21
- Author: claude, for jace
- Scope: how current trackers and agent runners hand work to several agents in parallel, keep two off one task, isolate them, tell sessions apart and order work; an input to Short-lists (TM-59).

## Question

tm is designing Short-lists: named, ordered lists of Issues that humans and Agents are assigned to (TM-59). For several agent terminals at once, the open questions are:

- how work is split across them;
- how two agents are kept off one task;
- how an agent picks the next task;
- how sessions of one agent tool are told apart;
- how file and merge conflicts are avoided.

The companion note [agent-first-trackers.md](agent-first-trackers.md) covers single trackers; this one covers coordination.

## Sources

Read on 2026-10-21. Where a detail rests only on third-party sources, or on none found, it is marked **unverified**.

1. beads CLI reference: <https://github.com/gastownhall/beads/blob/main/docs/CLI_REFERENCE.md>
2. beads multi-agent coordination: <https://github.com/gastownhall/beads/blob/main/docs/multi-agent/coordination.md>
3. beads worktrees: <https://github.com/gastownhall/beads/blob/main/docs/reference/worktrees.md>
4. Task Master README: <https://github.com/eyaltoledano/claude-task-master>
5. Task Master task service: <https://github.com/eyaltoledano/claude-task-master/blob/main/packages/tm-core/src/modules/tasks/services/task-service.ts>
6. Backlog.md README: <https://github.com/MrLesk/Backlog.md>
7. Backlog.md agent guidelines: <https://github.com/MrLesk/Backlog.md/blob/main/src/guidelines/agent-guidelines.md>
8. Backlog.md CLI instructions: <https://github.com/MrLesk/Backlog.md/blob/main/CLI-INSTRUCTIONS.md>
9. Linear, agents: <https://linear.app/docs/agents-in-linear>
10. Linear, assigning issues: <https://linear.app/docs/assigning-issues.md>
11. Claude Code agent teams: <https://code.claude.com/docs/en/agent-teams>
12. Claude Code worktrees: <https://code.claude.com/docs/en/worktrees>
13. Claude Code agent view: <https://code.claude.com/docs/en/agent-view>
14. Codex cloud: <https://learn.chatgpt.com/docs/cloud>
15. Codex worktrees: <https://learn.chatgpt.com/docs/environments/git-worktrees>
16. GitHub Copilot cloud agent: <https://docs.github.com/en/copilot/concepts/agents/cloud-agent/about-cloud-agent>
17. Conductor workspaces: <https://www.conductor.build/docs/concepts/workspaces-and-branches>
18. Claude Squad: <https://github.com/smtg-ai/claude-squad>
19. Devin backlogs: <https://docs.devin.ai/use-cases/examples/clear-engineering-backlogs.md>

## Findings

### How tasks reach an agent

- **The agent pulls from a computed ready set.**
  - beads: `bd ready` lists unblocked issues, and `bd ready --claim` claims the first match [1].
  - Task Master: `task-master next` [4].
  - Claude Code agent teams: teammates claim the next "unassigned, unblocked" task, or a lead assigns one [11].
- **A human dispatches each task.**
  - Codex starts each task from a human [14].
  - Conductor and Claude Squad start a session per workspace with a human's prompt [17][18].
  - Linear takes an assigned issue or an @mention [9]. Copilot starts from the agents panel, a conversation, or an issue or pull request [16].
  - Devin starts from a label, a mention or an assigned ticket [19].
  - Backlog.md has the human name the task ("Work on BACK-10 only") [6].

### Keeping two agents off one task

- beads: `bd update <id> --claim` sets the assignee and `in_progress` in one atomic step; the first claim wins [1][2].
- Claude Code agent teams: claiming uses file locking [11].
- Task Master, Backlog.md: nothing locks a task. Backlog.md agents set themselves as assignee by convention [7].
- Linear: the human stays the assignee and the agent is tracked separately [10].
- No surveyed tool lets a claim expire.

### Isolating parallel agents

- Every surveyed session runner gives each session its own git worktree or environment:
  - Claude Code: `--worktree` under `.claude/worktrees/<name>`, and `isolation: worktree` for subagents [12];
  - Codex: a detached HEAD under `$CODEX_HOME/worktrees`, or a cloud workspace per task [14][15];
  - Conductor: one workspace per branch [17];
  - Claude Squad: tmux plus a worktree [18];
  - Copilot: an ephemeral cloud environment, one branch and one PR per task [16].
- The runner owns the worktree, not the tracker. beads shares one database across worktrees [3].
- Claude Code agent teams are the exception: they warn that two teammates editing one file overwrite each other, and advise that "each teammate owns a different set of files" [11].
- For merges:
  - beads has `bd merge-slot`, a lock only one agent holds while resolving conflicts [1];
  - Backlog.md records which files a task modified and can search tasks by file [8].

### Telling sessions apart

- Session identity is weak almost everywhere.
- Claude Code gives a background session a short ID, a name and a full `sessionId` [13].
- beads assignees are plain strings, and closing an issue can record `--session` or `CLAUDE_SESSION_ID` [1].
- Backlog.md uses a free-text `@name` [7].
- Linear tracks the agent apart from the human assignee [10]; whether each assignment opens its own agent session is **unverified** from its user docs.

### Ordering

- Backlog.md alone keeps a manual order ahead of priority: lists sort by ordinal, then priority, then ID [8].
- beads sorts by priority, hybrid or oldest [1].
- Task Master prefers subtasks of a task already in progress, then priority, then fewest dependencies, then ID [5].

## Conclusions

- **No tool binds an agent to a named, ordered list.** The closest are beads filtering `bd ready` by assignee and Linear's per-agent views. Short-lists can combine the two patterns: a list is an agent's queue, and taking from it pulls the first Issue on it neither blocked nor claimed.
- **Claiming in one step is not universal.** beads and Claude Code agent teams claim atomically; Task Master and Backlog.md lock nothing. tm's Claims already work this way (ADR 0009). The gaps are telling sessions apart and expiry, which nobody solves well.
- **Worktrees belong to the runner.** tm should share one store across them through its repo mapping and create none, which fits "tm writes only what it owns".

## Implications / actions

- **TM-59, Short-lists:**
  - `tm next take <list>` pulls the first Issue on the list neither blocked nor claimed, in one write;
  - a terminal binds to a list by environment variable.
- **TM-60:** Claims record the harness's session id and a readable name, and a session sees only its own.
- **TM-61:** enforce Claim expiry, so a dead session cannot hold an Issue on a shared list for good.
- **Later:** record the files each Issue's work changed, to warn about overlap (Backlog.md [8]), and a merge lock for parallel agents (beads [1]).

## Open questions

- Whether a sandboxed agent reading the Snapshot can take from a Short-list, since a take is a write.
- Sculptor's isolation: its launch post describes a container per agent and its product page describes worktrees (**unverified** which is current). Copilot's `copilot/` branch prefix and Codex's best-of-N attempts are also **unverified**.
