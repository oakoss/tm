---
tm-cli: minor
---

`@mentions` resolve when text is saved: comments and their edits, descriptions, questions, answers, withdrawals and handoff notes store each `@word` that names a known Actor (a person's email, its local part or a word of their name, or an Agent's name) as a token, and `tm show`, `tm history`, `tm prime` and `tm tui` print it as `@` and the Actor's name. When a word names several, a person at a terminal picks one; an Agent, a pipe and `--json` are refused with each candidate as it can be written. `tm show --json` keeps the stored text and lists the mentions apart.
