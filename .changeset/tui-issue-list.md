---
tm-cli: minor
---

`tm tui` lists the Project's open Issues, most recently updated first, as `tm list` does: Key, status, priority and title, with the count in the header, read again whenever another process writes, a sandboxed Snapshot reopened when a publish replaces it. `/` types a filter in `tm list`'s language (Enter applies it, Esc drops it, a bad one says why, `@me` as `tm list` reads it), `a` switches between open work and every status, and `j`/`k`, the arrows, `g`/`G`, Home/End and the page keys move the selection, which stays on its Issue across a read. Stored text is shown escaped, as the CLI prints it.
