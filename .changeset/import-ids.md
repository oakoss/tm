---
tm-cli: minor
---

Ids from an import's source stay lookup aliases, so old commits and notes keep resolving: `tm show TREK-114` (or a beads id) finds the Issue imported from it, wherever a command takes an Issue, and `tm show CMT-243` shows the Issue holding that comment with the comment in view. `tm comment KEY --edit CMT-243` edits it, and `tm show` prints an imported comment's source id, such as `[CMT-243]`, with `source` in JSON.
