---
tm-cli: minor
---

In `tm tui`, `t` opens Triage: the Issues in a triage Status, the selected one shown beside the queue on a wide terminal and below it on a narrow one, each moved, prioritized or assigned with one key. `s`, `p` and `A` open a picker you can type into to narrow, with you first when assigning and a name no one holds yet taken as typed. Space marks Issues so the next action takes them all. Each write goes through `tm move` or `tm edit` as you, refused like the CLI's when someone changed the Issue since it was read. A line at the bottom says what happened, with the CLI's warnings, until the next key; the cursor goes on to the next Issue, and an Issue whose write was refused stays marked. `u` undoes this session's writes, newest first, as `tm undo` does, and keeps a refused undo for another try. The top line names the screens open, and the `?` keys list the open screen's own keys.
