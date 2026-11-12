---
tm-cli: minor
---

A person's write now records their name from git's `user.name` beside their email, once and again only when it changes, as a record of its own (ADR 0019); `tm undo` still undoes their write, not the name. Nothing prints the name yet; mentions will. Under `TM_ACTOR`, or as an Agent, nothing is recorded, since git's name is then someone else's. A `tm` older than this release cannot read a Workspace once it holds a name, so upgrade every `tm` that shares one.
