---
tm-cli: patch
---

`tm git scan` no longer closes a new Issue because an old commit names an imported tracker's id that spells its Key, such as beads' `cpl-482` for `CPL-482`. A word that spells a Key but is exactly the id one Issue was imported from names that Issue, and a Closes line in a commit written before the Project or the Issue it would name is skipped without a warning.
