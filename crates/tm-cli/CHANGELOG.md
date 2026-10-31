# Changelog

## 0.4.0 (2026-10-31)

### Added

An Agent that reaches a decision only its human can make runs `tm ask KEY "question"` and takes other work: the question joins the Issue's comments, `waiting_on` names its human, the Issue leaves the ready queue, and the Agent's Claim is kept for it. `tm prime` lists what waits on the session's human, telling an Agent's session to bring the questions to them, and `tm answer KEY "reply"` records the reply; the asking Agent then resumes with `tm start`. A person asks someone with `--of`. Finishing an Issue clears its wait. Issue records in an export carry no wait, and a record import refuses `waiting_on`. The store rebuilds its projection once on first open.

`tm ask KEY "question" --option A --option B` offers two or more likely replies, worded rather than numbered, shown numbered in `tm show` and `tm prime` and listed in their JSON. `tm answer KEY 2` or `tm answer KEY b` picks one, reported as `choice` in `tm answer`, `tm show` and the asking Agent's `tm prime`; a number that picks no option is refused, and any other reply is taken as written. At a person's terminal, `tm answer KEY` asks for the reply and a bare `tm answer` walks each question waiting on you, a blank line passing one over; an Agent, anyone not at a terminal, and `--json` are refused rather than left waiting. `tm withdraw KEY "why"` lets the asker settle its own question: the Issue waits on no one, the Claim stays kept for the asking Agent, and its next `tm prime` says the question was withdrawn.

`tm show` names the Statuses an Issue can move to, in a `moves to:` line after its Status and a `moves_to` list in JSON, each with its category, so finishing work needs no trip through `tm scheme show`. `tm move --help` gives `done` as an example Status and points at `tm show`.

`tm remember "fact"` keeps a durable fact about the Project, such as how the tests must run, and every session's `tm prime` shows the 10 most recently written with who wrote each and how long ago. Its key comes from its first words unless `--key` names one; an unnamed memory never replaces another, while `--key` replaces that memory's text and says whose it was. A memory that reads much like another names it, and one over 500 characters is refused. `tm memories` lists them, searches them by words or shows one by its key, and `tm forget KEY` drops one, which `tm undo` brings back. Memories go out with `tm export` and come in with a record import. The store rebuilds its projection once on first open.

Every operation an Agent writes now records its session, and a Claim the git branch it was taken on. `tm show KEY` lists the sessions that worked the Issue, most recently active first, each with its branch, what it did and the command that resumes it (`cd DIR && claude -r ID` or `cd DIR && codex resume ID`), or that it ran on another machine; the directory comes from this machine only and is never exported. `tm list session:ID` finds the Issues a session wrote, taking a full id or its end as `tm history` shows it. `tm history` names each write's session; `tm prime` names the session's own id and gives the resume command under a question an Agent asked. A store from before gains the session column and rebuilds its projection once, when first opened.

### Fixed

A bare `tm scheme` prints the Scheme as `tm scheme show` does, `--json` included, rather than failing with its help.

When a question waiting on you was asked from a Claude session, tm prime ends its waiting section with the note that Claude deletes sessions unused for 30 days, as tm show does.

## 0.3.0 (2026-10-30)

### Added

The filter language finds blocked work, with GitHub's names: `is:blocked` matches an open Issue an open Issue blocks, `is:blocking` one that blocks an open Issue, and `blocked-by:KEY` and `blocking:KEY` follow an Issue's `blocks` Links whatever their Statuses. `tm list` names the open Issues blocking each Issue it lists, as `[blocked by TM-1]` and a `blocked_by` list in JSON.

Field names in a filter read `-` as `_`, so `blocked-by:` works. A Scheme field can no longer be named `is`, `blocked_by` or `blocking`.

### Fixed

A field given to `tm edit`, `tm create` or `tm move` as its own flag, such as `tm edit TM-1 --priority low`, is refused with the command to run instead: `tm edit TM-1 --set priority=low`, or for `--status` beside only flags `tm move` takes, the `tm move` that changes it. clap's tip there pointed at `-- --priority`, which does not work.

`tm edit` given a field more than once, the Status included, says so; for the Status it used to point at a `tm move` that kept only the first value.

## 0.2.0 (2026-10-29)

### Added

`tm setup claude` names each Claude Code sub-agent in its tm commands, as `<agent_type>-<first 8 of agent_id>`, so parallel sub-agents' Claims and history are told apart. A sub-agent's Claims are released when it finishes, and the session's end releases its sub-agents' Claims with its own. Run `tm setup claude` again to add the new hooks; it rewrites the old session-end hook in place.

`tm setup codex` writes Codex's hooks to `.codex/hooks.json`. They prime each session, renew Claims after every tool use, and release the session's Claims when it ends. Codex runs them once the project is trusted and each hook is approved in `/hooks`; `tm setup codex --undo` removes them.

### Fixed

`tm setup`'s refusal to an Agent, and `tm init`'s without `--yes`, tell a person to run it from their own terminal, since a command run in the Agent's session, Claude Code's `!` included, counts as the Agent.

A `shortlist_name_shared` error carries `kind`, `name` and `count` whichever command raised it, and `tm shortlist` commands give the same fix as a filter: name the list by its id from `tm shortlist --all --json`. `tm import` refused this way says to rename one of the lists with `tm shortlist edit`.

## 0.1.0 (2026-10-27)

### Added

First release of tm: a local-first issue tracker for one human and their AI agents, with a complete `--json` CLI. Install it with the shell installer attached to this release or `brew install oakoss/tap/tm`.

Until v1 the store's log format can change between releases without a version bump, so a tm of another version can misread a store rather than refuse it. Take a copy with `tm export` before upgrading.

Generated by oakum 0.8.0.
