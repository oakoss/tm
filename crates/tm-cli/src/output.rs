//! What a command prints: text for people, one JSON document for programs
//! (ADR 0011).

use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, Write as _};
use std::os::fd::AsFd as _;
use std::process::ExitCode;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::CliError;

pub(crate) const SCHEMA_VERSION: u32 = 1;

/// A command's result, in both forms.
pub(crate) struct Output {
    pub(crate) text: String,
    pub(crate) data: Value,
    /// `data` already written as JSON; when set, `finish` prints it and
    /// ignores `data`.
    pub(crate) serialized: Option<String>,
    pub(crate) warnings: Vec<String>,
}

impl Output {
    pub(crate) fn new(text: String, data: Value) -> Self {
        Self {
            text,
            data,
            serialized: None,
            warnings: Vec::new(),
        }
    }

    /// JSON output written straight from its parts: building a value of a
    /// list's thousands of rows only to write it costs as much as reading
    /// them.
    pub(crate) fn serialized(data: String) -> Self {
        Self {
            serialized: Some(data),
            ..Self::new(String::new(), Value::Null)
        }
    }

    /// A write's result: `batch` in the data and, with `hint`, a last line
    /// saying how to undo it (ADR 0012).
    pub(crate) fn batched(mut self, batch: Uuid, hint: bool) -> Self {
        self.data["batch"] = json!(batch);
        if hint {
            let _ = writeln!(self.text, "undo this with: tm undo {}", short(batch));
        }
        self
    }
}

/// A batch id's last 12 hex digits: a `UUIDv7` begins with its time, so only
/// the end tells batches written close together apart.
pub(crate) fn short(batch: Uuid) -> String {
    let hex = batch.simple().to_string();
    hex[hex.len() - 12..].to_owned()
}

/// Stored text made safe to print: control characters, which could move the
/// cursor or clear a terminal, and characters that disguise text appear
/// escaped, as `\u{1b}` or `\u{202e}`; a tab passes through.
pub(crate) fn printable(text: &str) -> String {
    let escaped = |c: char| (c.is_control() && c != '\t') || disguises(c);
    // Almost all text needs nothing escaped, and lists print it by the
    // thousand.
    if !text.chars().any(escaped) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if escaped(c) {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// Bidirectional controls, which reorder what is shown (CVE-2021-42574); line
/// and paragraph separators, which `lines()` does not split on; and invisible
/// characters: zero-width space, word joiner and invisible operators, byte
/// order mark, and tags, which can hide text an agent reads but a person does
/// not see. Joiners stay, as emoji and scripts such as Persian need them.
const fn disguises(c: char) -> bool {
    matches!(
        c,
        '\u{061c}'
            | '\u{200b}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
            | '\u{e0000}'..='\u{e007f}'
    )
}

/// A span such as `2d 3h`, `1h 12m`, `2h` or `12m`, to the nearest minute.
pub(crate) fn span(ms: i64) -> String {
    let minutes = ms.clamp(0, i64::MAX - 30_000).saturating_add(30_000) / 60_000;
    let (days, hours, minutes) = (minutes / 1_440, minutes / 60 % 24, minutes % 60);
    match (days, hours, minutes) {
        (0, 0, 0) => "under a minute".to_owned(),
        (0, 0, _) => format!("{minutes}m"),
        (0, _, 0) => format!("{hours}h"),
        (0, _, _) => format!("{hours}h {minutes}m"),
        (_, 0, _) => format!("{days}d"),
        _ => format!("{days}d {hours}h"),
    }
}

/// An instant in this machine's time zone, to the minute.
pub(crate) fn local_time(utc_ms: i64) -> String {
    jiff::Timestamp::from_millisecond(utc_ms).map_or_else(
        |_| utc_ms.to_string(),
        |at| {
            at.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d %H:%M %Z")
                .to_string()
        },
    )
}

/// This machine's UTC offset at an instant, in minutes.
pub(crate) fn local_offset_min(utc_ms: i64) -> Option<i64> {
    let at = jiff::Timestamp::from_millisecond(utc_ms).ok()?;
    let offset = jiff::tz::TimeZone::system().to_offset(at);
    Some(i64::from(offset.seconds()) / 60)
}

/// An instant at a recorded UTC offset, to the minute, such as
/// `2026-10-22 19:49 +05:30`; an offset that is not a real one prints UTC.
pub(crate) fn time_at(utc_ms: i64, offset_min: i64) -> String {
    let offset = i32::try_from(offset_min * 60)
        .ok()
        .and_then(|seconds| jiff::tz::Offset::from_seconds(seconds).ok())
        .unwrap_or(jiff::tz::Offset::UTC);
    jiff::Timestamp::from_millisecond(utc_ms).map_or_else(
        |_| utc_ms.to_string(),
        |at| {
            at.to_zoned(jiff::tz::TimeZone::fixed(offset))
                .strftime("%Y-%m-%d %H:%M %:z")
                .to_string()
        },
    )
}

/// Whether to print JSON, decided before the arguments are parsed so a
/// usage error is a document too. A `--json` after `--` is an argument, not
/// the flag.
pub(crate) fn json_mode(args: &[String], env: Option<&str>) -> bool {
    env == Some("1")
        || args
            .iter()
            .skip(1)
            .take_while(|arg| *arg != "--")
            .any(|arg| arg == "--json")
}

/// In JSON mode a panic still ends in one document on stdout, and exits 1
/// rather than aborting.
pub(crate) fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "tm panicked".to_owned());
        let err = CliError::new("internal", message)
            .with("cause", "panic")
            .with("location", info.location().map(ToString::to_string));
        let _ = write(&format!("{}\n", err.document(&[])));
        std::process::exit(1);
    }));
}

