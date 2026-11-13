---
tm-cli: minor
---

A mention now notifies whom it names: `tm notifications` lists your unread Notifications, newest first, with the Issue, who wrote it, where and the line that mentions you (`--all` adds the read ones), and `tm notifications read ID`, or `all`, marks them read, which `tm undo` reverts; until then `tm prime` counts the session's unread ones and shows the newest. An edit notifies only for the mentions it adds, an import notifies no one, and an Agent's Notifications reach every session of it writing for the same human. Reading one never changes its Issue. A `tm` older than this release cannot read a Workspace once it holds a read mark, so upgrade every `tm` that shares one.
