---
tm-cli: minor
---

An Agent that reaches a decision only its human can make runs `tm ask KEY "question"` and takes other work: the question joins the Issue's comments, `waiting_on` names its human, the Issue leaves the ready queue, and the Agent's Claim is kept for it. `tm prime` lists what waits on the session's human, telling an Agent's session to bring the questions to them, and `tm answer KEY "reply"` records the reply; the asking Agent then resumes with `tm start`. A person asks someone with `--of`. Finishing an Issue clears its wait. Issue records in an export carry no wait, and a record import refuses `waiting_on`. The store rebuilds its projection once on first open.
