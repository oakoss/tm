//! `tm tui`: the screens over this directory's Workspace (ADR 0018).

use std::io::{self, IsTerminal};
use std::process::ExitCode;

use tm_store::StoreError;
use tm_tui::Seen;

use crate::error::CliError;
use crate::output;
use crate::{here, watch};

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
    drop(here);
    let mut watch = watch::watch()?;
    tm_tui::run(place, || match watch.changed() {
        Ok(true) => Ok(Seen::Changed(
            jiff::Zoned::now().strftime("%H:%M:%S").to_string(),
        )),
        Ok(false) => Ok(Seen::Nothing),
        Err(StoreError::Busy(_)) => Ok(Seen::Busy),
        Err(err) => Err(Ended::Store(err.into())),
    })
    .map_err(|ended| match ended {
        Ended::Terminal(err) => CliError::new("io", format!("the terminal: {err}")),
        Ended::Store(err) => err,
    })
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
