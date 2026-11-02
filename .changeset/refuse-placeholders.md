---
tm-cli: patch
---

A value written as one word in angle brackets, such as `<value>`, is refused as a placeholder to replace, in `--set`, `--append`, `--title`, `--label`, `TM_AGENT`, `TM_ACTOR` and `TM_SESSION`. tm's fixes now quote their placeholders, so a fix pasted unchanged fails with that error instead of the shell reading `<value>` as a redirect that could read or overwrite a file.
