---
tm-cli: patch
---

Errors whose `fix` was advice now give a command to run, or no fix. A shared Short-list name gives `tm shortlist --all --json` (or, on import, `tm shortlist edit '<id>' --name '<name>'`), a full Short-list `tm shortlist edit <name> --size '<size>'`, a `TM_SHORTLIST` naming no list the command that ran with `TM_SHORTLIST='<name>'` before it, such as `TM_SHORTLIST='<name>' tm next --peek`, and `tm tui` without a terminal `tm list`. An Issue not on a Short-list gives `tm shortlist add '<list>' '<key>'`. A store this process may not write, a missing Snapshot, a Snapshot from an older tm, and a line tm cannot rebuild with `--set` carry no fix; their message says what to do. The `retry` and `upgrade tm` fixes are unchanged.
