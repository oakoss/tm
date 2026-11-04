---
tm-cli: minor
---

`tm import --from trekker .trekker/trekker.db` moves a Trekker project into tm, reading its database read-only since Trekker has no export. Epics, tasks and sub-tasks keep their parents, Statuses, priorities, tags as labels (a `bug` or `spike` tag also sets the type), comments and dependencies as `blocks` Links; `rank` and the audit log are counted as not kept.
