---
tm-cli: minor
---

`tm edit KEY --append description=-` adds a paragraph to the end of a long text field after one blank line, so growing an Issue's plan no longer means rewriting it whole. It takes stdin like `--set`, refuses a field that is not long text, and is checked against the version it read like any other edit.
