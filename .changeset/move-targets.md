---
tm-cli: minor
---

`tm show` names the Statuses an Issue can move to, in a `moves to:` line after its Status and a `moves_to` list in JSON, each with its category, so finishing work needs no trip through `tm scheme show`. `tm move --help` gives `done` as an example Status and points at `tm show`.
