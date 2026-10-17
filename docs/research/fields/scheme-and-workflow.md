# Built-in fields, the default Scheme and Workflow enforcement

- Date: 2026-10-17
- Author: Claude (research pass for Jace Babin)
- Scope: Where tm's Scheme should live, how fields, options and statuses are identified in a log that is never rewritten, where Workflow rules are enforced when Replicas merge, how built-in fields map onto the projection, and what the minimal default Scheme for bead tm-u8g.6 should contain.

Method note: sources were read on 2026-10-17. Plane and beads were cloned at the commits named under Sources and are cited by permalink. Linear's GraphQL schema was cloned from `linear/linear` at `9dd2ed80f93ae596f4f7fa21aa7cf80895c58514`. GitHub's GraphQL types were read by running read-only `gh api graphql` introspection queries, and the output is quoted as printed. Atlassian quotes marked **[spec]** were extracted from the raw Jira Cloud OpenAPI file and are exact. Other docs-site quotes passed through WebFetch, which summarises pages, so they are near-verbatim. Two probes ran on the macOS system `sqlite3` (`3.54.0 2026-04-09`) and Python 3.15.0: [scripts/scheme-probes.sql](scripts/scheme-probes.sql) and [scripts/merge_vs_workflow.py](scripts/merge_vs_workflow.py). Anything not quoted or measured is labeled **inferred**.

## Question

Bead tm-u8g.6 asks for built-in fields and a minimal default Scheme and Workflow. It is done when `tm scheme show --json` lists the default types, fields and statuses. Some things are already decided:

- Operations carry a field name and a display label: `SetField{field,label,value}` and `Add/Remove{field,label,elem}`.
- Field names match `[a-z][a-z0-9_]*`.
- Operations are never rewritten.
- The projection is rebuilt from the log in canonical order.
- Replay consults nothing but the log.

This note answers six questions:

1. Where does the Scheme live: compiled in, in a config file, or as operations in the log? What does each choice mean for rebuild determinism, for sync between Replicas with different Schemes, and for history? How do Jira, Linear, Plane and GitHub store and rename these definitions?
2. What should a stored operation reference for fields, select options and statuses, a stable id or a name? Does the answer fit the label-in-operation decision?
3. Where should transitions and per-status requirements be enforced, given that a merged log can produce a state that no single write allowed? How do other trackers behave, and what reads status categories?
4. Built-in fields: which are columns, which are sets, and which are derived from operation metadata? How is priority ordered? How does parent interact with the Scheme's parent rules?
5. What is a concrete default Scheme and Workflow, and what JSON shape should `tm scheme show --json` produce?
6. Where do design.md, the glossary or beads conflict with these findings?

## Sources

In-repo:

- [design.md](../../specs/design.md)
- [GLOSSARY.md](../../../GLOSSARY.md)
- [ADR 0003](../../decisions/0003-issue-data-lives-outside-code-repos.md), [ADR 0004](../../decisions/0004-one-authority-assigns-keys.md), [ADR 0005](../../decisions/0005-operation-log-is-the-source-of-truth.md), [ADR 0006](../../decisions/0006-operation-merge-ordering-and-versioning.md)
- [projection-schema.md](../store/projection-schema.md)
- `crates/tm-core/src/op.rs`, `crates/tm-store/src/{lib,apply,schema}.rs` on branch `feat/tm-u8g-fields` at `fc98ea1`

Jira:

- Statuses and categories:
  - <https://support.atlassian.com/jira-cloud-administration/docs/what-is-a-workflow-status/>
  - <https://confluence.atlassian.com/adminjiraserver/defining-status-field-values-938847108.html>
  - <https://developer.atlassian.com/server/jira/platform/jira-issue-statuses-as-lozenges/>
  - <https://docs.atlassian.com/software/jira/docs/api/7.6.1/constant-values.html>
- History and custom fields:
  - <https://developer.atlassian.com/server/jira/platform/database-change-history/>
  - <https://support.atlassian.com/jira/kb/how-to-identify-and-list-when-a-field-value-was-changed-in-jira/>
  - <https://developer.atlassian.com/server/jira/platform/database-custom-fields/>
  - <https://confluence.atlassian.com/servicedeskcloud/edit-a-custom-fields-options-1097176205.html>
- Workflows:
  - <https://confluence.atlassian.com/adminjiraserver/advanced-workflow-configuration-938847443.html>
  - <https://confluence.atlassian.com/adminjiraserver/configuring-workflow-schemes-938847421.html>
  - <https://confluence.atlassian.com/adminjiraserver/working-with-workflows-938847362.html>
  - <https://support.atlassian.com/jira-software-cloud/docs/manage-how-work-flows-in-your-team-managed-project/>
  - <https://support.atlassian.com/jira/kb/unable-to-do-bulk-edit-using-the-option-transition-issues/>
  - <https://support.atlassian.com/jira/kb/project-import-completed-with-error-unable-to-create-issue-due-to-field-required-validator-on-target-workflow/>
  - <https://developer.atlassian.com/server/jira/platform/database-issue-status-and-workflow/>
- Defaults, priorities and resolution:
  - <https://support.atlassian.com/jira-cloud-administration/docs/what-are-issue-statuses-priorities-and-resolutions/>
  - <https://support.atlassian.com/jira-cloud-administration/docs/what-are-issue-types/>
  - <https://support.atlassian.com/jira-cloud-administration/docs/configure-the-issue-type-hierarchy/>
  - <https://confluence.atlassian.com/adminjiraserver/associating-priorities-with-projects-939514001.html>
  - <https://support.atlassian.com/jira/kb/best-practices-on-using-the-resolution-field-in-jira-cloud/>
  - <https://support.atlassian.com/jira-software-cloud/docs/configure-columns/>
