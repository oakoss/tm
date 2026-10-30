---
tm-cli: minor
---

The filter language finds blocked work, with GitHub's names: `is:blocked` matches an open Issue an open Issue blocks, `is:blocking` one that blocks an open Issue, and `blocked-by:KEY` and `blocking:KEY` follow an Issue's `blocks` Links whatever their Statuses. `tm list` names the open Issues blocking each Issue it lists, as `[blocked by TM-1]` and a `blocked_by` list in JSON.

Field names in a filter read `-` as `_`, so `blocked-by:` works. A Scheme field can no longer be named `is`, `blocked_by` or `blocking`.
