---
tm-cli: minor
---

`tm setup claude` writes tm's hooks to `.claude/settings.local.json` by default, your own settings kept out of commits, so contributors without tm are not given hooks that fail. At a terminal it asks which file, local first; `--shared` writes `.claude/settings.json` for everyone who clones the repo. Hooks already in a file stay there: a rerun updates them in place and `--undo` removes them from each file that holds them, so nothing moves for an existing install. Each Claude Code hook now starts with `command -v tm >/dev/null || exit 0`, so a clone without tm runs it silently, and the Bash hook gets a 10-second timeout. Where git does not ignore the local file, setup says so.
