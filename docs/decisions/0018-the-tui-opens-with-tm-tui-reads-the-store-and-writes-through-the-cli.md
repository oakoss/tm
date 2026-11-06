# The TUI opens with `tm tui`, reads the store directly and writes through the CLI

- Status: accepted
- Date: 2026-11-06
- Deciders: Jace Babin

## Context and Problem Statement

Phase 3 builds the first TUI screens, which show Issues while Agents and hooks write them from other processes. How do the screens open, where does their code live given that writes are assembled in the `tm-cli` binary, how do they notice writes, and how are they tested?

## Decision Drivers

- One binary ([ADR 0002](0002-rust-core-with-typescript-web-ui.md)) whose CLI keeps its cold-start budget.
- One write path: a move from a screen respects version checks, Claims and Scheme rules exactly as `tm move` does.
- A screen open for hours must not hold up writers, and must see their writes without being asked.
- A sandboxed reader that may not open the store reads the Snapshot ([ADR 0001](0001-local-store-is-plain-sqlite-without-a-daemon.md)).
- Screens testable without a terminal.

## Considered Options

- Launch: `tm tui`, or `tm` with no command.
- Layout: screens as modules of `tm-cli`; screens in `tm-tui` behind an actions trait `tm-cli` implements; the CLI's command logic moved into a library both use.
- Refresh: poll `PRAGMA data_version`; watch the files with the `notify` crate; re-read on a timer.

## Decision Outcome

Chosen options: **`tm tui`; screens in `tm-tui` behind an actions trait; polling `data_version` with a new read transaction per refresh; `TestBackend` with `insta` snapshots**, because `tm tui` changes nothing that runs today, the trait keeps one write path without moving the 5,400 lines of the CLI modules the first screens need, `data_version` moves exactly on another process's commit for about 2 µs a poll while a read held across frames grew the WAL from 2,591 to 21,903 KiB in five rounds of writes ([research](../research/tui/architecture-spike.md)), and a fake behind the trait lets screens be tested with no store or terminal.

- **Launch:** `tm tui`. `tm` alone keeps printing help.
- **Layout:** screens live in `tm-tui`, which depends on `tm-store` and `tm-core` and reads through them. Writes go through a trait `tm-tui` defines and `tm-cli` implements with its existing move and edit paths; `tm-cli` opens the Workspace, resolves the person writing and hands both in. Moving the command logic into a shared library waits until `tm serve` needs writes too.
- **Refresh:** one connection polls `data_version` about four times a second for the screen's life, since each connection counts its own; each refresh is a new read transaction on it, and none is held between frames. The screen re-reads when the value moves, and after writes made on that connection, which do not move it. Reading the Snapshot, it stats the file and reopens it when a publish has replaced it. TM-84's `--watch` uses the same primitive. Under `--json` it prints one [ADR 0011](0011-the-cli-speaks-one-json-envelope-and-flat-coded-errors.md) envelope per read, each on its own line, so the one-document rule holds per read; a warning it would print on stderr rides in the next envelope's `warnings`.
- **Testing:** screens render into Ratatui's `TestBackend` and are compared as `insta` snapshots; key handling is tested with synthetic events against a fake actions trait; one `rexpect` test opens `tm tui` and quits.

### Consequences

- Good, because a screen and `tm move` cannot disagree about what a write may do.
- Good, because screens are tested against a fake, with no store or terminal.
- Good, because a long-lived screen never blocks a checkpoint.
- Bad, because the trait grows with each action a screen gains, and `tm serve` will need the same logic through another route.
- Bad, because the binary grows by at least 179 KiB, and `deny.toml` needs a `skip` for build-time `syn` 2, which crossterm's `derive_more` uses.
- Neutral, because a sandboxed screen shows data as of the last Snapshot, as the CLI already does there.

### Confirmation

Revisit the layout when `tm serve` takes writes, since both would then want the command logic in a library. Revisit the refresh rate if TM-142's list at 10,000 Issues cannot redraw within a poll interval.

## More Information

- [Research: how the TUI launches, reads, refreshes and is tested](../research/tui/architecture-spike.md)
- design.md, Views and UIs, and its open item on long-lived sandboxed readers.
