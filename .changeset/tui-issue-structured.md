---
tm-cli: minor
---

In `tm tui`, an Issue opens laid out rather than as `tm show`'s text: its title, then Status, type, priority, the people and its Claim beside their labels, its description and every comment rendered as Markdown (a `tm ask` question, its answer and an edit marked as `tm show` marks them), and its five latest writes with how many more `tm history` holds. Its parent, children (archived ones marked) and Links sit in a Related pane, beside the text on a wide terminal and below it on a narrow one; Tab and Shift-Tab (or `n` and `N`) move between them and Enter opens one, so you can walk from a story to its epic and back with `q`. Triage's preview beside the queue shows the selected Issue laid out the same way. The screen reads what `tm show --comments all --json` and `tm history --json` report, so it shows what the CLI does.
