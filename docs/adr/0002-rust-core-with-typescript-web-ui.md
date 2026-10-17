# Rust core with a TypeScript web UI

The CLI, TUI (Ratatui) and `tm serve` are one Rust binary; the web UI is TypeScript with React and TanStack, built into static files embedded in that binary and talking to `tm serve` over JSON and a server-sent change stream that feeds a TanStack DB custom collection. Rust gives a single static binary, a bundled SQLite build of a known version (Bun on macOS uses Apple's system SQLite, which left `-wal`/`-shm` files behind in our tests), and a mature TUI framework. TypeScript types for the web UI are generated from the Rust types so the schema has one source.

## Considered Options

- **TypeScript throughout (Bun)**: one language and direct reuse of the UI component library, but a weaker TUI ecosystem and no control over the SQLite version on macOS.
- **Go**: mature TUI (Bubble Tea) and fast builds, but not a language used in the author's other projects.
