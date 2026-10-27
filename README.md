# tm

A local-first issue tracker for one human and their AI agents. Jira-style planning for the human, a complete `--json` CLI for agents, and awareness of the code itself: the files an Issue touches, its branches and pull requests, the release it ships in, and review findings anchored to lines of code.

**Status:** Phase 1 of the build plan works: the store, operation log and core CLI. This repo tracks its own work in `tm`.

## Install

On macOS or Linux, with Homebrew:

```sh
brew install oakoss/tap/tm
```

Or run the shell installer attached to the [latest release](https://github.com/oakoss/tm/releases/latest), or build from source with `cargo install --locked --path crates/tm-cli`. Until v1 the store's format can change between releases; take a copy with `tm export` before upgrading.

## More

- [Design](docs/specs/design.md): what `tm` does
- [Build plan](docs/contributing/build-plan.md): the order it is built in
- [Decisions](docs/decisions/): the hardest-to-reverse choices and why
- [Research](docs/research/): the trackers and storage engines examined on the way

Licensed under [MIT](LICENSE).
