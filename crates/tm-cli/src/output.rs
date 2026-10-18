//! What a command prints: text for people, one JSON document for programs
//! (ADR 0011).

use std::fs::File;
use std::io::{self, Write as _};
use std::os::fd::AsFd as _;
use std::process::ExitCode;

use serde_json::{Value, json};

use crate::error::CliError;

pub(crate) const SCHEMA_VERSION: u32 = 1;

/// A command's result, in both forms.
pub(crate) struct Output {
    pub(crate) text: String,
    pub(crate) data: Value,
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
        (true, Ok(output)) => (
            format!(
                "{}\n",
                json!({
                    "schema_version": SCHEMA_VERSION,
                    "data": output.data,
                    "warnings": warnings,
                })
            ),
            ExitCode::SUCCESS,
        ),
        (true, Err(err)) => (format!("{}\n", err.document(warnings)), ExitCode::FAILURE),
        (false, Ok(output)) => {
            for warning in warnings {
                eprintln!("tm: warning: {warning}");
            }
            (output.text, ExitCode::SUCCESS)
        }
        (false, Err(err)) => {
            for warning in warnings {
                eprintln!("tm: warning: {warning}");
            }
            eprintln!("tm: {}", err.message);
            if let Some(fix) = &err.fix {
                eprintln!("  fix: {fix}");
            }
            return ExitCode::FAILURE;
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
    let result = io::stdout()
        .as_fd()
        .try_clone_to_owned()
        .map(File::from)
        .and_then(|mut stdout| stdout.write_all(text.as_bytes()));
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
    fn json_mode_comes_from_the_flag_before_double_dash_or_the_environment() {
        assert!(json_mode(&args(&["show", "--json", "X-1"]), None));
        assert!(json_mode(&args(&["--json", "show"]), None));
        assert!(!json_mode(&args(&["comment", "X-1", "--", "--json"]), None));
        assert!(json_mode(&args(&["show"]), Some("1")));
        assert!(!json_mode(&args(&["show"]), Some("0")));
        assert!(!json_mode(&args(&["show", "--jsonx"]), None));
    }
}
