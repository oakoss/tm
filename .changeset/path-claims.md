---
tm-cli: minor
---

Path Claims: `tm claim`, `tm start` and `tm next` take `--paths GLOB`, the files a Claim covers, from the repository root and repeatable, and any other session or sub-agent is warned before editing one. In Claude Code, `tm setup claude` adds a hook before Edit, MultiEdit, Write and NotebookEdit that tells the Agent, once per Claim and file, which Issue and session hold the file, by a path the Claim names or as already committed on its branch. It never blocks the edit, and when git cannot say, it tells that once, and again if it comes back. Run `tm setup claude` again to add the hook to an existing setup. `tm prime` lists other sessions' Path Claims on the files the checkout changes, as `path_claims` in JSON.

`tm show`, `tm claims`, `tm history` and JSON show a Claim's paths, with its branch in JSON. A handoff keeps them, and the Agent it goes to keeps them when it takes the Claim up; the session holding a Claim changes them by claiming it again with other paths.