/// Prints the result and returns the exit code: 0 on success, 1 on failure.
pub(crate) fn finish(
    json: bool,
    result: Result<Output, CliError>,
    warnings: &[String],
) -> ExitCode {
    let (text, code) = match (json, result) {
        // The keys in the order a JSON value sorts them, so the bytes match.
        (
            true,
            Ok(Output {
                serialized: Some(data),
                warnings: own,
                ..
            }),
        ) => (
            format!(
                "{{\"data\":{data},\"schema_version\":{SCHEMA_VERSION},\"warnings\":{}}}\n",
                json!(warnings.iter().chain(&own).collect::<Vec<_>>())
            ),
            ExitCode::SUCCESS,
        ),
        (true, Ok(output)) => (
            format!(
                "{}\n",
                json!({
                    "schema_version": SCHEMA_VERSION,
                    "data": output.data,
                    "warnings": warnings.iter().chain(&output.warnings).collect::<Vec<_>>(),
                })
            ),
            ExitCode::SUCCESS,
        ),
        (true, Err(err)) => (format!("{}\n", err.document(warnings)), err.exit_code()),
        (false, Ok(output)) => {
            for warning in warnings.iter().chain(&output.warnings) {
                eprintln!("tm: warning: {warning}");
            }
            (output.text, ExitCode::SUCCESS)
        }
        (false, Err(err)) => {
            for warning in warnings.iter().chain(&err.warnings) {
                eprintln!("tm: warning: {warning}");
            }
            eprintln!("tm: {}", err.message);
            if let Some(fix) = &err.fix {
                eprintln!("  fix: {fix}");
            }
            return err.exit_code();
        }
    };
    match write(&text) {
        Ok(()) => code,
        Err(err) => {
            if !json {
                eprintln!("tm: cannot write output: {err}");
            }
            ExitCode::FAILURE
        }
    }
}

/// A reader that stops early, as `head` does, is not an error, but a full
/// disk is, and so is a stdout not open for writing. Writes go through a copy
/// of the descriptor: std's stdout drops such a write and reports success.
fn write(text: &str) -> io::Result<()> {
    quiet_on_broken_pipe(stdout().and_then(|mut stdout| stdout.write_all(text.as_bytes())))
}

/// Stdout as a file: unlike `io::stdout()`, it reports a descriptor that
/// cannot be written instead of swallowing the error.
pub(crate) fn stdout() -> io::Result<File> {
    io::stdout().as_fd().try_clone_to_owned().map(File::from)
}

/// A reader that closed the pipe, as `head` does, has had what it wanted.
pub(crate) fn quiet_on_broken_pipe(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("tm")
            .chain(list.iter().copied())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn a_span_reads_in_its_two_largest_units() {
        for (ms, shown) in [
            (-5, "under a minute"),
            (29_999, "under a minute"),
            (30_000, "1m"),
            (3_569_999, "59m"),
            (7_199_000, "2h"),
            (4_320_000, "1h 12m"),
            (86_369_999, "23h 59m"),
            (86_399_999, "1d"),
            (86_460_000, "1d"),
            (183_600_000, "2d 3h"),
        ] {
            assert_eq!(span(ms), shown, "{ms}");
        }
    }

    #[test]
    fn printable_escapes_what_disguises_text_and_keeps_joiners() {
        for (stored, shown) in [
            ("Fix login\u{202e}gnirts", r"Fix login\u{202e}gnirts"),
            ("a\u{2066}b\u{2069}", r"a\u{2066}b\u{2069}"),
            ("one\u{2028}two", r"one\u{2028}two"),
            ("TM\u{200b}-1", r"TM\u{200b}-1"),
            ("\u{feff}title", r"\u{feff}title"),
            ("\u{200f}\u{061c}", r"\u{200f}\u{61c}"),
            ("a\u{2060}b", r"a\u{2060}b"),
            ("a\u{e0041}b", r"a\u{e0041}b"),
            ("👩\u{200d}💻", "👩\u{200d}💻"),
            ("می\u{200c}خواهم", "می\u{200c}خواهم"),
            ("a\tb", "a\tb"),
        ] {
            assert_eq!(printable(stored), shown, "{stored:?}");
        }
    }

    #[test]
    fn json_mode_comes_from_the_flag_before_double_dash_or_the_environment() {
        assert!(json_mode(&args(&["show", "--json", "X-1"]), None));
        assert!(json_mode(&args(&["--json", "show"]), None));
        assert!(!json_mode(&args(&["comment", "X-1", "--", "--json"]), None));
        assert!(json_mode(&args(&["show"]), Some("1")));
        assert!(!json_mode(&args(&["show"]), Some("0")));
        assert!(!json_mode(&args(&["show", "--jsonx"]), None));
    }
}
