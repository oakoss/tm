---
tm-cli: minor
---

`tm ask KEY "question" --option A --option B` offers two or more likely replies, worded rather than numbered, shown numbered in `tm show` and `tm prime` and listed in their JSON. `tm answer KEY 2` or `tm answer KEY b` picks one, reported as `choice` in `tm answer`, `tm show` and the asking Agent's `tm prime`; a number that picks no option is refused, and any other reply is taken as written. At a person's terminal, `tm answer KEY` asks for the reply and a bare `tm answer` walks each question waiting on you, a blank line passing one over; an Agent, anyone not at a terminal, and `--json` are refused rather than left waiting. `tm withdraw KEY "why"` lets the asker settle its own question: the Issue waits on no one, the Claim stays kept for the asking Agent, and its next `tm prime` says the question was withdrawn.
