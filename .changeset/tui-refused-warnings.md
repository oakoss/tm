---
tm-cli: patch
---

`tm tui` shows the warnings a refused write or undo carries, as it does for one that lands, such as logged operations that did not apply, or a Snapshot not published when the write landed before a later step failed; before, the status line said only why it was refused.
