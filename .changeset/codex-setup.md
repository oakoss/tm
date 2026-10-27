---
tm-cli: minor
---

### Added

`tm setup codex` writes Codex's hooks to `.codex/hooks.json`. They prime each session, renew Claims after every tool use, and release the session's Claims when it ends. Codex runs them once the project is trusted and each hook is approved in `/hooks`; `tm setup codex --undo` removes them.
