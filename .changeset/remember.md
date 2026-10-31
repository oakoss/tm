---
tm-cli: minor
---

`tm remember "fact"` keeps a durable fact about the Project, such as how the tests must run, and every session's `tm prime` shows the 10 most recently written with who wrote each and how long ago. Its key comes from its first words unless `--key` names one; an unnamed memory never replaces another, while `--key` replaces that memory's text and says whose it was. A memory that reads much like another names it, and one over 500 characters is refused. `tm memories` lists them, searches them by words or shows one by its key, and `tm forget KEY` drops one, which `tm undo` brings back. Memories go out with `tm export` and come in with a record import. The store rebuilds its projection once on first open.
