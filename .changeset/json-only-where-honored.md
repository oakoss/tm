---
tm-cli: patch
---

`--json` is offered only where a command honors it. `tm tui` and `tm hook` no longer list it in their help; `tm tui --json` is a usage error (exit 2) with fix `tm list --json`, and a `tm hook` whose arguments fail to parse prints clap's error as text on stderr, where the harness looks, even under `--json` or `TM_JSON=1`. A JSON usage error names what clap lists, such as the missing arguments, and carries clap's hint under `tip`. `tm next --peek` refuses `--paths`, as it refuses `--for` and `--no-expiry`, and `tm --version --json` answers under `data.version`, not `data.help`. The edit hook says when its input has no `tool_input`, rather than listing nothing.
