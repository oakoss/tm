---
tm-cli: minor
---

A Scheme file can add, replace or remove Issue types and Workflows, by id as it does fields. `[workflows.ID]` takes a `start` and its `statuses`, each an `id`, an optional `label` and a `category` (triage, todo, active, done or cancelled); `[types.ID]` takes an optional `label` and its `workflow`; `remove.types` and `remove.workflows` drop inherited ones, the built-in `default` Workflow included. An Issue whose type or Status the Scheme no longer defines reads as Triage and stays editable. A Scheme file is refused at its line when a type names a Workflow that does not exist, a Workflow starts in a Status it lacks, has no Statuses or lists one twice, a Workflow a type uses is removed, or no type is left. `tm scheme show` marks types and Workflows a file defines, and `--json` gives each a `from`.
