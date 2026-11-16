---
tm-cli: minor
---

`tm scheme show` lists the Scheme the working directory's Project uses, its Workspace and repo Scheme files included, and marks each field a file defines with where it comes from: `[from the Workspace's Scheme]` or `[from the repo's Scheme]` in text, and `from` (`built_in`, `workspace` or `repo`) in `--json`. A select field lists its options. Outside any repo file it shows the built-in Scheme with a warning saying so. A Scheme file that cannot be used stops it with `invalid_scheme`, and a repo file naming a Workspace this machine lacks stops it with `unknown_workspace`, as other commands do.
