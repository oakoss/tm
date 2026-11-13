---
tm-cli: patch
---

`tm tui` shows the warnings every command prints on stderr, such as logged operations that did not apply in full or the Snapshot it reads from and when that was published, in a row above the footer on every screen. The row is read again with the screen, so it clears once the cause is fixed.
