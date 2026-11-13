# Changelog

## 0.5.0 (2026-11-13)

### Added

`@mentions` resolve when text is saved: comments and their edits, descriptions, questions, answers, withdrawals and handoff notes store each `@word` that names a known Actor (a person's email, its local part or a word of their name, or an Agent's name) as a token, and `tm show`, `tm history`, `tm prime` and `tm tui` print it as `@` and the Actor's name. When a word names several, a person at a terminal picks one; an Agent, a pipe and `--json` are refused with each candidate as it can be written. `tm show --json` keeps the stored text and lists the mentions apart.

A mention now notifies whom it names: `tm notifications` lists your unread Notifications, newest first, with the Issue, who wrote it, where and the line that mentions you (`--all` adds the read ones), and `tm notifications read ID`, or `all`, marks them read, which `tm undo` reverts; until then `tm prime` counts the session's unread ones and shows the newest. An edit notifies only for the mentions it adds, an import notifies no one, and an Agent's Notifications reach every session of it writing for the same human. Reading one never changes its Issue. A `tm` older than this release cannot read a Workspace once it holds a person's name, a read mark or a record of a write past a Claim, so upgrade every `tm` that shares one.

A write past another session's Claim, a person's or a forced one to its Status or description, is now recorded with the write and notifies the Agent holding it: `tm notifications` and `tm tui` show `jace in your Claim, 2m ago: wrote status while you held it`, `tm prime` shows `jace on TM-1: wrote status while you held it`, and `tm history` shows `wrote status past claude's Claim`. A finishing move or `tm git scan` that releases the Claim notifies no one.

`tm ready`, `tm prime`'s ready queue and `tm tui`'s Ready screen show the Project the directory maps to, as `tm next` already took from; what an Issue unblocks may still be in any Project. Where the directory maps to no Project in the Workspace, `tm ready` and `tm prime` show every Project's queue and warn why, and that a `TM_SHORTLIST` binding goes unused.

A person's write now records their name from git's `user.name` beside their email, once and again only when it changes in that Replica, as a record of its own (ADR 0019); `tm undo` still undoes their write, not the name. Mentions show it. Under `TM_ACTOR`, or as an Agent, nothing is recorded, since git's name is then someone else's.

`tm setup claude` writes tm's hooks to `.claude/settings.local.json` by default, your own settings kept out of commits, so contributors without tm are not given hooks that fail. At a terminal it asks which file, local first; `--shared` writes `.claude/settings.json` for everyone who clones the repo. Hooks already in a file stay there: a rerun updates them in place and `--undo` removes them from each file that holds them, so nothing moves for an existing install. Each Claude Code hook now starts with `command -v tm >/dev/null || exit 0`, so a clone without tm runs it silently, and the Bash hook gets a 10-second timeout. Where git does not ignore the local file, setup says so.

In `tm tui`, `b` opens the Board: a column per Workflow Status with its count, done and cancelled shown only after `a`. `h` and `l` change column, `j` and `k` move within one, and columns that do not fit scroll into view, the edge columns saying how many are hidden. `H` and `L` move the Issue to the Status beside its own, through `tm move` as you, and the cursor follows it; `s` picks any Status its Workflow has. A write is refused like the CLI's when someone changed the Issue since it was read, and `u` undoes this session's writes. Enter opens the Issue.

`tm tui` opens the terminal screens over this directory's Project, showing its name, the keys (`q` or Ctrl-C to quit, `?` to show them, Esc to hide them) and when another process last wrote. The terminal is restored when it quits, fails, panics, or is sent SIGTERM or SIGINT, which end it with 128 plus the signal's number; a second such signal ends it at once. Without a terminal it refuses with `not_a_terminal`.

In `tm tui`, an opened Issue takes the keys Triage does: `s` moves it to one of the Statuses `tm show` lists under "moves to", `p` sets its priority, `A` assigns it (`@me` first, or a name typed), and `u` undoes this session's last write. Each write is the one `tm move`, `tm edit` or `tm undo` makes, refused if the Issue changed since it was read. `c` opens `$VISUAL`, else `$EDITOR`, else `vi`, on an empty file and adds what you save as a comment, as `tm comment` does; saving nothing writes nothing. Ctrl-Z, or a SIGTSTP from elsewhere, hands the terminal back and stops `tm tui` until `fg`, which redraws it; after a SIGSTOP it takes the terminal back on resume. Where no shell could bring it back, as in `exec tm tui` or `tmux new-window 'tm tui'`, Ctrl-Z does nothing and says so. Its Claim reads as `tm show` words it: one with no expiry, one handed on and not yet taken, and one that lapsed, which any Agent may take unless the Issue is archived.

In `tm tui`, Enter opens the selected Issue, read again whenever another process writes. `j`/`k`, the arrows, `g`/`G`, Home/End and the page keys scroll it, and `q`, Esc or Backspace go back to the list, which keeps its place. An Issue that can no longer be shown says why in place.

`tm tui` lists the Project's open Issues, most recently updated first, as `tm list` does: Key, status, priority and title, with the count in the header, read again whenever another process writes, a sandboxed Snapshot reopened when a publish replaces it. `/` types a filter in `tm list`'s language (Enter applies it, Esc drops it, a bad one says why, `@me` as `tm list` reads it), `a` switches between open work and every status, and `j`/`k`, the arrows, `g`/`G`, Home/End and the page keys move the selection, which stays on its Issue across a read. Stored text is shown escaped, as the CLI prints it.

