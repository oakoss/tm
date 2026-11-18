---
tm-cli: minor
---

A Scheme file's fields can take three more types: `version`, a semantic version such as `1.2.0` or `2.0.0-rc.1`; `url`, an absolute http or https URL; and `file_paths`, paths relative to the repo root written with `/` and no `..`, held as a set. A value of another shape is refused with `invalid_value`. `tm create --add FIELD=VALUE`, and `tm edit --add` or `--remove FIELD=VALUE`, change a field that holds several values, `file_paths` and `multi_select` alike; `--set` on one of them, or `--add` on a field that holds one value, is refused. Filters match the new types exactly as written, such as `paths:src/main.rs` or `release:1.2.0`: versions do not compare or sort, and paths do not match by pattern.
