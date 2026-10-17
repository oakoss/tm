# Open-source and self-hostable human-first issue trackers (as of 2026-10-16)

Method note: GitHub/Codeberg stats and release dates below came from live `gh api` / Codeberg API calls run on 2026-10-16 (quoted values). Items marked **(inferred)** come from prior knowledge and were not re-verified in this session; treat them as leads to confirm.

## Tech stack, storage, deployment footprint, sync/real-time, offline

### Takeaway
Only Plane and Gitea/Forgejo/Vikunja are light enough to be a solo dev's daily driver; Huly needs 8 GB+ and a distributed stack (CockroachDB + Redpanda + Elasticsearch) and its upstream repo is now frozen. None of these offer a true offline-first local store a CLI agent could write to directly; all are server-authoritative web apps.

### Cited Findings
- **Plane**: TypeScript-dominant monorepo, 60,955 stars, AGPL-3.0, last commit 2026-10-16; latest release v1.4.2 (2026-08-23), v1.4.3-rc1 (2026-10-06) — `gh api repos/makeplane/plane` run 2026-10-16 ([repo](https://github.com/makeplane/plane)).
- Plane Docker Compose minimum: "2 cores", "4GB (8GB recommended for production)"; advanced install lets you "customize the database, Redis, storage"; recommends external DB and storage for production — [Plane self-hosting docs](https://developers.plane.so/self-hosting/methods/docker-compose).
- Plane self-hosted "Commercial" edition Free plan gives 12 seats per workspace without license key; Pro/Business/Enterprise keys unlock paid features; a separate Community Edition installs via `setup.sh` — [Plane self-hosting docs](https://developers.plane.so/self-hosting/methods/docker-compose).
- Plane backend is Django + PostgreSQL + Redis/Valkey + RabbitMQ + MinIO, frontend React/Next-style, with a separate "live" server (Y.js/Hocuspocus) for collaborative pages **(inferred, not verified this session)**.
- **Huly**: upstream `hcengineering/platform` (27,928 stars, EPL-2.0) README now says "**This repository is frozen and is no longer actively maintained.** Development continues in Platform-Collective/platform" — README fetched via `gh api` 2026-10-16 ([repo](https://github.com/hcengineering/platform)). Last upstream release v0.7.426 (2026-07-05).
- Successor `Platform-Collective/platform`: 86 stars, EPL-2.0, created 2026-06-26, last push 2026-10-15 — `gh api` 2026-10-16 ([repo](https://github.com/Platform-Collective/platform)).
- Huly self-host requirements: "Minimum: 2 vCPUs and 8 GB RAM; Recommended: 4 vCPUs and 16 GB RAM"; persistent volumes for Elasticsearch, CockroachDB ("Data storage for workspaces and accounts"), Redpanda ("Data storage for Kafka"); README warns bundled CockroachDB and Redpanda configs "might not be production-ready" — [huly-selfhost README](https://github.com/hcengineering/huly-selfhost) (3,575 stars, last push 2026-09-29).
- Huly desktop app has a workspace Backup option that can be restored into a self-hosted instance — [Huly self-host docs](https://docs.huly.io/getting-started/self-host/).
- Huly is a real-time collaborative platform (transactions pushed over websockets to clients) **(inferred)**.
- **Vikunja**: Go, 5,771 stars, AGPL-3.0-or-later (desktop/ is GPL-3.0-or-later); releases v2.6.0 (2026-08-31), v2.7.0 (2026-10-02); last commit 2026-10-16 — `gh api` + README ([repo](https://github.com/go-vikunja/vikunja)). Config schema lists `sqlite` as a database type (grep of `config-raw.json`); MySQL/Postgres also supported and it ships as a single Go binary **(inferred)**. OpenAPI docs published at try.vikunja.io/api/v2/docs — [README](https://github.com/go-vikunja/vikunja).
- **OpenProject**: Ruby (Rails), 16,446 stars, GPL-3.0; releases v17.9.1 (2026-10-01), v17.9.2 (2026-10-15) — `gh api` ([repo](https://github.com/opf/openproject)). PostgreSQL-only, packaged as Docker all-in-one or DEB/RPM **(inferred)**.
- **Gitea**: Go, 58,534 stars, MIT; v28.0.0 (2026-09-29), v28.1.0 (2026-10-06) — `gh api` ([repo](https://github.com/go-gitea/gitea)). Single binary with SQLite/Postgres/MySQL **(inferred)**.
- **Forgejo**: Codeberg-hosted, 5,660 Codeberg stars, GPL-3.0 (LICENSE header "GNU GENERAL PUBLIC LICENSE Version 3"); v16.0.6 (2026-10-12), v17.0.0 (2026-10-15) — Codeberg API run 2026-10-16 ([repo](https://codeberg.org/forgejo/forgejo)).
- **YouTrack Server**: proprietary (JetBrains); docs current at "YouTrack Server 2026.2" — [YouTrack MCP docs](https://www.jetbrains.com/help/youtrack/server/model-context-protocol-server.html). JVM app with embedded Xodus DB, single Docker image **(inferred)**.
- **Taiga** (backend `taigaio/taiga-back`): 857 stars on that repo, last push 2026-09-28, no GitHub releases listed — `gh api` ([repo](https://github.com/taigaio/taiga-back)); AGPL-3.0, free to self-host — [opensourcealternatives.to](https://www.opensourcealternatives.to/alternative-to/linear).
- **Planka** (Trello-style kanban): 12,618 stars, license reported as NOASSERTION by GitHub (custom/fair-code license); v2.2.1 (2026-08-10) — `gh api` ([repo](https://github.com/plankanban/planka)).

### Inferences
- A solo-dev tracker that wants a small footprint should look like Gitea/Vikunja (single binary + SQLite) rather than Plane (4-8 GB, 5+ services) or Huly (8-16 GB, distributed DB + Kafka).
- Huly's real-time transaction model is architecturally interesting, but its governance collapse (hosted shutdown + frozen repo + small fork) makes it a cautionary tale about depending on a VC-funded open-core project.

### Gaps
- Offline support: no source fetched confirms offline-first modes for any of these tools; Vikunja has a PWA and CalDAV, but offline write support was not verified.
- Plane's exact service list and real-time transport not verified from primary source this session.

## Typed/custom fields, required fields, validation, workflow rules

### Takeaway
YouTrack and OpenProject have the richest typed-field + workflow-rule engines; Plane added work item types and custom properties (paid tiers); Vikunja and Gitea/Forgejo have essentially no custom fields (labels only).

### Cited Findings
- YouTrack's MCP exposes a "get field schema" tool for issues, indicating per-project typed custom field schemas are first-class — [YouTrack MCP docs](https://www.jetbrains.com/help/youtrack/server/model-context-protocol-server.html).
- YouTrack workflows are JavaScript rules (on-change guards, state machines, scheduled rules) that can enforce required fields **(inferred)**.
- Plane AI gained workspace-level tools for managing work item types and properties (June 2026) — [Plane changelog via search](https://plane.so/changelog/2026-06-15-ai-block-in-pages); work item types/custom properties are a paid-tier Plane feature **(inferred)**.
- Vikunja feature page lists views (List, Gantt, Kanban, Table), saved filters, colored labels, CalDAV, and imports from Todoist/Trello/Microsoft To-Do; it does not mention custom fields — [Vikunja features](https://vikunja.io/features).
- OpenProject supports typed custom fields per work package type, required flags, and a role x type status-transition workflow matrix **(inferred)**.
- Gitea/Forgejo issues have labels (including scoped/exclusive labels), milestones, assignees, projects boards, and dependencies; no custom typed fields **(inferred)**. Gitea's MCP `issue_read` returns "details, comments, labels, dependencies" — [gitea-mcp](https://gitea.com/gitea/gitea-mcp).

### Inferences
- The gap a new tracker can fill: typed fields + validation (YouTrack/OpenProject-grade) in a lightweight, Linear-speed tool. Required-field enforcement on state transition (e.g., "cannot move to Done without `release` and `test_evidence`") is the key mechanism to borrow for agent guardrails.

### Gaps
- Did not verify whether Plane Community Edition includes work item types/custom properties.

## Git/branch/PR integration and auto-close

### Takeaway
Gitea/Forgejo have native, zero-config commit/PR keyword auto-close (issues live next to code). Plane's GitHub integration is a Pro feature with a nice mechanism: bracketed IDs trigger state automation, bare IDs only link. Vikunja has no git integration.

### Cited Findings
- Plane GitHub integration "is marked as a Pro feature"; self-hosted requires creating your own GitHub App. `[WEB-344]` in a PR title/description links + comments + enables state automation; bare `WEB-345` links only. State mappings are configurable per PR lifecycle event: draft created, opened, review requested, approved, merged, closed without merging — [Plane GitHub docs](https://docs.plane.so/integrations/github).
- Gitea has configurable `CloseKeywords` and `ReopenKeywords` repository settings (seen in `modules/setting/repository.go` via curl 2026-10-16) and a references parser that matches close/reopen keyword patterns — [Gitea source](https://github.com/go-gitea/gitea/blob/main/modules/references/references.go). Defaults are close/closes/closed/fix/fixes/fixed/resolve/resolves/resolved **(inferred)**.
- GitLab is cited as suiting teams already hosting code there with "native commit-to-issue automation" — [opensourcealternatives.to](https://www.opensourcealternatives.to/alternative-to/linear) (aggregator).
- OpenProject has GitHub and GitLab integrations showing PRs/MRs on work packages **(inferred)**; YouTrack has VCS integrations with commit commands that change issue fields **(inferred)**.

### Inferences
- Plane's two-tier reference (bracketed = automate, bare = mention) is a cheap, borrowable convention that prevents accidental closes from incidental mentions.
- Keyword auto-close closes on merge to default branch, not on release; none of these tools ties "closed" to "shipped in release X" automatically.

### Gaps
- No verified data on Huly or Vikunja git integrations (Huly had a GitHub sync integration **(inferred)**).

## Release/version/milestone awareness

### Takeaway
Milestones exist almost everywhere, but true release-aware tracking (issue fixed in version X, shipped when tag Y lands) is rare; Plane now has a "releases" resource, YouTrack has version fields, Gitea has milestones + releases but they are not linked.

### Cited Findings
- Plane MCP server covers "projects, work items, cycles, modules, releases, customers and more" — [plane-mcp-server README](https://github.com/makeplane/plane-mcp-server) (fetched via `gh api`).
- Gitea MCP exposes `milestone_read`/`milestone_write` — [gitea-mcp](https://gitea.com/gitea/gitea-mcp).
- OpenProject has versions/roadmap and Gantt; Taiga has sprint burndown and velocity — [opensourcealternatives.to](https://www.opensourcealternatives.to/alternative-to/linear) (aggregator).
- YouTrack uses "Fix versions"/"Affected versions" custom fields **(inferred)**.

### Inferences
- A tracker that derives "released in" from git tags/changelog automatically would be differentiated.

### Gaps
- Plane "releases" semantics (whether tied to git tags) not verified.

## Triage/intake inbox

### Takeaway
Plane has the most developed intake: a dedicated Intake with Triage state, owner, email/form/API sources. Others rely on default states or labels.

### Cited Findings
- Plane (Dec 2025): an intake owner can be set; "regardless of how the request was created (email, form, or external endpoint), intake items always start in Triage"; Feb 2026 added a more detailed Intake view — [Plane changelog](https://plane.so/changelog/2025-12-23-intake-ownership-ai-context-editor-work).
- Plane product page claims Plane AI sorts inbound on arrival (marketing copy, not verified in changelog) — [Plane for product teams](https://plane.so/solutions/product).
- YouTrack "create draft" tool exists in MCP (drafts as a pre-issue stage) — [YouTrack MCP docs](https://www.jetbrains.com/help/youtrack/server/model-context-protocol-server.html).

### Inferences
- Intake-always-starts-in-Triage is a clean invariant to borrow; agent-filed issues could land in a triage queue by default for human acceptance.

### Gaps
- No verified triage inbox info for Vikunja, OpenProject, Gitea/Forgejo (none believed to have one beyond default state/labels).

## Agent interface: MCP, CLI, API, JSON, AI features

### Takeaway
As of Oct 2026: official MCP servers exist for Plane (local + hosted, 30 tools / 207 operations), YouTrack Server (built-in `/mcp`), OpenProject (built-in, experimental, read-only, Enterprise add-on), and Gitea (official `gitea-mcp` v1.9.0). Vikunja and Huly rely on community MCPs. Plane launched native "Plane Agents" (paid only). No tool here ships a first-party agent-oriented CLI with JSON output.

### Cited Findings
- **Plane MCP**: "30 tools, one per Plane resource, covering 207 operations"; stdio/streamable HTTP/SSE; OAuth or API key; hosted at `https://mcp.plane.so/http/mcp`; self-hosted via `PLANE_BASE_URL`; built on FastMCP + `plane-sdk`; MIT, 342 stars, v0.3.4 (2026-10-08) — [plane-mcp-server](https://github.com/makeplane/plane-mcp-server).
- **Plane Agents** (published 2026-09-18): agents assignable to work items; triggered by assignment, mention, work item change, or schedule; configured with instructions, Skills, projects, sources, allowed tools; "escalate to a human when needed"; Pro 100 credits/seat/mo, Business 200 pooled, Enterprise unmetered with own model keys; Plane AI not on Free — [Plane blog](https://plane.so/blog/agents-are-now-live-in-plane). Community/self-hosted availability not stated.
- Plane AI added Autopilot mode and Skills (mobile v2.4.0, 2026-07-31) — [Plane mobile changelog](https://plane.so/changelog/mobile).
- **YouTrack Server MCP**: HTTP endpoint `/mcp`; stdio via `mcp-remote`; OAuth (CIMD, no DCR) or permanent token; runs with user's permissions; admins can hide projects/issues from AI tools; tools: issue search/get/create/create draft/update/assign/comment/tag/link/get field schema/saved searches, articles, projects, groups, users, log work; custom MCP tools via app packages (`customToolPackages`); `tools`/`ignoreTools` query params; `enableToolOutputSchema=true` — [YouTrack MCP docs](https://www.jetbrains.com/help/youtrack/server/model-context-protocol-server.html).
- YouTrack free for up to 10 users with full functionality; AI credits for free-plan teams and Server subscribers, replenished weekly; Server user packs from 15 users ($900 new license) after 2025-10-01 pricing change — [JetBrains blog](https://blog.jetbrains.com/youtrack/2025/06/new-youtrack-prices-starting-from-october-2025/), [JetBrains sales](https://sales.jetbrains.com/hc/en-gb/articles/28382529526162-Changes-to-YouTrack-Server-subscriptions-prices) (via search summary).
- **OpenProject MCP** (since 17.2): experimental, "not yet intended for production usage", read-only (Projects, Work Packages, Users), endpoint `/mcp`, OAuth scope `mcp`, Enterprise add-on from Professional plan; sponsored by Mercedes-AMG — [OpenProject MCP docs](https://www.openproject.org/docs/system-admin-guide/integrations/mcp-server), [17.2 release](https://www.openproject.org/blog/openproject-17-2-0-released/), [heise](https://heise.de/-11209316).
- **Gitea MCP**: official, v1.9.0 (2026-10-16); issues (list/read/write incl. dependencies), PRs (read incl. diff/status/reviews, write incl. merge, review write), milestones; stdio and stateless HTTP; OAuth for remote — [gitea-mcp](https://gitea.com/gitea/gitea-mcp). Forgejo compatibility not stated.
- Community MCPs (via `gh search repos` 2026-10-16): `democratize-technology/vikunja-mcp` (111 stars), `dearlordylord/huly-mcp` (59), `AndyEverything/openproject-mcp-server` (89), `tonyzorin/youtrack-mcp` (99).
- Gitea has `tea` CLI and Forgejo has `forgejo-cli`/`fj` **(inferred)**; none verified for JSON output this session.

### Inferences
- The MCP pattern converging across vendors: server-side `/mcp` endpoint, OAuth + PAT fallback, permission-scoped, admin-hideable projects, tool allowlists to save context. A new tracker should ship this on day one, plus a CLI with `--json`-style output since none of these provide one first-party.
- YouTrack's `enableToolOutputSchema` and "get field schema" tools are directly relevant: agents need the schema to satisfy required/typed fields.

### Gaps
- Whether Plane Agents/Plane AI work on self-hosted Community Edition is unverified.
- Huly official MCP: none found.

## Human UI quality

### Takeaway
Plane is consistently ranked the closest open-source Linear equivalent; OpenProject is heavier/classic-PM; Vikunja is a clean personal todo app; Gitea/Forgejo issues are GitHub-like and basic.

### Cited Findings
- Plane described as "the closest open-source equivalent to Linear"; Community Edition "free with no user limits and no issue caps" — [cut-the-saas](https://cut-the-saas.com/alternatives/linear), [devtoolpicks](https://devtoolpicks.com/blog/best-linear-alternatives-indie-hackers-2026) (aggregators, via search summary).
- OpenProject "better for structured project management" with Gantt, time tracking, Scrum — [opensourcealternatives.to](https://www.opensourcealternatives.to/alternative-to/linear).
- Vikunja views: List, Gantt, Kanban, Table — [Vikunja features](https://vikunja.io/features).

### Gaps
- No hands-on UI evaluation performed; UI claims rest on aggregators.

## License, self-hosting, privacy

### Takeaway
Plane AGPL-3.0 (open core, paid features gated by license key); Huly EPL-2.0 (hosted dead, upstream frozen); Vikunja AGPL-3.0; OpenProject GPL-3.0 (Enterprise add-ons); Gitea MIT; Forgejo GPL-3.0; YouTrack proprietary (free ≤10 users).

### Cited Findings
- License SPDX values from `gh api` 2026-10-16: Plane AGPL-3.0, Huly EPL-2.0, Vikunja AGPL-3.0, OpenProject GPL-3.0, Gitea MIT, plane-mcp-server MIT; Forgejo LICENSE file is GPL v3 (Codeberg raw fetch).
- **Huly hosted shut down**: README says "The hosted Huly service has been discontinued because its hosting is no longer funded" — [hcengineering/platform](https://github.com/hcengineering/platform). Earlier notice cited July 20, 2026 as shutdown date — [dev.co summary](https://dev.co/crm/open-source/platform), [Patreon notice](https://www.patreon.com/posts/142577083).
- YouTrack MCP: admins can hide projects/issues from AI tools — [YouTrack MCP docs](https://www.jetbrains.com/help/youtrack/server/model-context-protocol-server.html).
- Plane Enterprise can use own model keys for agents — [Plane blog](https://plane.so/blog/agents-are-now-live-in-plane).

## Known limitations, maintenance status, Height's fate, newer alternatives

### Takeaway
Height is confirmed dead (shut down 2025-09-24, announced 2025-03-20). Huly is effectively abandoned upstream. Plane, Vikunja, OpenProject, Gitea, Forgejo are all actively released in Oct 2026. Taiga is low-activity.

### Cited Findings
- **Height**: founder Michaël Villar posted "our @height_app journey has come to an end" — [X post](https://x.com/michaelvillar/status/1903820617683501100); AlternativeTo (2025-03-24): "shut down by September 2025" — [AlternativeTo](https://alternativeto.net/news/2025/3/height-project-management-tool-to-shut-down-by-september-2025/); shutdown date 2025-09-24, ~$18.3M raised, Height 2.0 relaunched Oct 2024 as "autonomous project collaboration tool", no public postmortem — [productgrowth.in](https://productgrowth.in/tools/product-management/height/), [therundown.ai](https://www.therundown.ai/tools/height), [creativerly](https://www.creativerly.com/height-is-shutting-down/) (secondary sources; 2025-09-24 date not confirmed on a primary Height page — height.app returned no content via curl).
- Activity snapshot (`gh api`, 2026-10-16): Plane last commit 2026-10-16; Vikunja 2026-10-16; OpenProject 2026-10-16; Gitea 2026-10-16; Forgejo v17.0.0 2026-10-15; Huly upstream frozen (last release 2026-07-05); Taiga backend last push 2026-09-28; Planka v2.2.1 2026-08-10.
- Plane limitations: GitHub integration and AI/agents are paid (Pro+); Plane AI unavailable on Free — [Plane GitHub docs](https://docs.plane.so/integrations/github), [Plane blog](https://plane.so/blog/agents-are-now-live-in-plane).
- OpenProject MCP is read-only and Enterprise-gated — [OpenProject docs](https://www.openproject.org/docs/system-admin-guide/integrations/mcp-server).
- Huly bundled CockroachDB/Redpanda "might not be production-ready" — [huly-selfhost](https://github.com/hcengineering/huly-selfhost).
- Linear has no self-hosted option (sources disagree; aggregator) — [cut-the-saas](https://cut-the-saas.com/alternatives/linear).
- Other notable open-source options surfaced: Taiga, Planka, GitLab — [opensourcealternatives.to](https://www.opensourcealternatives.to/alternative-to/linear).

### Inferences
- Two closures (Height 2025, Huly hosted 2026) both of "AI-native/all-in-one" VC-backed tools reinforce that a new tracker's data should be trivially exportable (plain files / git-backed) so users are never stranded.
- Missing across the whole category: (1) a first-party agent CLI with stable JSON, (2) required-field validation exposed to agents as machine-readable errors, (3) release-aware closure tied to git tags, (4) small-footprint self-host with real typed fields.

### Gaps
- Did not find newer (2026) open-source Linear clones with meaningful traction beyond Plane; searches returned mostly aggregator listicles.
- Forgejo v17 release notes not retrievable (Codeberg page fetch returned unrelated content).
- Common user complaints (Reddit/HN) not gathered within budget.
