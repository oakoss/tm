# tm adds its instruction lines only on a human's yes

- Status: accepted
- Date: 2026-10-20
- Deciders: Jace Babin

## Context and Problem Statement

An Agent works well with tm only once it knows the repo tracks work there and how: move an Issue to `in_progress` when work starts, record progress with `tm comment`, change fields with `tm edit`. `tm prime` tells it at session start, but only where a SessionStart hook runs it, and not everyone uses one. Beads writes a short blurb into `AGENTS.md` and `CLAUDE.md` on init, without asking. This repository's invariants say tm "never edits `AGENTS.md` or `CLAUDE.md`". How does a repo set up for tm get the lines that point Agents at it, and who writes them?

## Decision Drivers

- Instruction files are the owner's: a tool that adds to them unasked is the behavior this project chose not to copy from beads.
- Agents must never stop at a prompt; anything that asks needs a human to answer.
- Setting a repo up should take one step for a human and one for an Agent.
- The guidance must match the installed tm, so it should live in the binary, not in text copied once.

## Considered Options

- **Never write them**: `tm init` prints the lines, and whoever runs it adds them.
- **Write them unasked**, as beads does.
- **Write them on a human's yes; print them for an Agent.**

## Decision Outcome

Chosen option: **write them on a human's yes; print them for an Agent**, because it makes setup one step for each without tm adding to an instruction file unasked.

- **Who is a human.** tm asks only when stdin and stdout are both terminals, JSON output is off (neither `--json` nor `TM_JSON`), and no Agent is detected by the rules of [ADR 0010](0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md): `TM_AGENT`, or a harness variable in its table, such as `CLAUDE_CODE_SESSION_ID`. Anything else is an Agent or a script, which is never asked.
- **For a human**, `tm init` shows the exact lines and asks, defaulting to no. The file is `AGENTS.md` when it exists, including when `CLAUDE.md` only imports it; `CLAUDE.md` when it is the only one; with neither, `tm init` offers to create `AGENTS.md`. The lines sit between marker comments, so running `tm init` again updates them instead of adding a second copy.
- **For an Agent**, `tm init` writes no instruction file. Its output, text and `--json`, carries the same lines and names the file, for the Agent to apply with its own edits, where they show in its diff for review.
- **What the lines say** is short and points at the guide built into the binary: that the repo tracks work in tm and that `tm guide` says how, so they do not go stale as tm changes.
- **Only these two files.** `tm init` writes no other tool's settings. Writing an Agent's hooks stays with the separate `tm setup <agent>`, behind its own confirmed diff and `--undo`; `tm init` points at `tm setup claude` while any of tm's hooks are missing from `.claude/settings.json` (amended 2026-10-26).

The invariant "tm writes only what it owns" changes from "never edits `AGENTS.md` or `CLAUDE.md`" to "edits `AGENTS.md` or `CLAUDE.md` only to add its own lines, shown first and confirmed by a human at a terminal".

### Consequences

- Good, because a human sets a repo up in one step and sees every line before it lands.
- Good, because an Agent sets a repo up without a prompt, and its edit is reviewed like any other.
- Good, because the lines point at `tm guide`, so the guidance follows the installed tm.
- Bad, because tm now writes to a file it does not own, if only on a yes; finding the right file and keeping one copy between markers is code that must be right.
- Neutral, because removing the lines stays a manual edit; the markers make it easy.

### Confirmation

TM-46 implements the wizard and the Agent output, and TM-45 the guide the lines point at. Revisit if humans decline the lines often enough that the question is noise, or if an Agent harness reaches tm through a terminal that hides it.

## More Information

- The output contract this extends: [ADR 0011](0011-the-cli-speaks-one-json-envelope-and-flat-coded-errors.md), including that a prompt appears only when stdin and stdout are terminals.
- Identity and Agent detection: [ADR 0010](0010-operations-record-who-an-agent-acts-for-and-undo-names-a-batch.md).
