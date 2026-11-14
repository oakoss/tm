# tm traces to a file only when TM_LOG asks

- Status: accepted
- Date: 2026-11-13
- Deciders: Jace Babin

## Context and Problem Statement

tm had no logging. A slow command, a write that waited on another session's lock, an operation replay set aside or a Claim renewed when it was not expected could be explained only by reading the code or adding prints. Where can tm say what it did, without breaking what it prints?

## Decision Drivers

- [ADR 0011](0011-the-cli-speaks-one-json-envelope-and-flat-coded-errors.md): under `--json` exactly one document goes to stdout and nothing to stderr.
- `tm tui` owns the terminal, and a SessionStart hook passes only stdout to the Agent, so stderr is either seen at the wrong time or not at all.
- The cold-start budget (build-plan.md, Phase 1) holds for every read command.
- tm writes only what it owns: its home is its own; a repo is not.

## Considered Options

- **No logging**: diagnose from `tm doctor`'s set-aside operations, `tm history` and the cold-start timings.
- **A hand-written `TM_LOG=<file>`**: no dependency, a few trace points.
- **`tracing`, to a file under tm's home**: `TM_LOG` takes a `RUST_LOG`-style filter.

## Decision Outcome

Chosen option: **`tracing`, to a file under tm's home**, because it gives timed spans and per-crate filtering without tm's own code for either, and showed no difference above run-to-run noise when off.

- `TM_LOG` unset, blank, or only turning targets `off` installs no subscriber. Set, its comma-separated directives are read one by one; one tm cannot read is left out, and when none can be read nothing is traced. tm's own lines are under `tm` and `tm::<module>` (`tm::init=trace`), the store's under `tm_store`; a target matches by prefix, so `tm=debug` also takes in `tm_store`. `tracing-subscriber` would print its own complaint about an unreadable directive to stderr, so tm never hands it one.
- Traces are appended to `logs/tm.log` under the tm home (`~/.tm`, or `TM_HOME`), never to stdout or stderr; the subscriber's own report of a failed write is turned off. When the file passes 10 MB at startup it is renamed `tm.log.1`, so at most two files remain; when the rename fails, tm keeps appending.
- Lines are plain text, without colour codes: the time, level, the spans they are in and their fields. Every line is inside a `command` span naming the command and the process id, whose target (`tm_command`) every filter enables, so concurrent sessions' lines tell apart under a filter such as `tm_store=debug`; each span's close gives its duration.
- A directive left out, a log that cannot be written or set aside, or no tm home is a warning, and the command goes on: text mode prints it on stderr as tm starts; JSON mode adds it to the first document's `warnings`, per ADR 0011, so a `--watch` stream says it once, as text mode does. An export's records are not a document, so they carry none; a write to the log that fails after it opens is not reported either way.
- Trace points start where questions came from: each command, each write (its drafts, how long it waited for the lock, what it wrote or why it was refused), each rebuild and the operations it set aside, Claim renewals and heartbeats, and each git call. More are added when a question needs them.

### Consequences

- Good, because a slow or surprising run can be explained after the fact with `TM_LOG=debug`, without changing tm.
- Good, because a command's output is unchanged whether tracing is on or off, so scripts, hooks and the TUI cannot tell; only a `TM_LOG` tm cannot act on adds a warning.
- Bad, because the release binary grows by 7.0% (measured below), and the workspace gains `tracing` and `tracing-subscriber` with its `env-filter`.
- Neutral, because a misspelled level or crate, such as `debgu` or `tm_stroe=debug`, is a valid target that matches nothing; tm keeps it and warns that it names nothing tm traces. A misspelled module under a real crate, such as `tm_store::clam`, is not caught.
- Neutral, because a trace point is code to keep accurate; one that no question needs is removed.

### Confirmation

Measured on 2026-11-13 on an Apple Silicon Mac, release builds (`cargo build --release --locked -p tm-cli`): the `tm` binary is 6,818,160 bytes on main and 7,298,432 with tracing, 480,272 more.

On the TM Workspace (about 180 Issues), each command run 40 times with main's build, this branch's, and this branch's with `TM_LOG=debug` in turn, so all three saw the same load; the medians:

| Command              | main    | TM_LOG unset | TM_LOG=debug |
| -------------------- | ------- | ------------ | ------------ |
| `tm --version`       | 4.2 ms  | 4.2 ms       | 4.4 ms       |
| `tm list`            | 6.9 ms  | 7.0 ms       | 7.0 ms       |
| `tm prime`           | 13.9 ms | 14.0 ms      | 13.9 ms      |
| `tm --json show KEY` | 7.4 ms  | 7.4 ms       | 7.5 ms       |
| `tm ready`           | 7.8 ms  | 8.0 ms       | 8.1 ms       |

`mise run coldstart`, which clears the environment so measures `TM_LOG` unset, passed with every read command within budget; its medians against main's, in milliseconds:

| Issues | Command                | main | tracing | Budget |
| ------ | ---------------------- | ---- | ------- | ------ |
| 200    | `tm show TM-1`         | 6.3  | 6.4     | 40     |
| 200    | `tm list`              | 6.7  | 6.4     | 40     |
| 200    | `tm prime`             | 16.6 | 15.9    | 40     |
| 200    | `tm prime` (sandbox)   | 21.7 | 23.5    | 40     |
| 10,000 | `tm show TM-1`         | 7.6  | 6.9     | 60     |
| 10,000 | `tm list`              | 54.1 | 55.4    | 150    |
| 10,000 | `tm prime`             | 46.8 | 47.0    | 150    |
| 10,000 | `tm prime` (sandbox)   | 59.0 | 52.9    | 150    |

A first run on a machine at a load average of 26 put `tm prime` (sandbox) over budget at 200 Issues; the alternating runs above showed no difference between the builds, and the run repeated at lower load passed. Revisit if a cold-start run fails where main's passes.
