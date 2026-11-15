---
tm-cli: patch
---

`tm start` and `tm next` suggest a branch name sooner: the two git reads that tell whether this repository's branches are named `word/…` now run together. Measured on macOS in a repo with an `origin` remote, `tm start` went from 66 ms to 59 ms.
