---
tm-cli: patch
---

`tm prime` asks git what this checkout changes faster when another session holds a Path Claim: git's status now runs alongside the default-branch lookup, and without an `origin/HEAD` that lookup takes one git process instead of two. Measured on macOS in a repo without a remote, that `tm prime` went from 58 ms to 45 ms.
