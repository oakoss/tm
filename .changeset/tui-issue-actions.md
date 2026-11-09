---
tm-cli: minor
---

In `tm tui`, an opened Issue takes the keys Triage does: `s` moves it to one of the Statuses `tm show` lists under "moves to", `p` sets its priority, `A` assigns it (`@me` first, or a name typed), and `u` undoes this session's last write. Each write is the one `tm move`, `tm edit` or `tm undo` makes, refused if the Issue changed since it was read. `c` opens `$VISUAL`, else `$EDITOR`, else `vi`, on an empty file and adds what you save as a comment, as `tm comment` does; saving nothing writes nothing. Its Claim reads as `tm show` words it: one with no expiry, one handed on and not yet taken, and one that lapsed, which any Agent may take unless the Issue is archived.
