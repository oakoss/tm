//! The Workspace the working directory maps to, opened for a command.

use tm_store::Store;
use tm_store::workspace;
use uuid::Uuid;

use crate::error::CliError;

pub(crate) struct Here {
    pub(crate) store: Store,
    /// What every command reports, such as operations that did not apply.
    pub(crate) warnings: Vec<String>,
}

/// Opens the store the repo file at or above the working directory names.
///
/// # Errors
///
/// `no_workspace` with the fix `tm init` when no repo file maps the
/// directory, or the Workspace and store errors opening it.
pub(crate) fn open() -> Result<Here, CliError> {
    let cwd = std::env::current_dir()
        .map_err(|err| CliError::new("io", format!("working directory: {err}")))?;
    let home = workspace::home(std::env::var_os("TM_HOME"), std::env::home_dir())?;
    let found = workspace::discover(&cwd)?.ok_or_else(|| {
        CliError::new(
            "no_workspace",
            "no .config/tm.toml maps this directory to a Workspace",
        )
        .fix("tm init")
    })?;
    let (_, store) = workspace::workspace_for(&home, &found)?;
    let unapplied = store
        .unapplied()?
        .iter()
        .filter(|row| !row.is_lost_claim())
        .count();
    let warnings = if unapplied == 0 {
        Vec::new()
    } else {
        vec![format!(
            "{unapplied} logged operation{} did not apply in full; tm doctor will list {}",
            if unapplied == 1 { "" } else { "s" },
            if unapplied == 1 { "it" } else { "them" }
        )]
    };
    Ok(Here { store, warnings })
}

impl Here {
    /// The Issue a Key, matched without case and through retired Project
    /// keys, or a full UUID names. A UUID prefix is never guessed at.
    ///
    /// # Errors
    ///
    /// `not_found` when nothing has that Key or id, or `invalid_value` when
    /// the text is neither.
    pub(crate) fn issue(&self, reference: &str) -> Result<Uuid, CliError> {
        let not_found = || {
            CliError::new("not_found", format!("no Issue is {reference}"))
                .with("kind", "issue")
                .with("reference", reference)
        };
        if let Ok(id) = Uuid::parse_str(reference) {
            return match self.store.issue(id)? {
                Some(_) => Ok(id),
                None => Err(not_found()),
            };
        }
        self.store.issue_by_key(reference)?.ok_or_else(not_found)
    }
}
