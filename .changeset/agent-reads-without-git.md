---
tm-cli: patch
---

The edit hook and `tm ready` no longer run git to read `user.email` when an Agent runs them, since they tell Agents apart by name and session; each was measured about twice as fast.
