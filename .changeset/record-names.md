---
tm-cli: minor
---

A person's write now records their name from git's `user.name` beside their email, once and again only when it changes in that Replica, as a record of its own (ADR 0019); `tm undo` still undoes their write, not the name. Mentions show it. Under `TM_ACTOR`, or as an Agent, nothing is recorded, since git's name is then someone else's.
