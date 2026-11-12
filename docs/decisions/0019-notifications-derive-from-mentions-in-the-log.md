# Notifications derive from mentions in the log

- Status: accepted
- Date: 2026-11-04
- Deciders: Jace Babin

## Context and Problem Statement

design.md has `@mentions` resolve to Actors and create Notifications, shown in a personal list apart from Triage, with Agents only notified for now. tm records people only by email, so a mention has no name to show, and an Agent is a name that means nothing without the human it writes for. What does a mention store, where do Notifications and their read state live, and what shows a person's name?

## Decision Drivers

- Every change is an operation in the log ([ADR 0005](0005-operation-log-is-the-source-of-truth.md)), and everything the projection holds is rebuilt from it.
- Reading a Notification must not change the Issue it is about: a read would otherwise move the Issue's version and refuse another session's `--if-version` write, and fill its history.
- A name can change; a mention written before must show the new one.
- Mentions resolve when text is saved, Jira-style: a person picks between candidates, an Agent is refused with them, and an unknown `@word` stays text (TM-82).

## Considered Options

- **What a mention stores**: the name as typed; the person's email or Agent name, shown as typed; a token naming the Actor, shown by name.
- **Where a Notification lives**: written to the log as its own operation when the mention is saved; derived from the operation carrying the mention, with only its read state written.
- **Where read state lives**: a set field on the Issue; a record of its own.
- **Where a name lives**: on every operation; a record per person, written when it changes.

## Decision Outcome

Chosen options: **a token naming the Actor, Notifications derived from the operations carrying them, read state in a record of its own, and a name record per person**, because a token survives a rename and reads in raw text, a Notification derived from its mention cannot disagree with it, and neither a read nor a name touches an Issue's version or history.

### Mentions

- A mention is stored in the text as a token: `<@jace@example.com>` for a human, by the email tm records, and `<@claude for jace@example.com>` for an Agent, by its name and the human it writes for. `tm show` prints it as `@Jace Babin` or `@claude`; JSON keeps the text as stored and lists the mentions apart.
- tm resolves `@word` when a comment, a description or a new Issue is saved, against the Actors the Workspace knows: who has written to it, Assignees and Short-list Actors, by name or email, ignoring case. Text in backticks is never a mention, and a name holding `<`, `>` or a line break is never offered.

### Notifications

- A Notification is derived from the operation that carries a mention, for each Actor it mentions apart from the one writing: a comment, question, answer or withdrawal; a description set or changed; a comment's text corrected. An edit notifies only for the mentions it adds to the text it replaces.
- Its id is derived from the operation's id and its recipient, so a rebuild finds the same one. An Agent's reaches every session of that Agent writing for that human.
- An operation an import wrote, which carries its source, creates none: a records import replays what was already said.

### Read state and names

- A Notification's read mark is a record of its own, keyed by the Notification's id, set and cleared by `tm notifications read`; `tm undo` clears it. The Issue it is about does not change.
- A person's name is a record of its own, keyed by their email and taken from git's `user.name`, written only when it differs from what tm holds.

### Out of scope

- Notifying a Claim's holder of a forced write (design.md, ADR 0009), and the conflict and lost-Claim Notifications ADR 0006 and ADR 0009 name: they create Notifications from other operations, and follow this ADR's read state and list.
- Dispatching an Agent from a mention.

### Consequences

- Good, because a mention written once notifies, renames and rebuilds with no further writes.
- Good, because reading a Notification never refuses or reorders anyone's writes to the Issue.
- Bad, because the operation format gains kinds for read marks and names, which a tm built before them cannot read.
- Bad, because text holds email addresses in tokens, which raw readers such as `tm export` and search see instead of names.
- Neutral, because the TUI and web UI can offer mentions as you type from the same resolution, and later Notification sources reuse the read marks and the list.

### Confirmation

A comment mentioning someone notifies them once and survives `tm rebuild` unread; marking it read and undoing restores it unread, and neither changes the Issue's version. Editing a description notifies only the mentions it added. A records import of mentioned text creates no Notifications. A rename shows in mentions written before it.
