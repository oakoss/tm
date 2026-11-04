---
tm-cli: patch
---

`tm --help` lists commands under headings by use (Everyday, History and undo, Agent work, Memories, Setup and data), each in one short line, so a person can scan it; the detail moved into each command's own `--help`, which says no less than before. `tm hook`, run only by the hooks `tm setup` writes, is left out of the list, and every argument now has a description.
