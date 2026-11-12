//! `--watch`: a list command's result again each time another process
//! writes or a held Claim lapses, until interrupted (ADR 0018).

use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use tm_store::{StoreError, Watch};

use crate::error::CliError;
use crate::here;
use crate::output::{self, Output};

const POLL: Duration = Duration::from_millis(250);

/// Prints `read`'s result, then again after each write another process
/// makes or Claim that lapses: text after a line with the time it was read, `--json` one
/// document per line. A busy store is tried again at the next poll; any
/// other error ends the watch, as does the first print after the reader
/// closed the pipe.
pub(crate) fn run(json: bool, read: impl FnMut() -> Result<Output, CliError>) -> ExitCode {
    watching(json, watch, read, |result, before, warnings| {
        output::finish_again(json, result, before, warnings)
    })
}

/// [`run`], with the Watch made by `make` and each result printed by
/// `print`, as [`output::finish_again`] does.
fn watching(
    json: bool,
    mut make: impl FnMut() -> Result<Watch, CliError>,
    mut read: impl FnMut() -> Result<Output, CliError>,
    mut print: impl FnMut(Result<Output, CliError>, &str, &[String]) -> Option<ExitCode>,
) -> ExitCode {
    // Text warns at once; JSON, which writes nothing to stderr (ADR 0011),
    // carries it in the next document's warnings.
    let mut held: Vec<String> = Vec::new();
    let busy = |held: &mut Vec<String>| {
        if !json {
            eprintln!(
                "tm: warning: the store is busy, as another process holds the write lock; trying again"
            );
        } else if held.is_empty() {
            held.push(
                "the store was busy, as another process held the write lock; tried again"
                    .to_owned(),
            );
        }
    };
    // Made before the first read, so a write landing during it is not missed.
    let mut watch = loop {
        match make() {
            Ok(watch) => break watch,
            Err(err) if err.code == "busy" => {
                busy(&mut held);
                thread::sleep(POLL);
            }
            Err(err) => return output::finish(json, Err(err), &held),
        }
    };
    let mut before = String::new();
    let mut due = true;
    loop {
        if due {
            match read() {
                Err(err) if err.code == "busy" => busy(&mut held),
                result => {
                    match print(result, &before, &std::mem::take(&mut held)) {
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
            Err(StoreError::Busy(_)) => busy(&mut held),
            Err(err) => return output::finish(json, Err(err.into()), &held),
        }
    }
}

/// Watches the store, or the Snapshot where this process may not open it.
pub(crate) fn watch() -> Result<Watch, CliError> {
    let here = here::read()?;
    let watch = if here.snapshot.is_some() {
        Watch::snapshot(&here.workspace.snapshot_path())
    } else {
        Watch::store(&here.workspace.store_path())
    };
    Ok(watch?)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use tm_store::Store;
    use uuid::Uuid;

    use super::*;

    fn store() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tm-watch-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.db");
        drop(Store::open(&path).unwrap());
        path
    }

    /// Ends the watch at its first print, keeping the warnings it was given.
    fn first_print(
        printed: &RefCell<Vec<String>>,
    ) -> impl FnMut(Result<Output, CliError>, &str, &[String]) -> Option<ExitCode> {
        |_, _, warnings| {
            printed.borrow_mut().extend_from_slice(warnings);
            Some(ExitCode::FAILURE)
        }
    }

    #[test]
    fn the_watch_is_made_before_the_first_read() {
        let path = store();
        let calls = RefCell::new(Vec::new());
        let printed = RefCell::new(Vec::new());
        watching(
            true,
            || {
                calls.borrow_mut().push("watch");
                Ok(Watch::store(&path)?)
            },
            || {
                calls.borrow_mut().push("read");
                Err(CliError::new("stop", "stop"))
            },
            first_print(&printed),
        );
        assert_eq!(*calls.borrow(), ["watch", "read"]);
    }

    /// Busy at the start, then on the first read, then read.
    fn busy_twice(json: bool) -> ((usize, usize), Vec<String>) {
        let path = store();
        let (made, read) = (RefCell::new(0), RefCell::new(0));
        let printed = RefCell::new(Vec::new());
        let busy = || CliError::new("busy", "busy");
        watching(
            json,
            || {
                *made.borrow_mut() += 1;
                if *made.borrow() == 1 {
                    return Err(busy());
                }
                Ok(Watch::store(&path)?)
            },
            || {
                *read.borrow_mut() += 1;
                if *read.borrow() == 1 {
                    return Err(busy());
                }
                Ok(Output::new(String::new(), serde_json::Value::Null))
            },
            first_print(&printed),
        );
        let counts = (*made.borrow(), *read.borrow());
        (counts, printed.into_inner())
    }

    #[test]
    fn a_busy_store_is_tried_again_and_json_carries_it_once_in_the_next_document() {
        let (counts, warnings) = busy_twice(true);
        assert_eq!(counts, (2, 2));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("busy"), "{warnings:?}");
        let (_, warnings) = busy_twice(false);
        assert!(warnings.is_empty(), "text warns on stderr at once");
    }
}
