# Structure

A virtual root manifest with every Rust package under `crates/`:

| Crate | Holds | Published |
|---|---|---|
| `tm-core` | Domain model: Issues, Schemes, Workflows, the filter language. No I/O. | no |
| `tm-store` | SQLite store, operation log, projections, read-only Snapshot | no |
| `tm-sync` | Operation exchange with `tm serve` | no |
| `tm-server` | `tm serve`: JSON API and change stream | no |
| `tm-tui` | Ratatui screens | no |
| `tm-cli` | The `tm` binary, wiring the others together | yes |

Internal crates carry `publish = false`. Add a crate only when a boundary already exists in the design; splitting for organization alone is what modules are for.

The web UI will live in `web/` (pnpm) and the npm wrapper in `npm/`; neither exists yet.

## Integration tests

`crates/tm-cli/tests/it/main.rs` is the one integration-test binary; each suite is a module declared there. Add a suite as a module, not as a new `tests/*.rs` file: Cargo builds every top-level file as its own binary, and each one relinks on every library change. Filter with `cargo test -p tm-cli --test it <module>::`.
