---
tm-cli: patch
---

`tm tui` keeps what it used to drop: a failure to start prints the warnings read before it, such as logged operations that did not apply; a screen that cannot be read says the command that fixes it when the CLI gives one; and an unread count that cannot be read is a warning rather than no count.