- Cloud OpenAPI: <https://developer.atlassian.com/cloud/jira/platform/swagger-v3.v3.json>

Linear:

- GraphQL schema `packages/sdk/src/schema.graphql` @ `9dd2ed80f93ae596f4f7fa21aa7cf80895c58514`. Below, `S#Lnnn` means <https://github.com/linear/linear/blob/9dd2ed80f93ae596f4f7fa21aa7cf80895c58514/packages/sdk/src/schema.graphql> at line nnn.
- Docs:
  - <https://linear.app/docs/issue-property.md>
  - <https://linear.app/docs/triage.md>
  - <https://linear.app/docs/use-cycles.md>
  - <https://linear.app/docs/delete-archive-issues.md>
  - <https://linear.app/docs/parent-and-sub-issues.md>
  - <https://linear.app/docs/issue-templates.md>
  - Third-party integration listing: <https://linear.app/integrations/required>
- Reverse-engineered sync engine, wzhudev @ `45dc91e78e03300b69cc088057a28c42cee35a41`: <https://github.com/wzhudev/reverse-linear-sync-engine>

Plane @ `879ce8b5c6b4fe21efff4789a7062747f46d6383`. Below, **P/** means `https://github.com/makeplane/plane/blob/879ce8b5c6b4fe21efff4789a7062747f46d6383/apps/api/plane/`.

- Docs: <https://docs.plane.so/workflows-and-approvals/workflows>

GitHub:

- Projects fields and automation:
  - <https://docs.github.com/en/issues/planning-and-tracking-with-projects/understanding-fields/about-single-select-fields>
  - <https://docs.github.com/en/issues/planning-and-tracking-with-projects/automating-your-project/using-the-built-in-automations>
  - <https://docs.github.com/en/issues/planning-and-tracking-with-projects/automating-your-project/using-the-api-to-manage-projects>
- Issue types, issue fields and sub-issues:
  - <https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/managing-issue-types-in-an-organization>
  - <https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/managing-issue-fields-in-your-organization>
  - <https://docs.github.com/en/rest/orgs/issue-fields>
  - <https://github.blog/changelog/2026-07-02-issue-fields-are-now-generally-available/>
  - <https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/adding-sub-issues>
- GraphQL introspection of `ProjectV2SingleSelectFieldOption`, `ProjectV2ItemFieldSingleSelectValue`, `ProjectV2SingleSelectFieldOptionInput`, `IssueType`, `IssueFieldSingleSelectOption`, `ProjectV2ItemStatusChangedEvent`, `IssueTypeChangedEvent` and `IssueFieldChangedEvent`, run with `gh api graphql`
- Community report, not official: <https://github.com/orgs/community/discussions/198803>

beads @ `46aa59cc65f34b44193b055ca489087342e55dd1`. Below, **B/** means `https://github.com/gastownhall/beads/blob/46aa59cc65f34b44193b055ca489087342e55dd1/`.

## Findings

### 1. Where the Scheme lives and how it changes

**tm's own documents already put the Scheme in config, not in the log.**

- [ADR 0003](../../decisions/0003-issue-data-lives-outside-code-repos.md) says the repo file "may carry that Project's Components, Scheme overrides and rules".
- design.md says "A Preset only writes config" and that "Field renames are explicit migrations".
- design.md's Health section expects a Scheme change to preview its impact ("makes 14 Issues non-conforming"). That describes evaluating config against existing data, not appending to a log.
- [ADR 0006](../../decisions/0006-operation-merge-ordering-and-versioning.md) lists the operation kinds as `create`, `set_field`, `add`, `remove`, `comment`, `link`, `archive` and `claim`. None of them is a Scheme change.

**Every tracker examined keeps type, field and status definitions as configuration rows referenced by id, outside issue history.**

| Tracker | Where definitions live | Identity | Rename | Delete while in use |
|---|---|---|---|---|
| Jira | Config tables and schemes ("A workflow scheme defines a set of associations … between a workflow and an issue type"); custom field options in `customfieldoption` | Numeric ids (`customfield_10001`, status id, priority id) | "When a status is renamed, the name will be updated in every workflow … that uses it" | DC: "You can only delete statuses that not are being used in workflows". Cloud team-managed: "asks you to change their status to a valid one". Issue type: "all uses are updated with the alternative issue type" **[spec]**. Options: "You cannot remove options that have already been used … but you can make them unavailable" |
| Linear | `WorkflowState` per team (S#L52794) | `id`; `Issue.state: WorkflowState!` (S#L18304) | Issues reference the state object, so they follow (**inferred**) | No delete mutation exists (`grep workflowStateDelete` found 0 matches). "Only states with issues that have all been archived can be archived" (S#L28542) |
| Plane | `State` rows per project (P/db/models/state.py#L79-L93) | UUID, plus a `slug` field | Name is unique among live rows | Refused: `"The state is not empty, only empty states can be deleted"` (P/app/views/state/base.py#L119-L140). The default state "cannot be deleted" |
| GitHub Projects | Field options on the project | Option `id` ("The option's ID."); item values hold `name` and `optionId` | The docs do not say what happens to item values | Replacing options without ids: the input id exists to preserve "the option's identity during updates, preventing item field values from being cleared" (introspection). A community report says values were "wiped out" |
| GitHub issue fields | Org-level fields | Option `id`; value `optionId` "global relay ID" | Not documented | "Options sent without an id are treated as new options and may cause existing options to be deleted and recreated"; deleting a field means "all values … are permanently removed" |

**Three places the Scheme could live.**

- **(a) Compiled-in default only (Phase 1).** The Scheme is a constant in `tm-core`.
  - Rebuild determinism holds trivially.
  - Every Replica running the same binary has the same Scheme.
  - Changing it means a new release. If a release changes a status's category, any projection column derived from the Scheme goes stale unless the release also bumps `PROJECTION_VERSION` in `schema.rs`, and nothing forces that bump. The same failure is described for config below.
- **(b) Config file (Phase 4, what design.md and ADR 0003 describe).** Workspace config holds the defaults, and the repo file holds Project overrides.
  - Replay stays a function of the log, provided the projection stores raw values only.
  - Two Replicas with different Schemes then hold identical tables. They differ only in what they derive at read time: categories, violations, and labels.
  - The repo file travels with git, but Workspace config does not. How Workspace-level Scheme config reaches a second Replica is not designed (see open questions).
- **(c) Scheme changes as operations in the log.**
  - This gives history and sync of the Scheme for free.
  - It contradicts ADR 0003's placement and "A Preset only writes config".
  - It needs a new operation kind, which is a format change under ADR 0006.
  - It makes replay order-dependent on Scheme operations. A status recategorised at `server_seq` 900 would change the meaning of operations before and after it. No tracker examined versions its Scheme inside issue history. Jira's changelog records issue changes only, and its workflow scheme switch is a migration wizard.

**The determinism trap, measured.** [projection-schema.md](../store/projection-schema.md) proposed a `status_category` column on `issues`, and noted that this makes the projection "a function of (log, Scheme)". Probe (b)–(d) in [scheme-probes.sql](scripts/scheme-probes.sql) used 10,000 Issues across two types and six statuses:

- A stored, indexed column read `SEARCH issues USING COVERING INDEX issues_category (status_category=?)` in 0.083 ms.
- A join to a 12-row Scheme table read `SCAN s` then `SEARCH i USING COVERING INDEX issues_status (type=? AND status=?)` in 0.110 ms.
- An IN-list built from the Scheme, `(type, status) IN (VALUES …)`, read `SEARCH issues USING COVERING INDEX issues_status (type=? AND status=?)`.
- All three counted 3334.
- After the Scheme moved `in_review` from active to done, the stored column still answered `stored active after edit|3334`, while the join answered `joined active after edit|1667`.

A Scheme edit therefore silently invalidates a stored category until something rebuilds. Today that something is a `PROJECTION_VERSION` bump in code, not a config edit.

**Deleted statuses.** Probe (d) removed `cancelled` from the Scheme while 1,666 Issues still held it:

- An inner join counted `8334` of `10000` Issues across all categories.
- `LEFT JOIN … WHERE s.category IS NULL` found exactly the `1666`.

An Issue in a status the Scheme no longer defines disappears from every category-based view unless the query is written to surface it. This is Jira's "If the step gets out of sync with the status, then incorrect (or no) workflow operations appear" problem, in tm's terms a violation.

### 2. Field identity: names, ids and labels

**Every tracker pairs a stable identifier with a display string, and history keeps the string.**

- **Jira:** "The OLDVALUE column records the ID of the changed entity (for example, status) while OLDSTRING records the name for the entity, so that if the entity is removed from the system, the change history … can still be displayed".
  - Custom fields in history are stored by name: "changeitem.field value contains the custom field's name at the time of the change". As a result, queries on a renamed field need "both the current and previous field's names".
  - REST `ChangeDetails` now carries `field` ("The name of the field changed.") and `fieldId` ("The ID of the field changed.") **[spec]**.
- **Plane:** a state change writes `old_value=old_state.name` and `old_identifier=old_state.id` (P/bgtasks/issue_activities_task.py#L211-L224).
- **Linear:** history stores ids, `IssueHistory.fromStateId` "Identifier of the previous workflow state … Can be used to query the workflow state directly" (S#L19837). It resolves names through the live object, so a renamed state shows its new name (**inferred**). Priority history stores the number, `fromPriority: Float` (S#L19801).
- **GitHub:** current values carry both (`ProjectV2ItemFieldSingleSelectValue.name` and `.optionId`). History events store names only:
  - `ProjectV2ItemStatusChangedEvent.previousStatus : String`
  - `IssueFieldChangedEvent.previousOptions` of type `IssueFieldTimelineOption { name, color }`, with no id
  - `IssueTypeChangedEvent` is the exception: it references `IssueType` objects.

**tm's field names already behave as stable ids.**

- A name matches `[a-z][a-z0-9_]*` (`Change::validate`), is the key in `issues.fields`, `field_versions` and `set_members`, and is the filter qualifier.
- The operation's `label` keeps history readable after the display name changes. This is Jira's OLDSTRING idea applied to the field itself.
- Field names are already slugs, so tm has no need for a second opaque id such as Jira's `customfield_10001`.

**What a rename of the name itself must do.** design.md calls this "an explicit migration". The log is never rewritten and replay never consults the Scheme, so three mechanisms are possible:

- **Alias in the Scheme** (`old_name → new_name` applied during replay). This makes replay consult the Scheme, which the decision rules out.
- **New operations.** The migration appends, per affected Issue, a `set_field` on the new name with the current value and one clearing the old name. Both carry labels, so history reads "Area → Component" naturally.
  - Replay stays pure.
  - The cost is two operations per Issue that holds the field.
  - The migration is attributed to the Actor who ran it.
  - Concurrent edits to the old name from another Replica land on a field the Scheme no longer defines, and surface as violations (**inferred**).
- **Forbid renaming names; allow only relabelling.** This is what Linear does for priority: "We don't have the option to set custom priorities".

**Values are the open point, not fields.**

- `SetField.value` is a `serde_json::Value`, and the pinned-hash test writes `"value":"active"` as a bare string.
- The operation's `label` is the field's label, not the value's. If a status, priority level, type or select option is relabelled later ("In Review" becomes "Code Review"), history can show the current label (Linear's behaviour) or the bare id. It cannot show the label at the time of the write, which Jira, Plane and GitHub all keep.

There are three ways to settle it:

- **(i) Slug ids only.** The value is the option's slug (`in_review`). History renders the current label and falls back to the slug once the option is gone. Slugs are readable, which Jira's numeric ids are not, and that weakens the need for a stored string.
- **(ii) Value as an object** `{"id":"in_review","label":"In Review"}` for enumerated fields. This matches Jira, Plane and GitHub.
- **(iii) An optional `value_label` beside `value`.**

Changing the shape after the first operation is written means a new operation version with a read-time translation (ADR 0006), so this must be decided before tm-u8g.6 writes its first status.

The recommendation is (iii): an optional `value_label` on `set_field`, `add` and `remove`, filled for enumerated values (type, status, priority, select options, persons). `value` stays the slug, so the projection and filters read one shape. The label mirrors the field `label` already decided, and every tracker examined keeps the display string in history. (i) is the fallback if payload size matters more than relabel-proof history. (ii) is not recommended, because it gives the same field two value shapes depending on its type.

**Persons.** Assignee and reporter values reference an Actor. design.md defaults the human to git `user.email` and identifies Agents by `TM_ACTOR` or environment variables. No stable Actor id exists yet, so an email change would split history (**inferred**).

### 3. Workflow enforcement under a merged log

**ADR 0006 already decides the replay side.** "A merge that leaves an Issue breaking its Workflow is applied and flagged by `tm doctor`, not rejected; rejection is reserved for permissions and purged Issues". design.md's Health section adds that non-conforming Issues "stay editable; only moving into a status that needs the missing field enforces it".

**Measured: two valid local writes can merge into a state no write allowed.** [merge_vs_workflow.py](scripts/merge_vs_workflow.py) replays acknowledged operations using the same decision as `Version::beats` in `apply.rs`, with a Workflow where `in_progress` and `in_review` require `area`:

```text
--- 1: transition and field clear meet
  A writes status='in_progress': allowed
  B writes area=None: allowed
merged state: {'status': 'in_progress', 'area': None} | missing required: ['area']
--- 2a: two status paths, A's later clock
  A writes status='in_progress': allowed
  A writes status='in_review': allowed
  B writes status='cancelled': allowed
merged state: {'status': 'in_review', 'area': 'cli'} | missing required: []
  B sees cancelled -> in_review: not a transition
--- 2b: two status paths, B's later clock
merged state: {'status': 'cancelled', 'area': 'cli'} | missing required: []
  A sees in_review -> cancelled: not a transition
```

- Scenario 1 breaks a per-status requirement through a different field, so checking only status operations would not catch it.
- Scenarios 2a and 2b show that the final status was reached by a legal transition from the shared ancestor, but is an illegal step from what one Replica last showed its user.

The simulation models the rule; it does not run `tm-store`, which has no sync path yet.

**Other trackers mostly do not enforce transitions, and none reports invalid states afterwards.**

- **Jira** enforces only on the transition path. Conditions control "whether a transition should be executed by the user", and validators "check that any input made to the transition is valid before the transition is performed".
  - Edits cannot change status: "issue transition is not supported and is ignored here" **[spec]**.
  - Bulk transitions run validators and can fail silently ("the workflow had a validator, where a custom date field needs to have a value").
  - Project import runs Create-transition validators.
  - A workflow scheme switch forces a status mapping: "you must provide `mappingsByIssueTypeOverride`" **[spec]**.
  - No "invalid state" report was found. The closest is the DC internals note that an out-of-sync step makes "incorrect (or no) workflow operations appear".
- **Linear** has no transition rules in the schema or docs (not found). Required fields before a status change are sold by a third-party integration that "automatically reverts the issue back to its previous state". The only native requirement is "require priority to be set before an issue leaves Triage".
- **Linear's client** applies writes optimistically and undoes them when the server rejects: "If the server rejects the mutation query … the transaction will trigger its `rollback` method" (wzhudev README L1207; `html.js` L80636–L80645). So the server is the single enforcement point. tm has no equivalent for offline Replicas, because ADR 0006 applies every pulled operation.
- **Plane OSS** has no transition enforcement (grep for `transition|allowed_states|workflow` found only prose and reserved slugs). The docs place "transition conditions" in the Enterprise Grid tier. The only state-driven side effect is `completed_at` when `state.group == COMPLETED` (P/db/models/issue.py#L238-L253).
- **GitHub Projects** has no transitions or required fields. Built-in workflows set Status "to Done" when items close or merge.
- **beads** has no transition rules (grep found nothing outside its Jira integration). Opt-in validation exists (`validation.on-create`, `validation.on-close`, all off by default, B/internal/config/config.go#L311-L319), and a close guard refuses closing a parent with open children unless `--force`.

**Where the checks belong (recommendation, consistent with ADR 0006).** The write-time check runs in the command layer before `Store::append`, against the local projection and the local Scheme. It never runs inside `apply`, which replay shares. That leaves several write paths to classify:

| Path | Check transitions and requirements? |
|---|---|
| `tm move`, `tm create`, `tm set` by a human or Agent | Yes. Refuse with the structured error design.md shows (`missing_fields` with a `fix`) |
| Replay, `tm rebuild`, pulled operations | Never. Flag violations at read time |
| `tm undo` (appends an inverse) | Open. An inverse transition may not be a legal transition; Jira has no undo to compare |
| `tm import` | design.md says import "validates it like any other write". Jira's project import runs validators too. Historical statuses in imported history may need to bypass transitions while still checking requirements (**inferred**) |
| Automation rules | Open. Jira's prompted status changes "won't execute any rules" when statuses are deleted |
| Scheme migration operations | They write values on the user's behalf, so they bypass transitions (**inferred**) |

**Status categories and what reads them.**

| Tracker | Categories |
|---|---|
| Jira | Three fixed (To Do `new`, In Progress `indeterminate`, Done `done`) plus `undefined`. "this can't be customized". Completion is also carried by the separate Resolution field: "Jira considers that the lifecycle of an issue has come to an end when the "Resolution" field has a value" |
| Linear | `triage`, `backlog`, `unstarted`, `started`, `completed`, `canceled`, `duplicate` (S#L52867). Teams "remove statuses (as long as at least one status exists in each category)" |
| Plane | `backlog`, `unstarted`, `started`, `completed`, `cancelled`, `triage` (P/db/models/state.py#L14-L20) |
| beads | Custom statuses carry `active`, `wip`, `done` or `frozen` (B/internal/types/types.go#L629-L677) |
| tm | `triage`, `todo`, `active`, `done` (design.md) |

The other trackers keep "cancelled" separate from "completed" in one of two ways. Linear and Plane use separate categories. Jira uses Resolution.

Readers found:

- Linear cycles: "An active status is a status in the "Unstarted" and "Started" category"; auto-add of "Started or Completed issues".
- Linear auto-archive of "completed, canceled, or auto-closed" Issues.
- Linear Triage, which is excluded "from all views".
- Plane's `completed_at`, and its default manager, which "excludes triage states".
- Jira boards, lists and reports, and JQL `statusCategory`.

design.md names these tm readers:

- Triage and default Views (exclude triage)
- The ready queue (todo, unblocked, unclaimed)
- Release notes (done)
- `tm start`'s move "to the first active status"
- Blockers clearing (a blocker in done)
- The parent-follows-children rule
- "stale in progress" (active)

### 4. Built-in fields in the projection

The `issues` table is currently `id, project, created_hlc, fields JSON` (`schema.rs`), with the comment "Built-in typed columns arrive with the Scheme". The proposed mapping:

| Built-in | Projection | Written by | Notes |
|---|---|---|---|
| Key | `keys` table (exists) | `create` plus `assigned_key` | Not a column ([projection-schema.md](../store/projection-schema.md) §4) |
| type | `issues.type TEXT` | `set_field type` | Slug. Changing type can leave status outside the new type's Workflow; Jira forces a status mapping on such moves (`mappingsByIssueTypeOverride`) |
| title | `issues.title TEXT` | `set_field` | LWW |
| description | `issues.description TEXT` | `set_field` | Whole-field LWW (ADR 0006) |
| status | `issues.status TEXT` | `set_field` | Slug. Category derived at read time (§1) |
| priority | `issues.priority INTEGER` rank | `set_field` with a level slug | See below |
| Assignee | `issues.assignee TEXT` | `set_field` | Actor reference |
| reporter | `issues.reporter TEXT` | `create` operation's `actor`, or `set_field reporter` if editable | See the conflict in §6 |
| Agent and Claim | `claims` table (exists) | `claim` | Not `issues` columns; expiry compared with now at read |
| parent | `issues.parent_id BLOB` | `set_field parent` with the parent's UUID | UUID, not Key (ADR 0004) |
| labels | `set_members` with `field = 'labels'` | `add` and `remove` | Per-element merge (ADR 0006) |
| archived | `issues.archived INTEGER` | `set_field archived` | Not in design.md's built-in list. ADR 0006 calls archive "an ordinary field" but also lists an `archive` operation kind |
| created | `issues.created_utc_ms` | `create` operation's `utc_ms` | `created_hlc` already exists for ordering |
| updated | `issues.updated_utc_ms` | max `utc_ms` of operations applied to the Issue | Whether comments count is a choice; Linear and Jira bump updated on comments (**inferred**, not checked) |
| history | query over `ops` by `(issue_id, field, elem)` | none | `ops_target` index exists |

Under a pure projection, created and updated are derived from operation metadata, not from `set_field`. Under concurrent sync, "max `utc_ms`" uses the writer's clock. The capped HLC rule in ADR 0006 does not apply to it, so a skewed clock can push `updated` into the future (**inferred**).

**Priority ordering.** design.md fixes five levels (none, urgent, high, medium, low) with renameable labels.

| Source | Encoding | Sort |
|---|---|---|
| Linear | `priority: Float!` "0 = No priority, 1 = Urgent, 2 = High, 3 = Medium, 4 = Low" (S#L18162) | A separate `prioritySortOrder` exists, and "items without an assigned priority level are now always sorted last" |
| Plane | Strings `"urgent" … "none"` | A `Case/When` annotation over `PRIORITY_ORDER = ["urgent", "high", "medium", "low", "none"]` with `output_field=CharField()` (P/utils/order_queryset.py#L8-L166) |
| beads | Integer 0–4 with "0=highest" (B/cmd/bd/flags.go#L337) | — |
| Jira | Global priority list, reordered with `PUT /priority/move` **[spec]** | "The order from priority schemes is not relevant for JQL" |

Probe (a) stored both forms:

- Text sorted `high, low, medium, none, urgent`.
- A rank with `urgent=4, high=3, medium=2, low=1, none=0` sorted correctly with `ORDER BY priority_rank DESC` (`SCAN issues USING COVERING INDEX issues_priority`).
- The filter `priority:>=high` became `priority_rank >= 3` with `SEARCH issues USING COVERING INDEX issues_priority (priority_rank>?)`.

Ranking "more important = larger" makes `>=` read naturally and puts `none` last in a descending sort. The level set is fixed in code, not in the Scheme, so the slug-to-rank mapping is compiled in and the column stays deterministic. Labels can be renamed in config without touching the projection.

**Parent and "which types may be parents of which".**

- Jira fixes hierarchy levels ("level 1, by default called Epic", "level 0, called Story", "level -1, called Subtask"), and changes "will break existing parent and child relationships".
- Linear and Plane have no type rule. Plane validates only same-project parents (P/app/serializers/issue.py#L178-L185).
- GitHub allows "eight levels of nested sub-issues" and "up to 100 sub-issues per parent" with no type rule.
- beads caps depth at `hierarchy.max-depth` 3 and enforces it by counting dots in IDs.
- design.md: "hierarchy limits are off unless configured".

Parent rules are therefore a write-time check like transitions. Merges can break them in two ways:

- Two Replicas concurrently set `X.parent = Y` and `Y.parent = X`. Each write is legal locally, and per-field LWW keeps both because they are different Issues' fields. The result is a cycle.
- One Replica changes a parent's type to one that may not have children.

Recursive parent queries need a depth bound, and `tm doctor` should report parent cycles (**inferred** from the merge rule; not run against the store).

### 5. A minimal default Scheme and Workflow

**What others ship.**

| Product | Default types | Default statuses (category) | Transitions |
|---|---|---|---|
| Jira Cloud | Epic, Story, Task, Bug, Subtask | Scrum board: To Do, In Progress, Done; classic workflow: "Open, In Progress, Resolved, Closed, and Reopened" | Classic: explicit; simplified: each column maps to its status |
| Linear | None (no issue types found in the schema) | "Backlog > Todo > In Progress > Done > Canceled" plus system "Duplicate"; Triage optional | None |
| Plane | `IssueType` exists in OSS with `is_epic` and `level`, no shipped defaults read | Backlog (default), Todo, In Progress, Done, Cancelled, Triage | None in OSS |
| GitHub | Task, Bug, Feature | Projects Status: Todo, In Progress, Done (seen only in an API example) | None |
| beads | bug, feature, task, epic, chore, decision, message, molecule, gate, spike, story, milestone | open, in_progress, blocked, deferred, closed, pinned, hooked | None |

**Recommendation.** One shared default Workflow for all seven types, any-to-any transitions, no per-status requirements, and hierarchy unrestricted. This is the smallest Scheme that exercises every concept `tm scheme show` must print without inventing policy Phase 4 owns. The statuses:

| Status slug | Label | Category |
|---|---|---|
| `triage` | Triage | triage |
| `todo` | Todo | todo |
| `in_progress` | In Progress | active |
| `in_review` | In Review | active |
| `done` | Done | done |
| `cancelled` | Cancelled | done |

Notes on this set:

- **Starting status.** It is `todo`, so a human's `tm create` is visible in default Views. A Triage start would hide every new Issue, given that "Triage Issues are left out of default Views". Agents filing into Triage choose `--status triage`, or a later per-Actor default (Phase 4).
- **Any-to-any transitions.** They match Linear, Plane OSS and GitHub. Rules from design.md that name statuses, such as review and ready for testing, stay off by default.
- **`in_review`.** design.md's rules move Issues "to review", so the default has a review status to target.
- **`cancelled` sits in done** because tm has no cancelled category. See §6.

Types use slugs matching the field-name pattern: `epic`, `story`, `task`, `bug`, `sub_task`, `spike` and `design`. Labels are "Epic", "Story", "Task", "Bug", "Sub-task", "Spike" and "Design".

Built-in fields are listed with `builtin: true`, so the same JSON shape later carries Scheme fields. The proposed `tm scheme show --json` shape is in Raw data. `tm scheme show <type> --json` would return one element of `types` with its Workflow and fields inlined.

### 6. Conflicts found

1. **The glossary and design.md disagree on default types.**
   - design.md lists Epic, Story, Task, Bug, Sub-task, Spike and Design.
   - The glossary's "Default issue types" section defines only Epic, Story, Task, Bug and Sub-task.
2. **The glossary's Scheme omits Workflows.**
   - Glossary: "The named set of issue types, their fields and their allowed parents".
   - design.md: "A Scheme defines issue types, their fields, their Workflows and which types may be parents of which".
3. **Categories versus status names in shipped rules.** design.md says "Cross-type features … read categories, never status names". Yet its shipped automation rules name statuses:
   - "a PR opening moves the Issue to review"
   - "all done moves it to review"
   - "Issues in ready for testing"

   `review` and `ready for testing` are not categories. Either the rules take a configured target status, or a category is missing.
4. **No cancelled category.**
   - tm's four categories put cancelled work in done.
   - Linear (`canceled`, plus `duplicate`) and Plane (`cancelled`) separate it, and Jira separates it through Resolution.
   - Release notes "generated per Release from the Issues it shipped" and "all Sub-tasks done notifies" would both count cancelled Issues as shipped or finished, unless a resolution-like field or a fifth category exists.
5. **The projection-schema recommendation to store `status_category` on `issues`** conflicts with "replay consults nothing but the log", unless the Scheme is folded into the projection fingerprint. Probe (c) shows the stale result.
6. **Reporter cannot be derived as design.md specifies.**
   - design.md: "Agent writes are recorded as "agent, on behalf of human"".
   - `Op` has one `actor: String`, and the envelope is pinned by `PINNED_OP_HASH`.
   - Recording on-behalf-of needs either an envelope change (a new `ENVELOPE_FORMAT`) or an encoding inside `actor`. Both must be settled before operations are written outside tests.
7. **"Triage" says untyped, but every Issue has a type.** The glossary describes Triage Issues as "not yet been accepted, typed or placed". If the default Scheme always sets a type, "typed" never applies, so either `type` is nullable in triage or the definition drops "typed".
8. **beads priority is inverted relative to tm and Linear.**
   - beads: "0=critical … 4=backlog", default 2.
   - Linear: 0 means no priority.
   - The beads importer (Phase 1) needs an explicit mapping. beads has no "none", so `0→urgent, 1→high, 2→medium, 3→low, 4→low or none` is a choice to make (**inferred**).
   - beads types `feature`, `chore`, `decision`, `message`, `molecule`, `gate` and `milestone` have no default tm type. The glossary lists Chore as a term to avoid for Task.
9. **Built-in fields do not fit design.md's field types.**
   - design.md's types are "text, rich text, number, date, boolean, single select, multi select, person, version, URL, Issue reference, file paths and code anchor".
   - Status needs Workflow and category semantics, and priority needs fixed ranked levels. Neither is a plain single select.
   - Labels are free tags, not a multi select with allowed values.
   - Type is the Scheme's own type list.
   - Either built-ins get their own types (`status`, `priority`, `issue_type`, `labels`, as in the proposed JSON) or they are modelled as selects with extra rules.
10. **The bead's description cites a stale path.** It refers to "docs/design.md"; the file is `docs/specs/design.md`. This was read from fragments of the beads Dolt backup with `strings`, since bead commands were not run, so the text may be incomplete.

## Conclusions

- **The projection should stay a function of the log alone.** Store raw slugs for type and status. Derive category, violations and labels at read time from the Scheme.
  - The cost is negligible: 0.110 ms joined versus 0.083 ms stored at 10,000 Issues, and the IN-list form uses the same composite index.
  - The alternative needs a Scheme hash in the projection fingerprint, and a rebuild on every Scheme edit.
  - Priority is the exception that may be a typed column, because its levels are fixed in code.
- **Phase 1 needs only a compiled-in default.** Config-file Schemes (Phase 4) fit ADR 0003 and keep replay pure. Scheme-as-operations contradicts ADR 0003 and no tracker examined does it.
- **Field names are the stable ids, and labels are display.** The current operation shape covers field renames for display. The unsettled part is value labels for enumerated fields; Jira, Plane and GitHub all keep one in history.
- **Enforcement is write-time and local only.** Merges can produce states no write allowed; the simulation shows two ways. That is consistent with ADR 0006, but the read side must surface violations, including Issues whose status the Scheme no longer defines (probe d).
- **No tracker examined resolves invalid states after the fact.** Jira blocks at the transition, Linear rolls back on server rejection, and Plane, GitHub and beads do not enforce at all. tm's "apply and flag" has no direct precedent to copy.

## Implications / actions

The following are recommendations, not decisions.

- For tm-u8g.6:
  - A `Scheme` type in `tm-core` with the compiled-in default above, serialised by `tm scheme show --json` in the Raw data shape.
  - `issues` columns for the built-ins in the §4 table, with `priority` as an integer rank and no `status_category` column.
  - Category filters emitted as `(type, status) IN (…)` from the Scheme.
- **Before the first real operation is written:**
  - Decide the value shape for enumerated fields (slug only, object, or `value_label`).
  - Decide how "agent, on behalf of human" is recorded in the envelope.
  - Reserve the built-in field names (`key`, `type`, `title`, `description`, `status`, `priority`, `assignee`, `reporter`, `agent`, `claim`, `parent`, `labels`, `archived`, `created`, `updated`) so Scheme fields cannot take them. Linear similarly reserves label names such as "priority", "status" and "state".
- **A write-time validation layer** above `Store::append` that reads the local Scheme. A `violations[]` computation at read time that covers missing required fields, unknown statuses, disallowed parents and parent cycles.
- **Candidate ADR:** "The projection does not depend on the Scheme". It would supersede projection-schema.md's `status_category` column and settle Scheme-as-config versus Scheme-as-operations.
- **Glossary and design.md fixes** for conflicts 1–4, 7 and 9 in §6, to be decided by the author.

## Open questions

- **Value labels.** Should enumerated values carry their label at write time, as Jira, Plane and GitHub history do, or rely on readable slugs? The recommendation is `value_label` (§2), but it is not decided.
- **Field renames.** Should renaming a field's name be supported (migration operations) or forbidden (relabel only)? If supported, how are concurrent writes to the old name from an offline Replica reported?
- **Scheme sync.** How does Workspace-level Scheme config reach another Replica, and what does `tm sync status` say when two Replicas' Schemes differ?
- **Cancelled.** A fifth category, a resolution field, or neither? It affects release notes, "all Sub-tasks done" and the ready queue.
- **`tm undo` and automation rules.** Do they bypass transitions and requirements, or are they refused like any other write?
- **`updated` time.** Which operations bump it (comments? claims?), and whether to use writer `utc_ms` or capped HLC time under skew.
- **Type changes.** Do they require a status mapping when the new type's Workflow lacks the current status, as Jira's move does?
- **Actor identity.** Is an email-keyed Actor stable enough for Assignee and reporter history?
- **Measurements not made.** The simulation models the merge rule but does not exercise `tm-store`, which has no sync path yet.

## Raw data

### Proposed `tm scheme show --json` shape

This is the full default. Fields marked `derived` are not set by `set_field`: they come from operation metadata or other tables, and are listed so that filters and agents can discover them.

```json
{
  "schema_version": 1,
  "scheme": { "name": "default", "source": "built_in", "hash": "sha256:…" },
  "status_categories": ["triage", "todo", "active", "done"],
  "priorities": [
    { "id": "urgent", "label": "Urgent", "rank": 4 },
    { "id": "high", "label": "High", "rank": 3 },
    { "id": "medium", "label": "Medium", "rank": 2 },
    { "id": "low", "label": "Low", "rank": 1 },
    { "id": "none", "label": "None", "rank": 0 }
  ],
  "fields": [
    { "name": "key", "label": "Key", "type": "key", "builtin": true, "derived": true },
    { "name": "type", "label": "Type", "type": "issue_type", "builtin": true, "required": true },
    { "name": "title", "label": "Title", "type": "text", "builtin": true, "required": true },
    { "name": "description", "label": "Description", "type": "rich_text", "builtin": true },
    { "name": "status", "label": "Status", "type": "status", "builtin": true, "required": true },
    { "name": "priority", "label": "Priority", "type": "priority", "builtin": true, "default": "none" },
    { "name": "assignee", "label": "Assignee", "type": "person", "builtin": true },
    { "name": "reporter", "label": "Reporter", "type": "person", "builtin": true, "derived": true },
    { "name": "agent", "label": "Agent", "type": "person", "builtin": true, "derived": true },
    { "name": "claim", "label": "Claim", "type": "claim", "builtin": true, "derived": true },
    { "name": "parent", "label": "Parent", "type": "issue_reference", "builtin": true },
    { "name": "labels", "label": "Labels", "type": "labels", "builtin": true, "multi": true },
    { "name": "archived", "label": "Archived", "type": "boolean", "builtin": true, "default": false },
    { "name": "created", "label": "Created", "type": "datetime", "builtin": true, "derived": true },
    { "name": "updated", "label": "Updated", "type": "datetime", "builtin": true, "derived": true }
  ],
  "workflows": [
    {
      "name": "default",
      "start": "todo",
      "statuses": [
        { "id": "triage", "label": "Triage", "category": "triage", "required_to_enter": [] },
        { "id": "todo", "label": "Todo", "category": "todo", "required_to_enter": [] },
        { "id": "in_progress", "label": "In Progress", "category": "active", "required_to_enter": [] },
        { "id": "in_review", "label": "In Review", "category": "active", "required_to_enter": [] },
        { "id": "done", "label": "Done", "category": "done", "required_to_enter": [] },
        { "id": "cancelled", "label": "Cancelled", "category": "done", "required_to_enter": [] }
      ],
      "transitions": "any"
    }
  ],
  "types": [
    { "id": "epic", "label": "Epic", "workflow": "default", "fields": [], "parents": "any" },
    { "id": "story", "label": "Story", "workflow": "default", "fields": [], "parents": "any" },
    { "id": "task", "label": "Task", "workflow": "default", "fields": [], "parents": "any" },
    { "id": "bug", "label": "Bug", "workflow": "default", "fields": [], "parents": "any" },
    { "id": "sub_task", "label": "Sub-task", "workflow": "default", "fields": [], "parents": "any" },
    { "id": "spike", "label": "Spike", "workflow": "default", "fields": [], "parents": "any" },
    { "id": "design", "label": "Design", "workflow": "default", "fields": [], "parents": "any" }
  ]
}
```

- `transitions` is `"any"` or a list of `{ "from": …, "to": [...] }`.
- `parents` is `"any"` or a list of type ids.
- Both shapes carry Phase 4 Schemes without a format change.
- The shape is close to Jira's createmeta field entry, which requires `fieldId`, `key`, `name`, `operations`, `required` and `schema` **[spec]**:

  ```json
  {"fieldId":"assignee","hasDefaultValue":false,"key":"assignee","name":"Assignee","operations":["set"],"required":true}
  ```

### Probe output excerpts

From `sqlite3 p.db < scheme-probes.sql`, verbatim:

```text
high|2000
low|2000
medium|2000
none|2000
urgent|2000
4|urgent|2000
3|high|2000
2|medium|2000
1|low|2000
0|none|2000
`--SCAN issues USING COVERING INDEX issues_priority
`--SEARCH issues USING COVERING INDEX issues_priority (priority_rank>?)
`--SEARCH issues USING COVERING INDEX issues_category (status_category=?)
|--SCAN s
`--SEARCH i USING COVERING INDEX issues_status (type=? AND status=?)
|--SEARCH issues USING COVERING INDEX issues_status (type=? AND status=?)
`--LIST SUBQUERY 4
   `--SCAN 4-ROW VALUES CLAUSE
in-list active|3334
stored active|3334
joined active|3334
Run Time: real 0.000083 user 0.000079 sys 0.000012
Run Time: real 0.000110 user 0.000105 sys 0.000005
stored active after edit|3334
joined active after edit|1667
inner join, all categories|8334
left join, no category|1666
total issues|10000
```

The single timings are from one run each and only indicate the order of magnitude.

### Plane default states (P/db/models/state.py#L24-L62)

Backlog (15000, backlog, default), Todo (25000, unstarted), In Progress (35000, started), Done (45000, completed), Cancelled (55000, cancelled), Triage (65000, triage).

### Linear `WorkflowState.type` (S#L52867)

"The type of the state. One of "triage", "backlog", "unstarted", "started", "completed", "canceled", "duplicate"." The create input accepts only "backlog, unstarted, started, completed, canceled" (S#L52950).
