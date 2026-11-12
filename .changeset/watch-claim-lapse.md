---
tm-cli: patch
---

`--watch` and `tm tui` read again when a held Claim lapses. A lapse writes nothing, so `tm claims --watch` showed a lapsed Claim as held until some other write.
