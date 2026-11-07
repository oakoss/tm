---
tm-cli: minor
---

In `tm tui`, `b` opens the Board: a column per Workflow Status with its count, done and cancelled shown only after `a`. `h` and `l` change column, `j` and `k` move within one, and columns that do not fit scroll into view, the edge columns saying how many are hidden. `H` and `L` move the Issue to the Status beside its own, through `tm move` as you, and the cursor follows it; `s` picks any Status its Workflow has. A write is refused like the CLI's when someone changed the Issue since it was read, and `u` undoes this session's writes. Enter opens the Issue.
