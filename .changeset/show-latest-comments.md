---
tm-cli: minor
---

`tm show` prints an Issue's latest 5 comments, from the question it waits on if that is older, and counts the earlier ones, so checking an Issue no longer reads its whole discussion; `--comments N` or `--comments all` shows more. JSON gives the count as `earlier_comments`, and commands that answer with the Issue, such as `tm start` and `tm move`, print it the same way.
