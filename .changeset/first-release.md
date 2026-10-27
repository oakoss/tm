---
tm-cli: minor
---

### Added

First release of tm: a local-first issue tracker for one human and their AI agents, with a complete `--json` CLI. Install it with the shell installer attached to this release or `brew install oakoss/tap/tm`.

Until v1 the store's log format can change between releases without a version bump, so a tm of another version can misread a store rather than refuse it. Take a copy with `tm export` before upgrading.
