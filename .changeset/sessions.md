---
tm-cli: minor
---

Every operation an Agent writes now records its session, and a Claim the git branch it was taken on. `tm show KEY` lists the sessions that worked the Issue, most recently active first, each with its branch, what it did and the command that resumes it (`cd DIR && claude -r ID` or `cd DIR && codex resume ID`), or that it ran on another machine; the directory comes from this machine only and is never exported. `tm list session:ID` finds the Issues a session wrote, taking a full id or its end as `tm history` shows it. `tm history` names each write's session; `tm prime` names the session's own id and gives the resume command under a question an Agent asked. A store from before gains the session column and rebuilds its projection once, when first opened.
