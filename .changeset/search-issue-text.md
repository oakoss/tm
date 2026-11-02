---
tm-cli: minor
---

`tm list` searches Issue text: a bare word or a quoted phrase matches Issues whose title, description or comments not retracted hold it, matching English word stems and ignoring case and accents (`tm list login timeouts`), and `text:` writes the same term explicitly (`-text:word` excludes it). The `bare_word` filter error is gone, since a bare word is now a search. Opening a store rebuilds its projection once to make the full-text index.
