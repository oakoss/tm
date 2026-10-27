---
tm-cli: minor
---

### Added

`tm setup claude` names each Claude Code sub-agent in its tm commands, as `<agent_type>-<first 8 of agent_id>`, so parallel sub-agents' Claims and history are told apart. A sub-agent's Claims are released when it finishes, and the session's end releases its sub-agents' Claims with its own. Run `tm setup claude` again to add the new hooks; it rewrites the old session-end hook in place.
