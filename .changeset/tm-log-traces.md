---
tm-cli: minor
---

`TM_LOG` traces what a command did, for working out a slow or surprising run: set it to a filter such as `debug`, `tm_store=debug` or `tm::init=trace`, in `RUST_LOG`'s form, and each command appends timed lines to `logs/tm.log` under tm's home (`~/.tm`, or `TM_HOME`), never to the terminal, so a command's output and the TUI are unchanged. It covers each command, each write and how long it waited for the store's lock, replays and what they set aside, Claim renewals and git calls. The log is set aside as `tm.log.1` once it passes 10 MB. A part of the filter tm cannot read is left out with a warning, and one that names nothing tm traces, such as a misspelled `debgu`, is warned about; warnings go to stderr in text mode and into the first document's `warnings` under `--json`, except in `tm export`'s records, which are not a document. Unset or `off`, nothing is traced.
