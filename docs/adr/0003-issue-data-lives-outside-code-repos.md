# Issue data lives outside code repos

Issues, history, attachments and the store itself live in the Workspace directory (by default under `~/.tm/`), never inside a code repo; a repo holds at most one config file (`.config/tm.toml`, or `.tm.toml` at the root) that maps it to a Workspace and Project and may carry that Project's Components, Scheme overrides and rules. This keeps repos clean, means no code push can publish task data, and lets one Workspace serve many repos. It differs on purpose from beads, which keeps its database under `.beads/` inside the repo (gitignored) alongside committed config, hooks and logs, and can sync through a hidden ref on the code remote.

## Consequences

- Issue changes cannot be reviewed in a pull request alongside the code, which file-per-issue trackers such as Backlog.md allow; this is given up deliberately for clean repos and leak-free pushes.
- A fresh clone on another machine gets its Issues through `tm serve` sync, not from the git remote.
- Repo-scoped use still works: `tm init` in a repo with no Workspace creates one dedicated to that repo, whose config sits in the repo's `.config/`.
