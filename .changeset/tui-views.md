---
tm-cli: minor
---

Views: saved filters and layouts kept in the repo file as `[[views]]` tables, each with a `name`, a `filter`, a `layout` of `list` or `board`, an optional `project`, and for a Board optional `[[views.columns]]` (a `status` and its own `filter`) and `[views.swimlanes]` (a `field` and its own `filter`). In `tm tui`, `v` lists the Project's Views and Enter opens one; a board View shows its own columns and bands its cards into swimlanes by the field's value. `e` edits a View in `$EDITOR`, `n` adds one and `d` removes one; each is shown as a diff of the repo file and written only on `y`, with the rest of the file kept as written. An Agent or a read-only Snapshot sees the Views but cannot change them.
