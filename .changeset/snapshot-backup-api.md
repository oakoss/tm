---
tm-cli: patch
---

Every write publishes the Snapshot with SQLite's backup API instead of `VACUUM INTO`, about 30% faster (111 to 77 ms at 10,000 Issues on macOS). The Snapshot keeps its rollback-journal format, so a sandboxed reader can still open it read-only.
