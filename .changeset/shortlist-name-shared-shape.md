---
tm-cli: patch
---

### Fixed

A `shortlist_name_shared` error carries `kind`, `name` and `count` whichever command raised it, and `tm shortlist` commands give the same fix as a filter: name the list by its id from `tm shortlist --all --json`. `tm import` refused this way says to rename one of the lists with `tm shortlist edit`.