In `tm tui`, an Issue opens laid out rather than as `tm show`'s text: its title, then Status, type, priority, the people and its Claim beside their labels, its description and every comment rendered as Markdown (a `tm ask` question, its answer and an edit marked as `tm show` marks them), and its five latest writes with how many more `tm history` holds. Its parent, children (archived ones marked) and Links sit in a Related pane, beside the text on a wide terminal and below it on a narrow one; Tab and Shift-Tab (or `n` and `N`) move between them and Enter opens one, so you can walk from a story to its epic and back with `q`. Triage's preview beside the queue shows the selected Issue laid out the same way. The screen reads what `tm show --comments all --json` and `tm history --json` report, so it shows what the CLI does.

The filter language takes `project:`, a Project's key (current or retired, without case) or its id; a key or id no Project has fails with `not_found`. `tm tui`'s list, Triage and Board read only the Project the directory maps to, and a list filter naming `project:` reads that instead.

`tm tui` counts your unread Notifications on every screen, and `m` lists them, newest first: Enter opens the Issue, `r` marks one read and `R` every one, which `u` reverts. A comment written there that mentions several Actors with one `@word`, such as `@kim`, now asks which is meant in a picker, or leaves it as text, rather than being refused; Esc keeps the comment unwritten.

In `tm tui`, `r` opens the ready queue as `tm ready` prints it: the session's Short-lists, then the Issues that can start, each with what it unblocks and the Short-list it is on, read again whenever another process writes. A warning `tm ready` would print, such as no identity to find the Short-lists by, leads the screen. `j`/`k` and the page keys scroll it, and `q` or Esc go back to the list.

In `tm tui`, `t` opens Triage: the Issues in a triage Status, the selected one shown beside the queue on a wide terminal and below it on a narrow one, each moved, prioritized or assigned with one key. `s`, `p` and `A` open a picker you can type into to narrow, with you first when assigning and a name no one holds yet taken as typed. Space marks Issues so the next action takes them all. Each write goes through `tm move` or `tm edit` as you, refused like the CLI's when someone changed the Issue since it was read. A line at the bottom says what happened, with the CLI's warnings, until the next key; the cursor goes on to the next Issue, and an Issue whose write was refused stays marked. `u` undoes this session's writes, newest first, as `tm undo` does, and keeps a refused undo for another try. The top line names the screens open, and the `?` keys list the open screen's own keys.

Views: saved filters and layouts kept in the repo file as `[[views]]` tables, each with a `name`, a `filter`, a `layout` of `list` or `board`, an optional `project`, and for a Board optional `[[views.columns]]` (a `status` and its own `filter`) and `[views.swimlanes]` (a `field` and its own `filter`). In `tm tui`, `v` lists the Project's Views and Enter opens one; a board View shows its own columns and bands its cards into swimlanes by the field's value. `e` edits a View in `$EDITOR`, `n` adds one and `d` removes one; each is shown as a diff of the repo file and written only on `y`, with the rest of the file kept as written. An Agent or a read-only Snapshot sees the Views but cannot change them.

`tm list`, `tm ready` and `tm claims` take `--watch`: each prints its result, then again whenever another process writes or a held Claim lapses, until interrupted; `tm tui`'s screens read again the same way. Text marks each result after the first with the time it was read; `--json` prints one document per line, and nothing on stderr. A busy store, at the start or later, is tried again at the next poll; any other error ends the watch, and so does the first print after the reader closes the pipe.

### Fixed

The edit hook and `tm ready` no longer run git to read `user.email` when an Agent runs them, since they tell Agents apart by name and session; each was measured about twice as fast.

A Claim handed to an Agent that runs in no session now warns it before an edit and in `tm prime` until it takes the Claim up, as a handed Claim warns every other session.

`--json` is offered only where a command honors it. `tm tui` and `tm hook` do not list it in their help; `tm tui --json` is a usage error (exit 2) with fix `tm list --json`, and a `tm hook` whose arguments fail to parse prints clap's error as text on stderr, where the harness looks, even under `--json` or `TM_JSON=1`. A JSON usage error names what clap lists, such as the missing arguments, and carries clap's hint under `tip`. `tm next --peek` refuses `--paths`, as it refuses `--for` and `--no-expiry`, and `tm --version --json` answers under `data.version`, not `data.help`. The edit hook says when its input has no `tool_input`, rather than listing nothing.

Errors whose `fix` was advice now give a command to run, or no fix. A shared Short-list name gives `tm shortlist --all --json` (or, on import, `tm shortlist edit '<id>' --name '<name>'`), a full Short-list `tm shortlist edit <name> --size '<size>'`, a `TM_SHORTLIST` naming no list the command that ran with `TM_SHORTLIST='<name>'` before it, such as `TM_SHORTLIST='<name>' tm next --peek`, and `tm tui` without a terminal `tm list`. An Issue not on a Short-list gives `tm shortlist add '<list>' '<key>'`. A store this process may not write, a missing Snapshot, a Snapshot from an older tm, and a line tm cannot rebuild with `--set` carry no fix; their message says what to do. The `retry` and `upgrade tm` fixes are unchanged.

`tm git scan` no longer closes a new Issue because an old commit names an imported tracker's id that spells its Key, such as beads' `cpl-482` for `CPL-482`. A word that spells a Key but is exactly the id one Issue was imported from names that Issue, and a Closes line in a commit written before the Project or the Issue it would name is skipped without a warning.

Opening a Workspace's store no longer takes the write lock to check the Workspace id it already holds, so a read such as `tm list` no longer waits on, or fails behind, another process's write.

`tm tui` shows the warnings every command prints on stderr, such as logged operations that did not apply in full or the Snapshot it reads from and when that was published, in a row above the footer on every screen. The row is read again with the screen, so it clears once the cause is fixed.

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

Generated by oakum 0.10.1.
