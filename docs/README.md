# Documentation

## Layout

| Directory | Holds | Numbered |
|---|---|---|
| [`decisions/`](decisions/) | Architecture decision records: what was chosen, what was rejected, and why | yes |
| [`specs/`](specs/) | The contract for a feature area: interface, behavior, edge cases | no |
| [`research/`](research/) | What was verified about external systems, with sources and the scripts that measured it | no |
| [`contributing/`](contributing/) | Process for anyone changing this repository, human or agent | no |

`decisions/`, `specs/` and `research/` each have a `0000-template.md`.

## Which one am I writing?

**A decision** when something was chosen over a real alternative and the reasoning would otherwise be re-argued. Status stays `proposed` until it is actually agreed.

**A spec** when a feature area has a contract others must code against. Specs cite the decisions that drive them.

**Research** when an external system was examined and the findings would be expensive to re-derive. Every claim carries its source (a link, or a command and its output) and a date.

**Contributing** when the audience is someone changing this repository: process, build order, task tracking. Not product behavior.
