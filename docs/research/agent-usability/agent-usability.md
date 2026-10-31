# Agent usability of the tm CLI

- Date: 2026-10-24
- Author: claude, for TM-94
- Scope: whether Claude Code and Codex, told only that tm exists and to run `tm --help`, can do an Agent's everyday tm work, and where they stumble.

## Question

Phase 2 is done when Claude and Codex, given only `tm --help`, complete a scripted set of tasks and every stumble is fixed or filed (TM-85). This document is that script and its scoring. TM-94 ran it as a baseline on the commands that existed then, without `handoff`, which TM-81 made runnable; TM-85 runs it again once `tm setup` (TM-77) and a release (TM-93) land.

## Sources

- [`scripts/run.sh`](scripts/run.sh), which seeds, runs, checks and scores each task.
- `claude --help` (Claude Code 2.1.310) and `codex exec --help` (codex-cli 0.164.0), read 2026-10-24, for the flags below.
- The isolation probes under [Isolation](#isolation), run 2026-10-24.
- The baseline's results: a comment on TM-94.

## Findings

### Tasks

Each task runs in a fresh headless session over a freshly seeded scratch Workspace (Project `EV`), so a stumble in one cannot spill into the next. Every prompt starts: "You are working in the git repository in your current directory. Its work is tracked with the tm command-line tool; run tm --help to learn it. Do not change any file other than through tm unless the task says so."

| Task | Seed | Asks | Done when |
| --- | --- | --- | --- |
| `take-next` | three todo Issues, EV-2 the most urgent | take the next piece of work and start it | EV-2 is in progress and holds the Agent's Claim |
| `comment` | EV-1 | record a finding and the next step on EV-1 | EV-1 has a comment naming the empty input |
| `describe` | EV-1, and `plan.md` holding backticks, `$(…)` and quotes | replace EV-1's description with `plan.md` | the description equals the file |
| `blocked` | EV-1 blocks EV-3, EV-2 blocks EV-4, EV-5 free | name each blocked Issue and its blocker, one `BLOCKED <key> by <key>` line each | the answer has exactly those two lines |
| `finish` | EV-1 in progress, assigned to the human, no Claim | the work has landed; finish EV-1 | EV-1 is done |
| `undo` | EV-1 at high priority | set EV-1 to low, then undo that mistake | the Agent set EV-1 to low, and it is high again |
| `handoff` | EV-1 | start it, then hand it to the other Agent with what is left | EV-1's Claim is handed to the other Agent, with a note naming the error message |
| `ask` | EV-1 at high priority, EV-2 at low | start EV-1, leave the absent human a decision it needs with its two choices, then take the next work | EV-1 waits on someone with a question naming the choice and offering both options, and EV-2 is in progress under the Agent's Claim |
| `remember` | EV-1 | record, for future sessions, that the test suite must run with `-race` | a live memory mentions race |

### Scoring

Each task scores one of:

- **done unaided**: the check passes and no tm call failed;
- **done after N tm errors**: the check passes after tm calls that failed, each marked `tm-error` or `fail` in the task's `commands.txt`;
- **not done**: the check fails, with the condition that failed;
- **agent failed**: the Agent's CLI exited non-zero or gave no successful final answer, with the first line it printed to stderr, else its answer; the check is not run, since a Workspace no one touched passes some checks;
- **unscored**: the harness could not read the Workspace or the transcript, or the check passes but the transcript holds no tm command.

A tm misstep is a command whose output carries tm's or clap's error, wherever it falls in a chained call: an Agent often chains commands with `;`, and the last one's success hides the first one's error. tm's JSON error counts too, compact or pretty-printed, since its `"error"` key need not come first. A call that names tm and exited non-zero is a misstep even when it shows no error, since the Agent may have discarded tm's stderr; it counts even when another command in the call was the one that failed, or tm is only a word in it, as in `grep -r "tm " .`, so a person confirms it in `commands.txt`. A call that exited non-zero without naming tm, such as a search that finds nothing, is marked `fail` and is not a misstep. A pipe can hide a tm error altogether, as in `tm --json show EV-9 | jq .data`, which is left to the person reading the transcript. The script scores what it can check; a person then reads each transcript for what it cannot: a command the Agent guessed wrong that happened to succeed, and every tm message that misled it. Each stumble becomes an Issue related to TM-85.

A seed that fails stops the run, naming the task, rather than scoring an Agent against a Workspace seeded wrong.

### Isolation

A session sees tm only through `tm --help`, so neither Agent may load this machine's instructions, hooks, plugins or MCP servers, and neither may write to the real tm home. The script:

- removes every variable a harness, tm or git reads for identity (`CLAUDE*`, `CODEX*`, `TM_*`, `GIT_*`, `GEMINI_CLI`, `OPENCODE`, `CURSOR_AGENT`) before seeding or starting an Agent, so a session the script runs from cannot lend its identity: tm would otherwise read the parent's `CLAUDE_CODE_SESSION_ID` and write a Codex run as claude;
- gives each task its own `TM_HOME` and a git config holding only a user name and email, through `GIT_CONFIG_GLOBAL`;
- runs Claude as `claude -p --setting-sources "" --strict-mcp-config --no-session-persistence --allowedTools Bash Read Write Edit Glob Grep`. `--bare` would isolate further, but it reads only `ANTHROPIC_API_KEY`, which bills API credits rather than the plan;
- runs Codex as `codex exec --ephemeral --skip-git-repo-check --sandbox workspace-write --add-dir <TM_HOME>` with `CODEX_HOME` set to an empty directory that holds only a link to `~/.codex/auth.json`, removed when the run ends, even when the script is interrupted. The script never writes `~/.codex`: if Codex replaced the link, perhaps refreshing its login, the file it left is moved to `codex-auth.json.left-by-codex.<task>.<suffix>` in `$OUT`, readable only by its owner, and noted in `codex-scores.txt` for the person to check and copy back. The run then stops after that task, since the next session would link a login the refresh may have retired.

Measured on 2026-10-24, each probe asking the session what instructions, hooks and skills it could see:

- `claude -p` with the flags above reported no CLAUDE.md or AGENTS.md, no hook output, and only Claude Code's built-in skills.
- `codex exec --ignore-user-config --ignore-rules --ephemeral` still quoted `# AGENTS.md instructions`, the user's global `~/.codex/AGENTS.md`. With the empty `CODEX_HOME` it reported none. It still lists the user-level skills under `~/.agents/skills` (react-aria, shadcn, rust-best-practices and others), which say nothing of tm.
- A run's writes show in `tm history` as `claude for jace@example.com` or `codex for jace@example.com`, so tm saw each Agent by its own harness.

### Running it

```sh
docs/research/agent-usability/scripts/run.sh claude
docs/research/agent-usability/scripts/run.sh codex take-next undo
```

Each run writes to `$OUT`, a new temporary directory unless set: `<agent>-scores.txt`, which each run appends to under a dated heading, the tool versions with the tm binary's path and build time, and for each task its Workspace, `transcript.jsonl`, `commands.txt` and the final answer. The results hold transcripts and machine paths, so they stay out of the repository. Eighteen sessions, nine tasks for each Agent, count against the Claude and Codex plans.

## Conclusions

The baseline's scores and the Issues filed for its stumbles are in TM-94's comments; TM-85 compares its run against them.

## Open questions

- Model choice: each Agent runs its CLI's default model, recorded only through the CLI version. A run that should compare models passes `--model` (Claude) or `-m` (Codex).
- One run per task measures one sample. Rerunning a task that scored a misstep shows whether it repeats.
