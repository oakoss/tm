---
tm-cli: minor
---

`tm import --from beads`, `--from trekker` and the beads import `tm init` offers write each Issue's Status as the first Status of its category in the Issue type's Workflow, so a Scheme that renames Statuses still imports: beads `in_progress` lands in the first active Status, `closed` in the first done one, and Trekker's `wont_fix` in the first cancelled one. A type whose Workflow has no Status of that category is refused as before, naming the Status. `tm init` reads the repo's Scheme file for the new Project, so its import lands where `tm import` would, and a Scheme file it cannot use stops it. Under the built-in Scheme nothing changes.
