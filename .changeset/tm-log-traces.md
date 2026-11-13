---
tm-cli: minor
---

`TM_LOG` traces what a command did, for working out a slow or surprising run: set it to a filter such as `debug` or `tm_store=debug`, in `RUST_LOG`'s form, and each command appends timed lines to `logs/tm.log` under tm's home (`~/.tm`, or `TM_HOME`), never to the terminal, so `--json` output and the TUI are unchanged. It covers each command, each write and how long it waited for the store's lock, replays and what they set aside, Claim renewals and git calls. The log is set aside as `tm.log.1` once it passes 10 MB. A part of the filter tm cannot read is left out, with a warning in text mode only. Unset, nothing is traced.
