---
tm-cli: patch
---

### Fixed

A field given to `tm edit`, `tm create` or `tm move` as its own flag, such as `tm edit TM-1 --priority low`, is refused with the command to run instead: `tm edit TM-1 --set priority=low`, or for `--status` beside only flags `tm move` takes, the `tm move` that changes it. clap's tip there pointed at `-- --priority`, which does not work.

`tm edit` given a field more than once, the Status included, says so; for the Status it used to point at a `tm move` that kept only the first value.
