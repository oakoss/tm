---
tm-cli: patch
---

### Fixed

`tm setup`'s refusal to an Agent, and `tm init`'s without `--yes`, tell a person to run it from their own terminal, since a command run in the Agent's session, Claude Code's `!` included, counts as the Agent.
