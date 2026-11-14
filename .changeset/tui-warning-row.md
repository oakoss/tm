---
tm-cli: patch
---

`tm tui` shows a write's or undo's warnings on the warning row, ahead of the store's, instead of after the result: a long refusal on a narrow terminal, or a reason over two lines, no longer cuts the warning off or hides it. A write no longer repeats the store's warnings, which the row reads afresh, so a count such as the logged operations that did not apply is never out of date.
