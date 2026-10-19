//! The Workspace the working directory maps to, opened for a command.

use std::path::PathBuf;

use tm_core::ProjectId;
use tm_store::Store;
use tm_store::workspace::{self, Found};
use uuid::Uuid;

use crate::error::CliError;
use crate::output::Output;

pub(crate) struct Here {
    /// The repo file that applies, and the directory holding it.
    pub(crate) found: Found,
    /// The working directory, resolved, for the path-to-Project mapping.
    pub(crate) cwd: PathBuf,
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
    let cwd = std::fs::canonicalize(&cwd).map_err(|err| {
        CliError::new("io", format!("working directory: {err}"))
            .with("path", cwd.display().to_string())
    })?;
    Ok(Here {
        found,
        cwd,
        store,
        warnings,
    })
}

impl Here {
    /// A write that changed nothing gets no undo hint, since following it fails.
    pub(crate) fn undoable(&self, output: Output, batch: Uuid) -> Result<Output, CliError> {
        Ok(output.batched(batch, self.store.can_undo(batch)?))
    }

    /// The Project the repo file maps the working directory to.
    ///
    /// # Errors
    ///
    /// `not_found` when the Workspace has no Project with the mapped key.
    pub(crate) fn project(&self) -> Result<ProjectId, CliError> {
        let relative = self.cwd.strip_prefix(&self.found.dir).unwrap_or(&self.cwd);
        let key = self.found.file.project_for(relative);
        self.store.project_by_key(key)?.ok_or_else(|| {
            CliError::new(
                "not_found",
                format!(
                    "{} maps this directory to Project {key}, which this Workspace does not have",
                    self.found.path.display()
                ),
            )
            .with("kind", "project")
            .with("reference", key)
        })
    }

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
