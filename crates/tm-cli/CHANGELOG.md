# Changelog

## 0.4.1 (2026-11-05)

### Fixed

Publishes the 0.4.0 changes: 0.4.0's release built nothing for Apple silicon Macs, as its runner had been retired, so no binaries or Homebrew formula went out. 0.4.1 is the same tm, released.

## 0.4.0 (2026-11-05)

### Added

An Agent that reaches a decision only its human can make runs `tm ask KEY "question"` and takes other work: the question joins the Issue's comments, `waiting_on` names its human, the Issue leaves the ready queue, and the Agent's Claim is kept for it. `tm prime` lists what waits on the session's human, telling an Agent's session to bring the questions to them, and `tm answer KEY "reply"` records the reply; the asking Agent then resumes with `tm start`. A person asks someone with `--of`. Finishing an Issue clears its wait. Issue records in an export carry no wait, and a record import refuses `waiting_on`. The store rebuilds its projection once on first open.

`tm ask KEY "question" --option A --option B` offers two or more likely replies, worded rather than numbered, shown numbered in `tm show` and `tm prime` and listed in their JSON. `tm answer KEY 2` or `tm answer KEY b` picks one, reported as `choice` in `tm answer`, `tm show` and the asking Agent's `tm prime`; a number that picks no option is refused, and any other reply is taken as written. At a person's terminal, `tm answer KEY` asks for the reply and a bare `tm answer` walks each question waiting on you, a blank line passing one over; an Agent, anyone not at a terminal, and `--json` are refused rather than left waiting. `tm withdraw KEY "why"` lets the asker settle its own question: the Issue waits on no one, the Claim stays kept for the asking Agent, and its next `tm prime` says the question was withdrawn.

`tm comment KEY --edit ID "<text>"` corrects a comment you wrote, or the one an Agent wrote for you; `--edit last` corrects your own latest on the Issue. `tm show` prints each comment's id and marks an edited one, JSON gives `edited_utc_ms`, search finds the new words, `tm history` keeps the earlier text, and `tm undo` restores it. Questions, answers and withdrawals are not edited, since their text decides what was chosen. The store rebuilds once on first open to index edits.

`tm edit KEY --append description=-` adds a paragraph to the end of a long text field after one blank line, so growing an Issue's plan no longer means rewriting it whole. It takes stdin like `--set`, refuses a field that is not long text, and is checked against the version it read like any other edit.

`title:word`, `description:word` and a Scheme text field (`notes:word`) search that field alone, matched as `text:` matches: by English stem, ignoring case and accents, with quoted phrases, comma lists for any of and `-` for none. Opening a store rebuilds its projection once to index Scheme text fields.

Ids from an import's source stay lookup aliases, so old commits and notes keep resolving: `tm show TREK-114` (or a beads id) finds the Issue imported from it, wherever a command takes an Issue, and `tm show CMT-243` shows the Issue holding that comment with the comment in view. `tm comment KEY --edit CMT-243` edits it, and `tm show` prints an imported comment's source id, such as `[CMT-243]`, with `source` in JSON.

`tm list` keeps to the open work when given a filter: `tm list priority:high` no longer lists done and cancelled Issues. A filter naming `status`, `category` or `archived` reaches every Status, and `--all` still lists everything.

`tm show` names the Statuses an Issue can move to, in a `moves to:` line after its Status and a `moves_to` list in JSON, each with its category, so finishing work needs no trip through `tm scheme show`. `tm move --help` gives `done` as an example Status and points at `tm show`.

Path Claims: `tm claim`, `tm start` and `tm next` take `--paths GLOB`, the files a Claim covers, from the repository root and repeatable, and any other session or sub-agent is warned before editing one. In Claude Code, `tm setup claude` adds a hook before Edit, MultiEdit, Write and NotebookEdit that tells the Agent, once per Claim and file, which Issue and session hold the file, by a path the Claim names or as already committed on its branch. It never blocks the edit, and when git cannot say, it tells that once, and again if it comes back. Run `tm setup claude` again to add the hook to an existing setup. `tm prime` lists other sessions' Path Claims on the files the checkout changes, as `path_claims` in JSON.

`tm show`, `tm claims`, `tm history` and JSON show a Claim's paths, with its branch in JSON. A handoff keeps them, and the Agent it goes to keeps them when it takes the Claim up; the session holding a Claim changes them by claiming it again with other paths.

