# Task Management

A local-first tracker for work planned and carried out by one human and their AI agents.

## Language

### Work

**Issue**:
Any tracked unit of work, whatever its type.
_Avoid_: Item, work item, ticket, bead

**Key**:
The human-readable identifier of an Issue, made of its Project key and a number, such as CPL-42; earlier Keys stay valid as aliases.
_Avoid_: ID, number, handle

**Link**:
A typed relationship between two Issues, such as blocks or duplicates, separate from parent and child.
_Avoid_: Relation, dependency

**Gate**:
A blocker that waits on an outside event, such as a pull request merging, rather than on another Issue.
_Avoid_: Check, condition

**Question**:
A decision only a person can make that an Issue waits on, asked with `tm ask`, which may offer options, and settled with `tm answer` or, by its asker, `tm withdraw`; the Issue is waiting on that person until then.
_Avoid_: Blocker, request, prompt

**Issue type**:
A configurable category of Issue, such as Epic or Bug, that sets which fields and parents the Issue may have.
_Avoid_: Kind, category

**Workflow**:
The statuses an issue type moves through, the allowed transitions between them, and the status a new Issue starts in.
_Avoid_: Lifecycle, process

**Status category**:
One of the fixed groups every status belongs to: triage, todo, active, done or cancelled.
_Avoid_: State, phase

**Triage**:
The status category for Issues that have not yet been accepted, typed or placed by a human or rule.
_Avoid_: Inbox, backlog, intake

**Finding**:
A review observation anchored to code and attached to the Issue under review; it becomes an Issue of its own only when deferred.
_Avoid_: Review comment, alert, note

**Check**:
A command attached to an Issue, together with the result that shows the work is done.
_Avoid_: Test, acceptance criterion, done-check

**Evidence**:
The recorded outcome of running an Issue's Checks: what ran, the result, the commit it ran at, who ran it and when.
_Avoid_: Proof, report, verification

### Default issue types

**Epic**:
A large body of work that groups other Issues toward one outcome.
_Avoid_: Initiative, feature

**Story**:
An Issue described from the user's point of view, delivering visible value.
_Avoid_: Feature

**Task**:
An Issue for technical work that is not framed as user value.
_Avoid_: Chore

**Bug**:
An Issue that corrects behaviour that differs from what was intended.
_Avoid_: Defect

**Sub-task**:
A piece of another Issue, tracked separately.
_Avoid_: Child task, checklist item

**Spike**:
An Issue for research or investigation whose outcome is knowledge, not shipped work.
_Avoid_: Research task, investigation

**Design**:
An Issue whose outcome is a design, such as a mock-up or a written approach, rather than code.
_Avoid_: Mock-up, RFC

### People and agents

**Actor**:
Whoever made a change, either a human or an Agent.
_Avoid_: User, author

**Assignee**:
The human accountable for an Issue.
_Avoid_: Owner

**Agent**:
An AI tool, such as Claude, Codex or Copilot, that carries out work on an Issue on behalf of its Assignee.
_Avoid_: Bot, delegate, worker

**Claim**:
An Agent's exclusive, expiring hold on an Issue while it works on it.
_Avoid_: Lock, lease, checkout

**Notification**:
A personal notice to an Actor that something needs their attention, such as being mentioned; separate from Triage.
_Avoid_: Alert, inbox item

### Organisation

**Project**:
A named container of Issues that may contain other Projects. Its Project key, an uppercase letter then 1 to 9 uppercase letters or digits such as CPL, begins each of its Issues' Keys.
_Avoid_: Repo

**View**:
A saved, named selection of Issues with its own filter and layout, picked from a sidebar.
_Avoid_: Saved search, query, dashboard

**Board**:
A View laid out as columns of statuses, each column optionally narrowed by its own filter.
_Avoid_: Kanban

**Swimlane**:
An optional horizontal band on a Board grouping Issues by one field's value, such as Ships in, with its own optional filter.
_Avoid_: Row, group

**Component**:
A named part of a Project that has its own version line, such as one plugin in a repo of plugins.
_Avoid_: Package, module, sub-project

**Release**:
One version of a Component, or of a Project that has no Components, such as review-cycle@2.3.0.
_Avoid_: Version, fix version, milestone

**Planned bump**:
The size of version change (patch, minor or major) an Issue is expected to cause in a Component.
_Avoid_: Bump label, semver level

**Ships in**:
The Release an Issue's work is delivered in, first as a plan and then as fact once the work is in a tagged Release.
_Avoid_: Fix version, target version

**Found in**:
The Release in which a Bug was observed.
_Avoid_: Affects version, affected release

**Iteration**:
An optional time-boxed period of work in a Project, shown under whatever name the team prefers, such as sprint or cycle.
_Avoid_: Sprint, cycle (as canonical terms)

**Short-list**:
A named, ordered list of Issues in a Project that says what to do next and in what order, apart from priority, with the Actors who work through it; an Issue sits on at most one.
_Avoid_: Queue, inbox, to-do list

**Preset**:
A named bundle of Scheme, Workflows, Views and automation rules offered when a Project is set up.
_Avoid_: Template, profile

**Scheme**:
The named set of issue types, their fields, their Workflows and their allowed parents that a Project follows; child Projects inherit it unless they override it.
_Avoid_: Template, schema, config

**Replica**:
One copy of a Workspace's store on one machine, identified separately from the Actors who write to it.
_Avoid_: Device, node, machine, client

**Workspace**:
A self-contained store of Projects and their Schemes, either global to a machine or created separately for isolation.
_Avoid_: Database, instance, org

**Snapshot**:
The read-only copy of a Workspace's store that each write publishes, which a reader that may not open the store, such as a sandboxed Agent, reads instead; it is as current as the last write that published it.
_Avoid_: Backup, cache, mirror, export
