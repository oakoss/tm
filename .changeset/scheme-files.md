---
tm-cli: minor
---

The Scheme is read from config. A Workspace's `scheme.toml`, in its directory under `~/.tm/workspaces/`, can add custom fields for every Project in it, and a repo's `.config/tm/scheme.toml` can add, replace or remove them for the Projects its repo file maps, under `[projects.KEY]`. A field takes a `type` (text, rich_text, number, date, datetime, boolean, single_select, multi_select or person), an optional `label` and, for selects, `options`; `remove.fields` drops one a layer below defined. Built-in fields cannot be replaced or removed. A Scheme file that cannot be used stops every command with the code `invalid_scheme`, naming its file and line, while `tm doctor` still runs and reports it. With no Scheme file, nothing changes.
