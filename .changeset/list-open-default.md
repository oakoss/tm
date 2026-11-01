---
tm-cli: minor
---

`tm list` keeps to the open work when given a filter: `tm list priority:high` no longer lists done and cancelled Issues. A filter naming `status`, `category` or `archived` reaches every Status, and `--all` still lists everything.
