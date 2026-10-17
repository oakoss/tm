# Operation log is the source of truth

Every change is appended to an operation log, and the Issue tables are projections of it, updated in the same SQLite transaction and regenerable at any time with `tm rebuild`. Undo, history and sync then share one mechanism, and a damaged or mis-migrated table is recoverable rather than fatal, following Fossil's ticket design; storing tables as the truth with a separate history would recreate the dual-storage drift that beads' JSONL export suffered.

## Consequences

- Every write costs an extra append; at tracker scale this is assumed negligible but has not been measured.
- Schema changes to the tables are a rebuild, not a data migration; changes to the operation format need versioned operations.
- The log needs compaction once sync exists; compaction must never drop content, only collapse superseded operations.
