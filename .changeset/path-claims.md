---
tm-cli: minor
---

`tm claim`, `tm start` and `tm next` take `--paths GLOB`, the files a Claim covers, from the repository root and repeatable: `tm show`, `tm claims`, `tm history` and JSON show them, with the Claim's branch in JSON. A handoff keeps them, and the Agent it goes to keeps them when it takes the Claim up; the session holding a Claim changes them by claiming it again with other paths.
