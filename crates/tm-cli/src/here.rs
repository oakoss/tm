//! The Workspace the working directory maps to, opened for a command.

use std::path::{Path, PathBuf};

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
    let (home, cwd, found) = find()?;
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
    let cwd = canonical(&cwd)?;
    Ok(Here {
        found,
        cwd,
        store,
        warnings,
    })
}

/// The path writes to Issues and Claims take: the store's warnings come
/// before the identity warnings `write` returns, on success and failure.
pub(crate) fn write(
    write: impl FnOnce(&mut Here) -> Result<(Output, Vec<String>), CliError>,
) -> Result<Output, CliError> {
    let mut here = open()?;
    let warnings = std::mem::take(&mut here.warnings);
    match write(&mut here) {
        Ok((mut output, identity_warnings)) => {
            output.warnings = warnings;
            output.warnings.extend(identity_warnings);
            Ok(output)
        }
        Err(err) => Err(err.warned(warnings)),
    }
}

/// tm's home, the working directory and its repo file, with the store unopened.
///
/// # Errors
///
/// As [`open`], before the store is opened.
pub(crate) fn find() -> Result<(PathBuf, PathBuf, Found), CliError> {
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
    Ok((home, cwd, found))
}

/// The Project, and its key, that the repo file maps `cwd` to.
///
/// # Errors
///
/// `not_found` when the Workspace has no Project with the mapped key.
pub(crate) fn mapped_project<'a>(
    store: &Store,
    found: &'a Found,
    cwd: &Path,
) -> Result<(ProjectId, &'a str), CliError> {
    let relative = cwd.strip_prefix(&found.dir).unwrap_or(cwd);
    let key = found.file.project_for(relative);
    let project = store.project_by_key(key)?.ok_or_else(|| {
        CliError::new(
            "not_found",
            format!(
                "{} maps this directory to Project {key}, which this Workspace does not have",
                found.path.display()
            ),
        )
        .with("kind", "project")
        .with("reference", key)
    })?;
    Ok((project, key))
}

/// The working directory as the repo file's directory is written, so one
/// strips the other.
pub(crate) fn canonical(cwd: &Path) -> Result<PathBuf, CliError> {
    std::fs::canonicalize(cwd).map_err(|err| {
        CliError::new("io", format!("working directory: {err}"))
            .with("path", cwd.display().to_string())
    })
}

impl Here {
    /// A write that changed nothing gets no undo hint, since following it fails.
    pub(crate) fn undoable(&self, output: Output, batch: Uuid) -> Result<Output, CliError> {
        Ok(output.batched(batch, self.store.can_undo(batch)?))
    }

    /// As [`mapped_project`], for the working directory.
    pub(crate) fn project(&self) -> Result<ProjectId, CliError> {
        mapped_project(&self.store, &self.found, &self.cwd).map(|(project, _)| project)
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
