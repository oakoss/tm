# How the TUI launches, reads, refreshes and is tested

- Date: 2026-11-06
- Author: Claude, for Jace Babin (TM-140)
- Scope: what the first TUI screens (Phase 3) rest on: the command that opens them, where their code lives, how they notice writes, and what Ratatui costs the binary.

## Question

The screens show Issues that other processes write while they are open: Agents, hooks, a person's own `tm` in another terminal. How should they notice those writes without holding the store in a way that hurts writers, how should they reach the code that reads and writes, and what does adding Ratatui cost a binary whose CLI has a cold-start budget?

## Sources

- SQLite's documentation of [`PRAGMA data_version`](https://sqlite.org/pragma.html#pragma_data_version) and [`wal_checkpoint`](https://sqlite.org/pragma.html#pragma_wal_checkpoint).
- Ratatui 0.30's API documentation ([`ratatui::run`](https://docs.rs/ratatui/latest/ratatui/fn.run.html), [`TestBackend`](https://docs.rs/ratatui/latest/ratatui/backend/struct.TestBackend.html)), read through Context7.
- [`crates/tm-store/src/snapshot.rs`](../../../crates/tm-store/src/snapshot.rs): `publish_snapshot` renames a fresh copy over the Snapshot, then checkpoints `PASSIVE`; `open_snapshot` opens it `immutable=1`.
- [`crates/tm-cli/src/moves.rs`](../../../crates/tm-cli/src/moves.rs) and [`edit.rs`](../../../crates/tm-cli/src/edit.rs): how a move or edit builds its operations.
- Measurements from [scripts](scripts), on a copy of this repository's store (323 Issues, 3,153 operations), macOS on an Apple M2 Max, SQLite 3.54.1 through Python and 3.54.0 for the second process.

## Findings

### `data_version` sees other processes' commits, cheaply

[`scripts/data-version.py`](scripts/data-version.py) printed:

```text
journal: wal
own write moves it: False
other process write moves it: True 2 -> 3
no write, unchanged: True
a new connection starts at: 2
poll cost: 1.84 us/call
```

- A commit by another connection moves it; nothing else does, so polling it says exactly when to re-read.
- Its value belongs to the connection: a new connection starts low, so comparing a new connection's value with an old one's misses writes. One connection polls for the screen's life, and each refresh is a new read transaction on it.
- Writes on the polling connection do not move it, so a screen whose writes go through that connection re-reads after them.
- A Snapshot opened `immutable=1` is never written, so its `data_version` never moves; a publish replaces the file by rename, which a stat of the path sees.

### A reader holding a transaction grows the WAL without bound

[`scripts/wal-growth.py`](scripts/wal-growth.py) wrote five rounds of 200 rows with a `PASSIVE` checkpoint after each, as a write's publish does:

```text
reader holds a transaction=True: WAL KiB after each round [2591, 7419, 12247, 17075, 21903]
reader holds a transaction=False: WAL KiB after each round [2591, 4031, 4031, 4031, 4031]
```

A checkpoint cannot pass the oldest open read, so a screen that keeps one read transaction open between frames makes every write's log grow until the screen closes: 2,591 to 21,903 KiB here. A new read transaction per refresh keeps it at 4,031 KiB.

### Writes are built in the CLI

A move or edit is assembled in `tm-cli`, not in `tm-store`: `moves.rs` and `edit.rs` check the version a write is based on, the Claims it must respect, the Scheme's field rules and the parent a move starts, then append the operations. Reads need nothing from the CLI: `tm-store` returns `IssueView`s and `tm-core` parses the filter language. `tm-cli` is a binary that will launch the screens, so `tm-tui` cannot depend on it; Cargo refuses the cycle.

### What Ratatui costs

[`scripts/ratatui-probe.sh`](scripts/ratatui-probe.sh) builds HEAD with `ratatui = "0.30.2"` linked into one bordered frame.

- **Size:** the release binary went from 5,642,032 to 5,824,976 bytes (+179 KiB, 3.2%). Real screens link more widgets, so this is a floor.
- **Cold start:** `hyperfine -N --warmup 10 -m 150` on this machine, which was busy, gave `tm list` 10.8 ± 5.7 ms without and 11.4 ± 5.7 ms with, and `tm prime` 43.8 ± 12.6 against 44.4 ± 11.6 ms: no difference beyond the noise. CI's cold-start job is the gate.
- **cargo-deny:** advisories, licenses and sources pass; bans fail on two versions of `syn`, since crossterm's `derive_more` uses `syn` 2 at build time while the workspace uses 3. `cargo tree -p tm-cli -e normal --prefix none | sed 's/ (\*)//;s/ (proc-macro)//' | sort -u | wc -l` counts 58 crates at HEAD and 111 with Ratatui.

### Widget crates beyond Ratatui's own

Ratatui ships `List`, `Table`, `Tabs`, `Scrollbar` and `Paragraph`; the first screens are lists, tables and a scrolling detail. What it lacks, from `cargo info` and crates.io's dependency lists on 2026-11-06; `tui-input` depends on `ratatui` ^0.30.2, and `ratatui-textarea`, `tui-tree-widget`, `tui-scrollview` and `tui-popup` on Ratatui 0.30's `ratatui-core` ^0.1 and `ratatui-widgets` ^0.3:

| Crate | Licence | Gives | Fits |
|---|---|---|---|
| `ratatui-textarea` 0.9.4 | MIT | A multi-line editor, from the ratatui organisation | Editing a description or comment, completing an `@mention` |
| `tui-input` 0.15.5 | MIT | A single-line input | A filter bar |
| `tui-widgets` 0.7.12 | MIT or Apache-2.0 | A collection from the ratatui organisation, `tui-scrollview` 0.6.8 and `tui-popup` 0.7.7 among it | A long history, help and confirmation popups |
| `tui-tree-widget` 0.24.1 | MIT | A collapsible tree | Parents and children |
| `nucleo` 0.5.0 | MPL-2.0, which `deny.toml` does not allow | Fuzzy matching | Phase 7's search palette |

`tuirealm` 4.1.0 (components after React and Elm) and `rat-salsa` 4.0.3 with `rat-widget` 3.2.1 (an event loop and widget set) are frameworks: each brings its own structure for a screen's state and events, which the first screens, built from the widgets above, do not need.

## Conclusions

- Polling `data_version` on one long-lived connection, with a new read transaction per refresh, sees every other process's write within a poll and keeps the WAL bounded: 4,031 KiB, against 21,903 with a read held across writes.
- The screens can read through `tm-store` and `tm-core` alone; their writes need the CLI's move and edit paths, which `tm-tui` cannot depend on directly.
- Ratatui costs at least 179 KiB and 53 crates, and no cold start this machine could measure.

## Implications / actions

- [ADR 0018](../../decisions/0018-the-tui-opens-with-tm-tui-reads-the-store-and-writes-through-the-cli.md) records the choices; TM-84 builds the polling primitive for `--watch` and the screens.
- `deny.toml` gains a `skip` for build-time `syn` 2 when Ratatui lands (TM-141).
- The first screens use Ratatui's built-in widgets only. `ratatui-textarea` joins with the first screen that edits a description or comment or completes an `@mention` (TM-82), and `tui-widgets` if the detail screen's scrolling needs it; each is checked for size, `cargo-deny` and its Ratatui version when added. No framework.

## Open questions

- What the first screen costs to draw at 10,000 Issues is measured when it exists (TM-142), against the same fixture as the cold-start test.
- Mouse support, and whether `tm` alone should one day open the TUI, wait for the screens.
