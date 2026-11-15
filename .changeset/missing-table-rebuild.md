---
tm-cli: patch
---

A store missing one of the tables `tm rebuild` makes again from the log is refused with the code `missing_table`, naming the table, and the fix `tm rebuild`, where it used to stop on a bare SQLite error. `tm rebuild` now runs on a store missing its table of logged operations that did not apply, which every other command reads first and so could not repair.