`tm remember "fact"` keeps a durable fact about the Project, such as how the tests must run, and every session's `tm prime` shows the 10 most recently written with who wrote each and how long ago. Its key comes from its first words unless `--key` names one; an unnamed memory never replaces another, while `--key` replaces that memory's text and says whose it was. A memory that reads much like another names it, and one over 500 characters is refused. `tm memories` lists them, searches them by words or shows one by its key, and `tm forget KEY` drops one, which `tm undo` brings back. Memories go out with `tm export` and come in with a record import. The store rebuilds its projection once on first open.

`tm list` searches Issue text: a bare word or a quoted phrase matches Issues whose title, description or comments not retracted hold it, matching English word stems and ignoring case and accents (`tm list login timeouts`), and `text:` writes the same term explicitly (`-text:word` excludes it). The `bare_word` filter error is gone, since a bare word is now a search. Opening a store rebuilds its projection once to make the full-text index.

Every operation an Agent writes now records its session, and a Claim the git branch it was taken on. `tm show KEY` lists the sessions that worked the Issue, most recently active first, each with its branch, what it did and the command that resumes it (`cd DIR && claude -r ID` or `cd DIR && codex resume ID`), or that it ran on another machine; the directory comes from this machine only and is never exported. `tm list session:ID` finds the Issues a session wrote, taking a full id or its end as `tm history` shows it. `tm history` names each write's session; `tm prime` names the session's own id and gives the resume command under a question an Agent asked. A store from before gains the session column and rebuilds its projection once, when first opened.

`tm show` prints an Issue's latest 5 comments, from the question it waits on if that is older, and counts the earlier ones, so checking an Issue no longer reads its whole discussion; `--comments N` or `--comments all` shows more. JSON gives the count as `earlier_comments`, and commands that answer with the Issue, such as `tm start` and `tm move`, print it the same way.

`tm sessions` lists the most recently active Agent sessions, each with when it last wrote, the Issues it wrote and the command that resumes it, so a session you lost track of can be found without knowing an Issue it touched. `--limit` shows more than 10; `--json` gives each session's Issues as `tm list` does.

`tm import --from trekker .trekker/trekker.db` moves a Trekker project into tm, reading its database read-only since Trekker has no export. Epics, tasks and sub-tasks keep their parents, Statuses, priorities, tags as labels (a `bug` or `spike` tag also sets the type), comments and dependencies as `blocks` Links; `rank` and the audit log are counted as not kept.

### Fixed

A bare `tm scheme` prints the Scheme as `tm scheme show` does, `--json` included, rather than failing with its help.

`tm start`'s branch suggestion keeps a word with an apostrophe whole (`issues`, not `issue-s`), and where most of the repository's branches are named `word/…` it adds `fix/` for a bug and `feat/` for any other type.

`tm guide` says that where tm is a mise shim, a new worktree whose mise config needs trust stops tm's hooks from starting tm until a person runs `mise trust` there.

`tm import --from trekker` decodes the HTML entities an Agent wrote into a task's title or description (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&#39;`, `&#x27;`), so `Build &amp; Run` arrives as `Build & Run`; any other `&` is kept. `tm create` and `tm edit` warn when a title or description they are given holds one of those entities, and store it as written.

`tm list --help` and `tm guide` name `no:parent`, which lists the Issues under no parent, and the `has:FIELD` and `no:FIELD` filters it belongs to.

When a question waiting on you was asked from a Claude session, tm prime ends its waiting section with the note that Claude deletes sessions unused for 30 days, as tm show does.

A value written as one word in angle brackets, such as `<value>`, is refused as a placeholder to replace, in `--set`, `--append`, `--title`, `--label`, `TM_AGENT`, `TM_ACTOR` and `TM_SESSION`. tm's fixes now quote their placeholders, so a fix pasted unchanged fails with that error instead of the shell reading `<value>` as a redirect that could read or overwrite a file.

`tm --help` lists commands under headings by use (Everyday, History and undo, Agent work, Memories, Setup and data), each in one short line, so a person can scan it; the detail moved into each command's own `--help`, which says no less than before. `tm hook`, run only by the hooks `tm setup` writes, is left out of the list, and every argument now has a description.

`tm search WORDS` runs `tm list WORDS`, where it used to fail and suggest `tm start`, which would claim work.

`tm setup claude` names the warning before an edit to a file another session's Claim covers when it lists the hooks it added.

Every write publishes the Snapshot with SQLite's backup API instead of `VACUUM INTO`, about 30% faster (111 to 77 ms at 10,000 Issues on macOS). The Snapshot keeps its rollback-journal format, so a sandboxed reader can still open it read-only.

Editing a title or description updates its search index entry by lookup instead of scanning the index: a title edit at 100,000 Issues went from 73 to 45 ms, and writing 100,000 Issues in one batch from 1,408 to 79 s. Opening a store rebuilds its projection once.

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

Generated by oakum 0.8.3.
