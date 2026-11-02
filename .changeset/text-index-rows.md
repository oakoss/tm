---
tm-cli: patch
---

Editing a title or description updates its search index entry by lookup instead of scanning the index: a title edit at 100,000 Issues went from 73 to 45 ms, and writing 100,000 Issues in one batch from 1,408 to 79 s. Opening a store rebuilds its projection once.
