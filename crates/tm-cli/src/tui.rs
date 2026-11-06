//! `tm tui`: the screens over this directory's Workspace (ADR 0018).

use std::io::{self, IsTerminal};
use std::process::ExitCode;

use tm_store::{Store, StoreError};
use tm_tui::Seen;

use crate::error::CliError;
use crate::output;
use crate::{here, identity, watch};

pub(crate) fn run(json: bool) -> ExitCode {
    match open(json) {
        Ok(tm_tui::Ended::Quit) => ExitCode::SUCCESS,
        // As a shell reports a process a signal ended.
        Ok(tm_tui::Ended::Signal(signal)) => {
            ExitCode::from(u8::try_from(128 + signal).unwrap_or(u8::MAX))
        }
        Err(err) => output::finish(json, Err(err), &[]),
    }
}

fn open(json: bool) -> Result<tm_tui::Ended, CliError> {
    if json || !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        return Err(CliError::new(
            "not_a_terminal",
            "tm tui draws screens, so it needs a terminal for its input and output",
        )
        .fix("run it in a terminal; tm list, tm ready and tm show print the same for a script"));
    }
    let here = here::read()?;
    let (_, key) = here::mapped_project(&here.store, &here.found, &here.cwd)?;
    let mut place = key.to_owned();
    if here.snapshot.is_some() {
        place.push_str(" (read-only Snapshot)");
    }
    // As tm list reads @me, keeping why no one can be named.
    let me =
        identity::resolve(identity::env, || identity::git_email(&here.found.dir)).map(|identity| {
            let actor = identity.actor;
            actor
                .on_behalf_of()
                .unwrap_or_else(|| actor.name())
                .to_owned()
        });
    // A Snapshot is opened immutable and a replaced store file is another
    // file, so the list's reader is opened again on each change, and first
    // after the watch starts, so no change between the two is missed.
    let snapshot = here.snapshot.is_some();
    let workspace = here.workspace;
    let open = || {
        if snapshot {
            Store::open_snapshot(&workspace.snapshot_path(), workspace.id)
        } else {
            Store::open_existing(&workspace.store_path(), workspace.id)
        }
    };
    let mut watch = watch::watch()?;
    let store = open()?;
    let me = me.as_deref().map_err(String::as_str);
    let mut pending = false;
    tm_tui::run(place, store, me, |store| {
        let (opened, seen) = refresh(watch.changed(), &mut pending, open, || {
            jiff::Zoned::now().strftime("%H:%M:%S").to_string()
        })?;
        if let Some(opened) = opened {
            *store = opened;
        }
        Ok(seen)
    })
    .map_err(|ended| match ended {
        Ended::Terminal(err) => CliError::new("io", format!("the terminal: {err}")),
        Ended::Store(err) => err,
    })
}

/// A look at the store: on a change the list's reader is opened again, and a
/// busy store leaves that `pending` for the next look, since the Watch has
/// already reported the change.
fn refresh(
    changed: Result<bool, StoreError>,
    pending: &mut bool,
    open: impl FnOnce() -> Result<Store, StoreError>,
    now: impl FnOnce() -> String,
) -> Result<(Option<Store>, Seen), Ended> {
    if !(*pending || matches!(changed, Ok(true))) {
        return seen(changed, now).map(|seen| (None, seen));
    }
    match open() {
        Ok(opened) => {
            *pending = false;
            // The reopen read the change, though the Watch may be busy now.
            let changed = match changed {
                Err(StoreError::Busy(_)) => Ok(true),
                other => other.map(|_| true),
            };
            seen(changed, now).map(|seen| (Some(opened), seen))
        }
        Err(StoreError::Busy(_)) => {
            *pending = true;
            Ok((None, Seen::Busy))
        }
        Err(err) => Err(Ended::Store(err.into())),
    }
}

/// What a look at the store shows: a busy store waits for the next look,
/// any other error ends the TUI.
fn seen(changed: Result<bool, StoreError>, now: impl FnOnce() -> String) -> Result<Seen, Ended> {
    match changed {
        Ok(true) => Ok(Seen::Changed(now())),
        Ok(false) => Ok(Seen::Nothing),
        Err(StoreError::Busy(_)) => Ok(Seen::Busy),
        Err(err) => Err(Ended::Store(err.into())),
    }
}

enum Ended {
    Terminal(io::Error),
    Store(CliError),
}

impl From<io::Error> for Ended {
    fn from(err: io::Error) -> Self {
        Self::Terminal(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn busy() -> StoreError {
        StoreError::Busy(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            None,
        ))
    }

    #[test]
    fn a_busy_store_waits_and_any_other_error_ends_the_tui() {
        let now = || "10:15:03".to_owned();
        assert!(matches!(seen(Err(busy()), now), Ok(Seen::Busy)));
        assert!(matches!(seen(Ok(false), now), Ok(Seen::Nothing)));
        assert!(matches!(seen(Ok(true), now), Ok(Seen::Changed(at)) if at == "10:15:03"));
        let gone = StoreError::Io(io::Error::from(io::ErrorKind::NotFound));
        assert!(matches!(seen(Err(gone), now), Err(Ended::Store(_))));
    }

    #[test]
    fn a_busy_reopen_is_tried_again_and_then_read_as_the_change() {
        let dir = std::env::temp_dir().join(format!("tm-refresh-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.db");
        drop(Store::open(&path).unwrap());
        let now = || "10:15:03".to_owned();
        let mut pending = false;
        let quiet = refresh(Ok(false), &mut pending, || panic!("opened"), now);
        assert!(matches!(quiet, Ok((None, Seen::Nothing))));
        let first = refresh(Ok(true), &mut pending, || Err(busy()), now);
        assert!(matches!(first, Ok((None, Seen::Busy))) && pending);
        let next = refresh(Ok(false), &mut pending, || Store::open(&path), now);
        assert!(matches!(next, Ok((Some(_), Seen::Changed(_)))) && !pending);
        let held = refresh(Ok(true), &mut pending, || Err(busy()), now);
        assert!(matches!(held, Ok((None, Seen::Busy))) && pending);
        let busy_watch = refresh(Err(busy()), &mut pending, || Store::open(&path), now);
        assert!(
            matches!(busy_watch, Ok((Some(_), Seen::Changed(_)))) && !pending,
            "a reopen that succeeds reports the change while the Watch is busy"
        );
        let mut pending = true;
        let watch_gone = Err(StoreError::Io(io::Error::from(io::ErrorKind::NotFound)));
        assert!(
            matches!(
                refresh(watch_gone, &mut pending, || Store::open(&path), now),
                Err(Ended::Store(_))
            ),
            "any other Watch error ends the TUI, reopened or not"
        );
        let gone = || Err(StoreError::Io(io::Error::from(io::ErrorKind::NotFound)));
        assert!(matches!(
            refresh(Ok(true), &mut pending, gone, now),
            Err(Ended::Store(_))
        ));
    }
}
