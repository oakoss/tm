---
tm-cli: minor
---

`tm tui` opens the terminal screens over this directory's Project, showing its name, the keys (`q` or Ctrl-C to quit, `?` to show them, Esc to hide them) and when another process last wrote. The terminal is restored when it quits, fails, panics, or is sent SIGTERM or SIGINT, which end it with 128 plus the signal's number; a second such signal ends it at once. Without a terminal it refuses with `not_a_terminal`.
