# Human-first commercial trackers: Jira Cloud, Linear, GitHub Issues + Projects, Shortcut

Research date: 2026-10-16. About 25 tool calls. Coverage is uneven. Linear and GitHub came mostly from primary docs and changelogs. Jira is partly primary (support.atlassian.com, the atlassian-mcp-server repo) and partly secondary. Shortcut got only a brief pass. Anything not fetched from a primary page is marked as such.

## 1. Storage/sync model, offline support, concurrency

### Takeaway

Linear is the only one of the four with a client-side replicated data model, and its internals are documented mostly by reverse engineering. Jira, GitHub and Shortcut are conventional server-authoritative SaaS apps. Agents reach all four only over the network, through API or MCP. None of them gives you a local, git-native copy of the data that you can read.

### Cited Findings

- **Linear (secondary/reverse-engineered):** the client keeps an in-memory object graph (MobX) persisted to IndexedDB. Writes apply locally first, and a server transaction is queued in the background. The first load comes from a bootstrap endpoint and later loads are deltas. Each model has a load strategy: loaded at bootstrap, on demand, or local-only. The engine is described as neither OT nor CRDT, and it supports partial sync, permissions, offline availability and edit history. Sources: [reverse-linear-sync-engine study](https://docsearch.algolia.com/mcp/docs/repo/wzhudev/reverse-linear-sync-engine), [performance.dev "What makes Linear feel instant"](https://performance.dev/what-makes-linear-feel-instant), [braindetox write-up](https://braindetox.kr/en/posts/linear_local_first_speed_2026.html).
- **Linear conflict resolution (secondary):** conflicts resolve last-writer-wins per property, so concurrent edits to different fields both survive, with no conflict detection, OT or CRDT. Linear CTO Tuomas Artman is quoted in a reverse-engineering repo's README calling it "probably the best documentation that exists", but that endorsement doesn't cover conflict resolution. Sources: [Aslam97/reverse-linear-sync-engine](https://github.com/Aslam97/reverse-linear-sync-engine), [Paul Hovley Substack](https://paulhovley.substack.com/p/technically-speaking-how-400-million), [HN comment](https://hn.svelte.dev/item/44123131).
- Linear's official offline doc URL (`linear.app/docs/offline`) returned 404 when fetched, so I could not confirm the official offline guarantees or conflict semantics. [linear.app/docs/offline (404)](https://linear.app/docs/offline)
- **GitHub concurrency** is limited server-side. No more than 100 concurrent requests are allowed across REST and GraphQL. Secondary limits also cap CPU time at 90 s per 60 s, and content creation at 80 requests per minute and 500 per hour. [GitHub REST rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api)

### Inferences

- Linear's "instant" UX comes from optimistic local writes against a client replica. A tracker built for agents could copy the pattern with a local replica that an agent reads directly, such as SQLite or files. That would remove the network round-trip that all four commercial tools impose on agents.
- With no OT or CRDT in Linear, concurrent edits to the same field probably resolve last-writer-wins at the server. This is inferred and was not confirmed in official docs.

### Gaps

- Linear's official statement on offline editing (desktop vs. web), on reconnect conflict handling, and on the 2023–2024 sync-engine engineering talks was not retrieved.
- Jira and GitHub optimistic concurrency was not researched: whether there are ETags or version fields on updates. Jira issue edits are believed to have no optimistic locking on most fields, but this is unverified.
- Jira and GitHub have no offline mode as far as I know, but no primary source was checked.

## 2. Typed/custom fields, required fields, validation, workflow rules at creation

### Takeaway

Jira has the richest enforcement model, with workflows, validators and screens, though the details here are unverified. In July 2026 GitHub finally added typed org-level issue fields on top of issue types and Projects fields. Linear keeps a fixed core schema (status, priority, estimate, labels, cycle, project) plus a few enforcement knobs.

### Cited Findings

- **GitHub issue fields** reached GA on 2026-07-02 for all org plans (Free, Team, Enterprise, GHEC with data residency) and ship in GHES 3.23. There are four types: single select, text, number and date. Fields can be pinned to issue types. Each org gets four preconfigured fields: Priority, Effort, Start date and Target date. The limit is 25 fields per org, set up under Settings > Planning > Issue fields by org owners only. REST and GraphQL APIs exist, plus `field_added` and `field_removed` webhooks. More than 40k orgs adopted it during the preview. [GA changelog](https://github.blog/changelog/2026-07-02-issue-fields-are-now-generally-available/), [public preview](https://github.blog/changelog/2026-05-21-issue-fields-are-now-in-public-preview-for-all-organizations/), [org docs](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/managing-issue-fields-in-your-organization)
- A third-party write-up says issue fields are also exposed over MCP for AI access. This is secondary. [windowsforum](https://windowsforum.com/threads/github-issue-fields-ga-typed-priority-dates-mcp-ai-access-for-better-planning.433581/)
- **GitHub Projects fields:** text, number, date, single select and iteration custom fields, plus issue fields, parent issue and sub-issue progress, linked pull request, reviewers and issue type. [Understanding fields](https://docs.github.com/en/issues/planning-and-tracking-with-projects/understanding-fields)
- **GitHub CLI** v2.94.0 (changelog 2026-06-10) can manage sub-issues, issue types and dependencies, and `gh issue list` can filter by type. [changelog](https://github.blog/changelog/2026-06-10-manage-sub-issues-types-and-dependencies-from-github-cli/)
- **Linear enforcement:** a team can require priority before an issue leaves Triage. [Linear Triage docs](https://linear.app/docs/triage)
- **Linear** added team-level project labels and statuses and "durable project and initiative IDs" (2026-09-24), and added initiative properties (proposed/canceled status, priority, labels) on 2026-07-02. [Linear changelog](https://linear.app/changelog)
- **Jira workflow rules** can trigger agents: "Add rule" → "Trigger agent action" on a transition. Setting this up requires the Space admin role and the "Edit workflows" permission. [Atlassian: Collaborate on work items with AI agents](https://support.atlassian.com/jira-software-cloud/docs/collaborate-on-work-items-with-ai-agents/)

### Inferences

- GitHub's typed fields are org-scoped and capped at 25, so they suit a small shared schema rather than a per-repo schema. Validation is limited to type, with no conditional required fields.
- None of the three, as far as was verified, has a first-class "acceptance criteria" or "definition of done" field type that is machine-checked.

### Gaps

- Jira custom field types (select, multi-select, user, date, number, labels, cascading select, and so on), required-field configuration via field configuration schemes, and workflow validators, conditions and post-functions were not fetched from primary docs. Believed from general knowledge, unverified for 2026: Jira Cloud supports per-transition validators (field required) and conditions. Team-managed vs. company-managed projects differ in these capabilities.
- I did not confirm whether Linear has a general-purpose custom-fields feature as of Oct 2026. It historically lacked one, but this is unverified.
- Whether GitHub issue fields can be marked required on creation or in issue forms was not verified.

## 3. Git/branch/PR integration

### Takeaway

Linear has the most granular automation. Branch-name auto-linking, closing and non-closing magic words, and per-team, per-target-branch status rules cover draft, open, review requested, ready-to-merge and merged. GitHub closes issues natively, but only for PRs into the default branch. Jira links by issue key and uses Smart Commits for transitions and comments.

### Cited Findings

- **Linear:**
  - Including the issue ID in the branch name auto-links the PR. "Copy git branch name" is bound to Cmd/Ctrl+Shift+. and can optionally move the issue to Started on copy.
  - Closing words: close/closes/closed/closing, fix/fixes/fixed/fixing, resolve/complete/implement variants.
  - Non-closing words: ref, refs, references, part of, contributes to, toward(s). These move the issue through other statuses but don't apply the merge status.
  - "relates to" links without any status change. "skip" or "ignore" before an ID suppresses linking.
  - Status defaults: PR open → In Progress, merge → Done. Configurable events are draft, open, review requested, ready for merge and merged, set per team.
  - "Ready for merge" fires only when GitHub reports a stable, mergeable PR, so any failing check blocks it.
  - Rules match the target branch and accept regex. With multiple PRs on one issue, the issue moves only when the last PR reaches the state. One PR can close many issues ("Fixes ENG-123, DES-5").
  - Commit linking via magic words needs a webhook, and is not available on GHES.
  - [Linear GitHub docs](https://linear.app/docs/github)
- **Linear (Oct 2026):**
  - Linear Diffs shows stacked PRs with review, CI and merge status.
  - Live diffs update during agent coding sessions.
  - Guided Reviews became GA on 2026-07-30 and reached Basic on 2026-10-08.
  - Review requests to GitHub teams appear in the Reviews inbox.
  - Coding sessions support signed commits.
  - [Linear changelog](https://linear.app/changelog)
- **GitHub:**
  - Closing keywords are close/closes/closed, fix/fixes/fixed and resolve/resolves/resolved. Per the docs they "can be followed by colons or in uppercase."
  - They work in the PR body or in commit messages, but only when the PR targets the default branch. Otherwise no link is made and the merge has no effect.
  - Cross-repo syntax is `Fixes owner/repo#100`.
  - You can manually link up to 10 issues per PR through the Development sidebar.
  - A commit-message close does not list the PR as linked.
  - [GitHub: Linking a PR to an issue](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/linking-a-pull-request-to-an-issue)
- **Jira:**
  - Linking works when the work item key appears in a branch name (`JRA-123-...`), a commit message or a PR title.
  - Keys are case-sensitive and must be uppercase.
  - A branch or PR created from the Development panel gets the key automatically.
  - Sync can take a few minutes after a push.
  - It works with Bitbucket Cloud, GitHub, GitHub Enterprise and GitLab.
  - [Atlassian: Reference work items in development work](https://support.atlassian.com/jira-software-cloud/docs/reference-issues-in-your-development-work/)
- **Jira Smart Commits:**
  - Syntax is `<KEY> #<command> [args]`.
  - The commands are `#comment` and `#time`, plus any workflow transition name used as a hashtag (e.g. `JRA-090 #close #comment Fixed`).
  - A command must fit on one line, though several can share a line.
  - The committer email must match a single Jira user with permission.
  - [Atlassian: Process work items with smart commits](https://support.atlassian.com/jira-software-cloud/docs/process-issues-with-smart-commits/), [Bitbucket Smart Commits](https://support.atlassian.com/bitbucket-cloud/docs/use-smart-commits/)

### Inferences

- Linear's model of closing words, non-closing words, aggregation across all linked PRs, and gating on a stable mergeable state is the best template to borrow. GitHub's default-branch-only rule is a frequent footgun for release-branch workflows.
- Jira's email-matching requirement for Smart Commits is a problem for agent commits made under bot identities.

### Gaps

- Jira automation triggers ("Pull request merged", "Branch created") and their pricing-tier run limits were not fetched.
- Shortcut's VCS integration was not covered. Shortcut is believed to support story IDs in branch names (e.g. `sc-123`) and workflow-state transitions on PR events, but this is unverified.

## 4. Release/version awareness

### Takeaway

Linear Releases (Business+) is the most CI-native. A CLI or GitHub Action scans commits for issue IDs on deploy and attaches the issues to a release in a pipeline. Jira fix versions and GitHub milestones/releases were not fetched from primary sources in this pass.

### Cited Findings

- **Linear Releases:**
  - A release belongs to a pipeline (a product/environment combination) and has a name, a commit SHA and a set of issues.
  - **Continuous pipelines:** each deploy is one release, auto-created on a push to main. The release action scans new commits for IDs like ENG-123, attaches those issues, and marks the release complete. Path globs such as `mobile-ios/**` scope a pipeline in a monorepo.
  - **Scheduled pipelines:** have dates and stages, can freeze started stages, and carry open issues over to the next release.
  - Auth uses a per-pipeline access key; personal API keys aren't accepted. Tooling is the open-source `linear-release` CLI and `linear/linear-release-action`.
  - Release completion can move issues to Done. Issues linked only through "contributing" PRs are not moved.
  - Linear Agent can generate release notes, and each pipeline has a changelog tab.
  - Business plan allows up to 15 pipelines; Enterprise is unlimited.
  - [Linear Releases docs](https://linear.app/docs/releases)
- The Linear MCP server gained release management (2026-07-02), and Linear Agent can update releases (2026-08-13). [Linear changelog](https://linear.app/changelog)

### Inferences

- Linear ties releases to commit SHAs plus ID scanning, which is effectively "what shipped is derived from git". It is a strong model for agents because it needs no manual version tagging.

### Gaps

- Jira fix versions / Releases hub and Jira's deployment tracking (Bitbucket Pipelines/Bamboo by default per [Atlassian](https://support.atlassian.com/jira-software-cloud/docs/reference-issues-in-your-development-work/)) were not detailed.
- GitHub milestones and releases were not fetched. GitHub milestones are believed to have no link to releases or tags, but this is unverified.
- Linear cycles were not fetched.

## 5. Triage inbox

### Takeaway

Linear Triage is a dedicated per-team inbox with keyboard actions, rules, on-call rotation and AI routing. The rules, rotation and AI are Business+. Jira has agents on workflow and board columns but intake was not researched. GitHub relies on issue forms plus labels, which was also not fetched.

### Cited Findings

- **Linear Triage:**
  - It is an inbox for issues from integrations or from non-team members, enabled per team, and reached with `G T`.
  - Triage issues are excluded from views by default.
  - Keyboard actions: `1` accepts (moves to the default status), `2` declines (moves to Canceled), `3`/`MM` marks a duplicate (moves attachments and customer requests to the canonical issue), and `H` snoozes until a time or new activity.
  - Rules run top-down, can set team, status, assignee, label, project or priority, and re-apply when an issue moves to another team's triage.
  - Triage Responsibility notifies or auto-assigns, can rotate via PagerDuty, OpsGenie, Rootly or Incident.io, and offers a public API for custom schedules.
  - Triage Intelligence is agentic AI that suggests assignee and labels and finds duplicates and related issues.
  - Rules, Responsibility and Intelligence require Business or Enterprise.
  - [Linear Triage](https://linear.app/docs/triage)
- The Linear Priority inbox tab (2026-09-03) separates notifications that need attention from those that can wait. [Linear changelog](https://linear.app/changelog)

### Inferences

- "Triage is excluded from default views" and "require priority before leaving triage" act as soft gates that keep half-specified items out of the work queue. An agent tracker could gate on completeness the same way, for example by requiring acceptance criteria before an item is ready.

### Gaps

- Jira intake (JSM forms, Jira Product Discovery, "Intake" in Jira Cloud) and GitHub issue forms/templates (YAML forms with required inputs) were not fetched in this pass.

## 6. Agent interface: MCP, CLI, API, agent-specific features

### Takeaway

As of October 2026, all three majors treat agents as first-class assignees:

- **Linear:** agent apps act as *delegates* while the human stays assignee, and Linear Agent runs its own coding sessions.
- **Jira:** Rovo and third-party agents go in the assignee field or on workflow transitions and board columns, consuming Rovo credits.
- **GitHub:** the Copilot cloud agent is assigned like a user and opens a draft PR.

All three have official remote MCP servers. GitHub has the best first-party CLI (`gh` with `--json`). Linear's official CLI is release-only.

### Cited Findings

#### Linear

- **Agent apps:** install with an OAuth app using `actor=app`, which needs admin. Scopes are `app:assignable` (can be a delegate on issues and projects) and `app:mentionable`. An agent can't request the `admin` scope.
- **Delegate model:** assigning an issue to an agent sets the issue's `delegate`, not its `assignee`, so a human stays accountable.
- **Sessions:** a mention or delegation creates an AgentSession and sends a `created` webhook. The agent must emit a `thought` activity within 10 s. Session state is derived from the activities the agent emits. `promptContext` provides formatted issue and comment context.
- **Billing:** agents are not billable users, and development is free.
- Source: [Linear developers: Agents](https://linear.app/developers/agents)
- Sessions can be waiting for input, working, completed or errored. Source: [Linear agent interaction SDK post](https://linear.app/blog/our-approach-to-building-the-agent-interaction-sdk) (via search summary).
- **MCP server:**
  - `https://mcp.linear.app/mcp` (Streamable HTTP), with a read-only variant at `/mcp/readonly`. The SSE endpoint `/sse` is deprecated.
  - Auth is OAuth 2.1 with DCR, or a bearer API key or token. Only the `read` scope yields read-only access.
  - Each workspace needs a separate auth context.
  - Okta enterprise-managed auth arrived on 2026-07-02 and 2026-08-13.
  - Source: [Linear MCP docs](https://linear.app/docs/mcp), [changelog](https://linear.app/changelog)
- **Linear Agent and coding sessions:**
  - Linear Agent can write code using Claude Code and Codex. This was announced 2026-06-11 according to a search-result summary only. No page was fetched, and the date falls before the visible changelog entries, so it is unverified.
  - Coding sessions gained environments, browser use and usage-based pricing with sandbox runtime charges (2026-08-20).
  - Model routing, environment secrets and open-weight models arrived 2026-09-24.
  - Live diffs, plus AI credit spend limits per workspace, user or loop, arrived 2026-10-08.
  - Loops, recurring agent workflows on a schedule or event, arrived 2026-07-20.
  - Copilot for Linear lets you assign issues to Copilot's cloud agent (2026-07-30).
  - Source: [Linear changelog](https://linear.app/changelog)
- GitHub's Copilot cloud agent for Linear became GA on 2026-07-23. It opens a draft PR and streams progress to the Linear activity timeline. [GitHub changelog](https://github.blog/changelog/2026-07-23-copilot-cloud-agent-for-linear-is-now-generally-available/)
- **API rate limits:**
  - GraphQL API: an API key gets 2,500 requests per hour and 3M complexity points per hour. An OAuth app gets 5,000 requests per hour and 2M points per hour per user.
  - Unauthenticated access gets 600 requests per hour per IP.
  - Complexity: a property is 0.1, an object is 1, and connections multiply by `first` (default 50). The maximum is 10,000 points per query. Exceeding a limit returns HTTP 400 `RATELIMITED`.
  - Source: [Linear rate limiting](https://linear.app/developers/rate-limiting)
- The `agentSessionCreateOnIssue` mutation is documented by a third party as `[Internal]`, so it is unverified. [withone.ai](https://www.withone.ai/knowledge/linear/conn_mod_def%3A%3AGJ4vBrN7waY%3A%3A5a4sDFy1RQGP6UnEFKDnTw/md)
- **CLI:** the only official CLI found is `linear-release`. Community CLIs exist, e.g. `lincli`. [Linear Releases](https://linear.app/docs/releases), [lincli](https://docs.rs/lincli/2026.4.5)

#### Jira / Atlassian

- **Agents on work items:**
  - Four ways to engage: the assignee field via the Agent picker (Favorites, Coding agents, Others), an @mention in a comment, a workflow transition rule ("Trigger agent action"), or a board column (team-managed only).
  - Third-party coding agents appear in the picker even when not installed; admins install them and users can request them. "GitHub Copilot coding agent, the first of many partner agents" is listed, and there is a link to "Set up Claude Agent for Jira".
  - Agent output is private to the invoker until shared as a draft comment. Only the invoker can interact directly.
  - Each use consumes Rovo credits. Allowances apply from 2026-12-03, admins are alerted at 80% and 100%, and overage is billed.
  - Source: [Atlassian: Collaborate on work items with AI agents](https://support.atlassian.com/jira-software-cloud/docs/collaborate-on-work-items-with-ai-agents/)
- Search summaries said Agents in Jira went GA at Team '26 but also described the help page as "beta". The fetched page itself states no beta status. These conflict. [Techzine](https://www.techzine.eu/news/devops/139112/atlassian-gives-jira-and-rovo-agents-an-agentic-upgrade/)
- **Rovo MCP server:**
  - The recommended endpoint is `https://mcp.atlassian.com/v2/mcp`; v1 still works. `?tools=all` returns a flat tool list for gateways. The SSE endpoint is unsupported after 2026-06-30.
  - Auth is OAuth 2.1, or headless API tokens: personal Basic `email:token` or a service-account Bearer token, which an admin must enable.
  - Jira tool groups: `read_jira`, `write_jira` and `search_jira`, plus `delete_jira` and `manage_jira`, which are off by default.
  - Admins get domain allowlists, IP allowlists and an audit log (search "MCP"). Only "Primary" tools are shown to agents, and others are discovered on demand.
  - Source: [atlassian/atlassian-mcp-server](https://github.com/atlassian/atlassian-mcp-server)
- The Rovo MCP server is GA for Jira, Confluence and Compass, with JSM and Bitbucket on the roadmap at GA time. [Atlassian Community GA announcement](https://community.atlassian.com/forums/discussion/3186690/announcing-ga-of-the-atlassian-rovo-mcp-server)
- **REST rate limits:**
  - Points-based hourly quotas for Forge, Connect and OAuth 3LO apps were enforced starting 2026-03-02, alongside per-second burst limits; API-token traffic stays on the burst limits. The response headers are `RateLimit-Policy` and `RateLimit`.
  - Developers complain that one bucket is shared across tenants.
  - Source: [Jira rate limiting](https://developer.atlassian.com/cloud/jira/platform/rate-limiting), [community thread](https://community.developer.atlassian.com/t/2026-point-based-rate-limits/97828)
- An Atlassian–OpenAI partnership was announced on 2026-10-06. Secondary sources say agents autonomously picking up Jira work is still "being designed". [cellcog (secondary)](https://cellcog.ai/blog/atlassian-openai-partnership/)

#### GitHub

- **Copilot cloud ("coding") agent:**
  - Started from the agents panel, a conversation, or an issue or PR. It researches, plans, edits, and runs tests and linters in a cloud environment.
  - Limits: one repo per task, one branch and one PR per task, and a hard 59-minute maximum execution time. Works with GitHub-hosted repos only.
  - Repository rules such as commit-author restrictions can block it, and admins can disable it per repo.
  - Source: [GitHub Docs: About Copilot cloud agent](https://docs.github.com/en/copilot/concepts/agents/coding-agent/about-coding-agent)
- Secondary sources add more behavior:
  - It reacts with 👀, creates a `copilot/` branch and opens a draft PR.
  - It sees the issue title, body and comments only as of assignment time.
  - It can be assigned via the UI, Projects, Mobile, `gh issue edit`, or GraphQL by checking `suggestedActors` for `copilot-swe-agent`.
  - Source: [Microsoft Learn](https://learn.microsoft.com/en-us/training/modules/github-copilot-code-agent/3-assign-track-troubleshoot-copilot-code-agent-tasks), [GitHub blog](https://github.blog/ai-and-ml/github-copilot/assigning-and-completing-issues-with-coding-agent-in-github-copilot/)
- A secondary source says Codex and Claude are available inside "Agent HQ", and the coding agent is disabled by default on Business and Enterprise. [danielvaughan (secondary)](https://codex.danielvaughan.com/2026/03/28/codex-github-copilot-coding-agent-issue-assignment/)
- **GitHub MCP server:**
  - Official server with remote and local variants. Default toolsets are context, repos, issues, pull_requests and users; others are enabled via `--toolsets`, `GITHUB_TOOLSETS` or the `X-MCP-Tools` header.
  - Remote-only tools include `assign_copilot_to_issue` (optional `base_ref`, custom instructions since 2026-01) and `create_pull_request_with_copilot`.
  - The Projects toolset was consolidated to `projects_get`, `projects_list` and `projects_write` (2026-01).
  - Source: [github/github-mcp-server](https://github.com/github/github-mcp-server), [remote-server.md](https://github.com/github/github-mcp-server/blob/main/docs/remote-server.md), [changelog 2026-01-28](https://github.blog/changelog/2026-01-28-github-mcp-server-new-projects-tools-oauth-scope-filtering-and-new-features/)
- A secondary claim says the hosted MCP server requires a Copilot license for OAuth. Unverified. [instawp](https://instawp.com/what-is-github-mcp/)
- **REST rate limits:**
  - A PAT gets 5,000 requests per hour per user, shared across that user's tokens.
  - A GitHub App installation gets 5,000 to 12,500 per hour. `GITHUB_TOKEN` gets 1,000 per hour per repo, and GHEC gets 15,000.
  - Secondary limits: 900 REST and 2,000 GraphQL points per minute, and 80 content-creating requests per minute or 500 per hour. A write costs 5 points.
  - Source: [GitHub REST rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api)

#### Shortcut

- Shortcut has an official MCP server that can find, create and update Stories (including comments and sub-tasks), Epics and Docs. It names Claude Code, Windsurf, Cursor and Zed as clients. [Shortcut MCP help](https://www.shortcut.com/help/integrations/mcp-server), [Shortcut blog](https://www.shortcut.com/blog/why-we-built-the-shortcut-mcp-server/)
- The package is reportedly `@shortcut/mcp` and uses an API token. This comes from a third-party directory and is unverified. [glama](https://glama.ai/mcp/servers/x67vlqhqek)

### Inferences

- Linear's delegate-vs-assignee split, which keeps human accountability alongside an agent executor, plus streamed "activities" (thought, action, and so on) and a 10 s acknowledgement SLA, is the most mature agent-interaction protocol of the four. It is a good model for a tracker that serves both humans and agents.
- All of these agent features are cloud-executed and metered: Rovo credits, Linear AI credits and sandbox runtime, Copilot premium usage. A local agent such as Claude Code driving the tracker gets only the generic MCP or API path with its rate limits.
- Write budgets are low for heavy agent use. GitHub allows 80 content-creating requests per minute, and the Linear API key's 2,500 requests per hour is about 0.7 per second.

### Gaps

- Linear AgentActivity types beyond `thought` (action, elicitation, response, error) were not confirmed from the fetched page.
- GitHub `gh issue`/`gh project` `--json` field coverage for issue fields was not fetched. `gh` is believed to support `--json`, `--jq` and `--template`, but this was not verified in this pass.
- Exact tool lists for the Linear MCP server: the docs page lists none. A third-party source claims 22 to 42 tool definitions costing about 12.8k to 19.6k tokens. [Quandri](https://quandri.io/engineering-blog/mcp-is-dead), [lincli](https://docs.rs/lincli/2026.4.5)
- Copilot cloud agent plan and pricing (premium requests, Actions minutes) is not on the fetched page.

## 7. Human UI quality

### Takeaway

Linear's UI is keyboard-first and leans on its local-first sync, as the triage single-key actions and `G T` navigation show. The other UIs were not researched in depth.

### Cited Findings

- Linear has single-key triage actions (`1`, `2`, `3`, `H`), `G T` navigation and `Cmd/Ctrl+Shift+.` to copy a branch name. [Linear Triage](https://linear.app/docs/triage), [Linear GitHub](https://linear.app/docs/github)
- Linear can share a filtered view by URL with `Cmd/Ctrl+Shift+C` (2026-07-02). Coding-session diffs and steering work on mobile (2026-07-30). [Linear changelog](https://linear.app/changelog)
- Linear's speed is attributed to local reads and writes against IndexedDB with optimistic updates. [performance.dev](https://performance.dev/what-makes-linear-feel-instant)

### Gaps

- No primary or practitioner sources were gathered on Jira or GitHub Projects UI ergonomics, such as Jira slowness complaints or GitHub Projects views (table, board, roadmap).

## 8. Privacy/self-hosting and pricing gates

### Takeaway

Jira Data Center reaches end of life on 2029-03-28. After that date, GitHub Enterprise Server will be the only self-hosting path among the majors, and GitHub issue fields ship in GHES 3.23. Linear is cloud-only, and its key dev and agent features are on Business ($16) and above.

### Cited Findings

- **Jira Data Center:** Atlassian announced on 2025-09-08 that all Data Center products reach end of life on 2029-03-28. New-customer sales stopped in March 2026, and existing customers can buy licenses or expansions until 2028-03-30. These dates are from secondary sources and are consistent across them. [getint](https://www.getint.io/blog/jira-data-center-end-of-life), [stride.page](https://www.stride.page/blog/jira-data-center-end-of-life), [frox](https://www.frox.ch/en/newsroom/blog-articles/jira-data-center-end-of-life-2029-what-to-do/)
- **GitHub Enterprise Server:** issue fields arrive in GHES 3.23. [GitHub changelog](https://github.blog/changelog/2026-07-02-issue-fields-are-now-generally-available/) Linear's commit linking is not available on GHES. [Linear GitHub](https://linear.app/docs/github)
- **Linear pricing:**
  - Free is $0 with a 250-issue cap and 2 teams. Basic is $10 per user per month (annual) with 5 teams. Business is $16 per user per month with unlimited teams. Enterprise is custom, annual only.
  - SAML is Enterprise-only. Triage Intelligence and Code Intelligence are Business+, and Loops is listed under Business.
  - Coding sessions and Loops require AI credits. Linear Agent is listed on Free.
  - Source: [Linear pricing](https://linear.app/pricing)
- **Linear gates:** Releases are Business and Enterprise only (15 pipelines on Business). Triage rules, responsibility and intelligence are Business+. Guided Reviews became available on Basic on 2026-10-08. [Linear Releases](https://linear.app/docs/releases), [Linear Triage](https://linear.app/docs/triage), [changelog](https://linear.app/changelog)
- Linear Enterprise gets MCP Okta-managed auth. Admin pre-approval of third-party apps is on paid plans. [Linear changelog](https://linear.app/changelog)
- **Jira:** agents consume Rovo credits, with allowances from 2026-12-03. [Atlassian](https://support.atlassian.com/jira-software-cloud/docs/collaborate-on-work-items-with-ai-agents/)

### Gaps

- Jira Cloud prices and tier gates: the pricing page fetch was truncated. The plans are Free, Standard, Premium and Enterprise. [Atlassian pricing](https://www.atlassian.com/software/jira/pricing)
- GitHub plan gates for Copilot cloud agent, and for Projects on GHES, were not verified.
- Shortcut pricing was not researched.

## 9. Known limitations and complaints (solo devs / agent-heavy)

### Takeaway

The recurring pain points are:

- network-only access, with no local read path
- MCP context bloat
- per-hour API budgets
- metered AI credits on every vendor's native agent
- integration quirks, such as GitHub's default-branch-only closing and Jira's email-matched Smart Commits

### Cited Findings

- **MCP context cost:** Linear's MCP was measured at more than 12,800 tokens for 42 tool definitions in one practitioner post. Another source claims 22 tools and about 19.7k tokens from a 25-run benchmark. Treat both as practitioner or vendor claims. The same post cites MCP being about 3x slower per call than a CLI, measured on Jira's MCP. [Quandri](https://quandri.io/engineering-blog/mcp-is-dead), [lincli](https://docs.rs/lincli/2026.4.5)
- One practitioner argues that a CLI beats MCP when the agent has a shell, a good CLI exists, and the user is a developer on their own machine. [Descope](https://www.descope.com/blog/post/mcp-vs-cli)
- **Linear MCP friction:** switching workspaces requires a separate auth context. Internal errors may need `rm -rf ~/.mcp-auth`, and dropped connections need a manual reconnect. [Linear MCP](https://linear.app/docs/mcp)
- **Linear limits:** 2,500 requests per hour on an API key, and HTTP 400 `RATELIMITED` when exceeded. [Linear rate limiting](https://linear.app/developers/rate-limiting)
- **Jira:** new points-based quotas, and a bucket shared across tenants. [community](https://community.developer.atlassian.com/t/2026-point-based-rate-limits/97828) Dev-panel sync "may take a few minutes". [Atlassian](https://support.atlassian.com/jira-software-cloud/docs/reference-issues-in-your-development-work/)
- **Jira agents:** output is private to the invoker until shared, and only the invoker can interact. [Atlassian](https://support.atlassian.com/jira-software-cloud/docs/collaborate-on-work-items-with-ai-agents/)
- **Copilot cloud agent:** 59-minute cap, one repo, one PR, GitHub-hosted repos only. [GitHub Docs](https://docs.github.com/en/copilot/concepts/agents/coding-agent/about-coding-agent) Secondary sources say it doesn't see issue comments added after assignment. [Microsoft Learn](https://learn.microsoft.com/en-us/training/modules/github-copilot-code-agent/3-assign-track-troubleshoot-copilot-code-agent-tasks)
- **Cost:** the Linear Free plan caps out at 250 issues, which agents generating many small tasks would hit fast. [Linear pricing](https://linear.app/pricing)

### Inferences

- For an agent-heavy solo developer, Linear Free's 250-issue cap is the tightest economic constraint. API budgets are the tightest technical one. A local-first, git- or file-backed tracker removes both.

### Gaps

- No Reddit or HN complaint threads specific to Linear or Jira for solo developers were retrieved. Practitioner sentiment is under-sampled.

## 10. Linking items to file paths/code, review findings, verifying completion criteria

### Takeaway

None of the four has first-class "file path" or "acceptance-criteria verification" fields. Code linkage runs through PRs and commits. The closest things to verification are Linear's "ready for merge" gate on a stable, mergeable PR with passing checks, its rule that an issue moves only when all linked PRs reach the state, and Linear Diffs and Guided Reviews inside the tracker.

### Cited Findings

- **Linear PR gating:** "Ready for merge" fires only when GitHub reports a stable, mergeable PR, so any failing check blocks it. An issue with multiple PRs changes state only when the last one reaches it. [Linear GitHub](https://linear.app/docs/github)
- **Linear code review in the tracker:**
  - Diffs are pinned in the issue sidebar (2026-07-23).
  - Line comments on mobile (2026-07-30).
  - Stacked PRs with CI and merge status, and live agent-session diffs (2026-10-08).
  - Rich previews of images, fonts, Markdown and PDFs in diffs (2026-09-03).
  - Guided Reviews.
  - Source: [Linear changelog](https://linear.app/changelog)
- **Linear release path filters:** glob-scoping by file paths (`mobile-ios/**`) is the only file-path-aware mechanism found. [Linear Releases](https://linear.app/docs/releases)
- **Jira agents:** work-item agents use summary, description, comments and attachments, and may pull other work items as context. [Atlassian](https://support.atlassian.com/jira-software-cloud/docs/collaborate-on-work-items-with-ai-agents/)
- **GitHub:** the Projects "Pull request" field shows linked PRs and reviewers. Sub-issue progress is a field. [GitHub Projects fields](https://docs.github.com/en/issues/planning-and-tracking-with-projects/understanding-fields)

### Inferences

- These gaps leave room for a new tracker:
  - acceptance criteria as structured, checkable items, such as commands to run with expected results
  - file and path anchors on tasks
  - review findings as linked child items with a resolution state
- These would go beyond "PR merged = done", which all incumbents use as a proxy for completion.

### Gaps

- Whether GitHub code-scanning or PR review comments can be converted into issues with file and line anchors was not researched. GitHub supports "reference in new issue" from a code line, but this is unverified in this pass.
- Jira "Code" or Compass component linkage to repo paths was not researched.
