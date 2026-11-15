---
tm-cli: patch
---

With `GIT_TRACE` set, an error that quotes git, such as the one saying no one can be named, no longer carries git's trace lines. Every git process tm starts now runs the same way, without git's optional locks and without its trace variables.
