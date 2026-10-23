# tm

A local-first issue tracker for one human and their AI agents. Jira-style planning for the human, a complete `--json` CLI for agents, and awareness of the code itself: the files an Issue touches, its branches and pull requests, the release it ships in, and review findings anchored to lines of code.

**Status:** Phase 1 of the build plan works: the store, operation log and core CLI. This repo tracks its own work in `tm`. Nothing is published yet; build it with `cargo install --locked --path crates/tm-cli`.

- [Design](docs/specs/design.md): what `tm` does
- [Build plan](docs/contributing/build-plan.md): the order it is built in
- [Decisions](docs/decisions/): the hardest-to-reverse choices and why
- [Research](docs/research/): the trackers and storage engines examined on the way

Licensed under [MIT](LICENSE).
