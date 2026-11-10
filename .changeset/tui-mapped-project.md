---
tm-cli: minor
---

The filter language takes `project:`, a Project's key (current or retired, without case) or its id; a key or id no Project has fails with `not_found`. `tm tui`'s list, Triage and Board read only the Project the directory maps to, and a list filter naming `project:` reads that instead.
