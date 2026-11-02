---
tm-cli: minor
tm-store: minor
tm-core: minor
---

`tm comment KEY --edit ID "<text>"` corrects a comment you wrote, or the one an Agent wrote for you; `--edit last` corrects your own latest on the Issue. `tm show` prints each comment's id and marks an edited one, JSON gives `edited_utc_ms`, search finds the new words, `tm history` keeps the earlier text, and `tm undo` restores it. Questions, answers and withdrawals are not edited, since their text decides what was chosen. The store rebuilds once on first open to index edits.
