---
tm-cli: minor
---

A Scheme file's `[types.ID]` can list the Scheme fields the type carries, as `fields = ["severity"]`. A type with no list carries every field, and every type carries the built-in fields and `waiting_on`. `tm create`, `tm edit` and `tm move` refuse to set a field the Issue's type does not carry, and refuse a type change that would leave the Issue holding such a value unless the same command clears it. A value the Issue already holds for a field its type stopped carrying still reads back and can be cleared. `tm scheme show TYPE` lists only the fields the type carries, and `tm scheme show` prints each type's list. A Scheme file is refused at its line when a type lists a field twice, a field every type carries, or one no layer defines, or when it removes a field a type still lists.
