---
tm-cli: minor
---

A Scheme file's `[types.ID]` can list the types an Issue's parent may have, as `parents = ["epic"]`; `parents = []` takes no parent, and a type without the list takes a parent of any type. `tm create --parent`, `tm edit --parent` and a type change through `tm edit --set type=` are refused with `parent_type` when they would leave a child under a parent its type does not list, naming the child, the parent and the allowed types; `tm create` and `tm edit` refuse before reading piped text. A parent and child already in place when the Scheme began refusing them stay editable, and `tm undo` and imports are not held to the list. `tm scheme show` names a type's parent types, and `tm scheme show --json` gives them as `"any"` or the list. A Scheme file is refused at its line when a type lists a parent type twice or one no layer defines, or when the file removes a type another lists as a parent.
