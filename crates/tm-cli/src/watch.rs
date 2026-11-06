//! `--watch`: a list command's result again each time another process
//! writes, until interrupted (ADR 0018).

use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use tm_store::{StoreError, Watch};

use crate::error::CliError;
use crate::here;
use crate::output::{self, Output};

const POLL: Duration = Duration::from_millis(250);

/// Prints `read`'s result, then again after each write another process
/// makes: text after a line with the time it was read, `--json` one
/// document per line. A busy store is tried again at the next poll; any
/// other error ends the watch, as does the first print after the reader
/// closed the pipe.
pub(crate) fn run(json: bool, mut read: impl FnMut() -> Result<Output, CliError>) -> ExitCode {
    // Made before the first read, so a write landing during it is not missed.
    let mut watch = loop {
        match watch() {
            Ok(watch) => break watch,
            Err(err) if err.code == "busy" => {
                warn();
                thread::sleep(POLL);
            }
            Err(err) => return output::finish(json, Err(err), &[]),
        }
    };
    let mut before = String::new();
    let mut due = true;
    loop {
        if due {
            match read() {
                Err(err) if err.code == "busy" => warn(),
                result => {
                    match output::finish_again(json, result, &before) {
                        None => return ExitCode::SUCCESS,
                        Some(code) if code != ExitCode::SUCCESS => return code,
                        Some(_) => {}
                    }
                    due = false;
                }
            }
        }
        thread::sleep(POLL);
        match watch.changed() {
            Ok(false) => {}
            Ok(true) => {
                let at = jiff::Zoned::now().strftime("%H:%M:%S");
                before = format!("\n-- changed, read at {at} --\n");
                due = true;
            }
            Err(StoreError::Busy(_)) => warn(),
            Err(err) => return output::finish(json, Err(err.into()), &[]),
        }
    }
}

fn warn() {
    eprintln!(
        "tm: warning: the store is busy, as another process holds the write lock; trying again"
    );
}

/// Watches the store, or the Snapshot where this process may not open it.
fn watch() -> Result<Watch, CliError> {
    let here = here::read()?;
    let watch = if here.snapshot.is_some() {
        Watch::snapshot(&here.workspace.snapshot_path())
    } else {
        Watch::store(&here.workspace.store_path())
    };
    Ok(watch?)
}
