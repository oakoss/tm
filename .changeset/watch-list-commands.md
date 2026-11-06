---
tm-cli: minor
---

`tm list`, `tm ready` and `tm claims` take `--watch`: each prints its result, then again whenever another process writes, until interrupted. Text marks each result after the first with the time it was read; `--json` prints one document per line, and nothing on stderr. A busy store, at the start or later, is tried again at the next poll; any other error ends the watch, and so does the first print after the reader closes the pipe. A Claim lapsing is not a write, so `tm claims --watch` shows it lapsed at the next write.
