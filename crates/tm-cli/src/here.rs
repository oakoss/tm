//! The Workspace the working directory maps to, opened for a command.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use tm_core::{Actor, ProjectId};
use tm_store::Store;
use tm_store::workspace::{self, Found, Workspace, WorkspaceError};
use uuid::Uuid;

use crate::error::CliError;
use crate::output::{Output, local_time};

pub(crate) struct Here {
    /// The repo file that applies, and the directory holding it.
    pub(crate) found: Found,
    /// The working directory, resolved, for the path-to-Project mapping.
    pub(crate) cwd: PathBuf,
    pub(crate) store: Store,
    pub(crate) workspace: Workspace,
    /// Where the log ended when the store was opened.
    log_end: i64,
    /// The warning that this is the Snapshot, not the store, when it is.
    pub(crate) snapshot: Option<String>,
    /// What every command reports, such as operations that did not apply.
    pub(crate) warnings: Vec<String>,
}

impl Here {
    /// Publishes the Snapshot when this command committed a write.
    pub(crate) fn publish(&self) -> Option<String> {
        match self.store.log_end() {
            Ok(end) if end == self.log_end => None,
            _ => publish(&self.store, &self.workspace),
        }
    }
}

/// Publishes `workspace`'s Snapshot from `store`. A failure is a warning, not
/// an error: the write stands, and the next one publishes again.
pub(crate) fn publish(store: &Store, workspace: &Workspace) -> Option<String> {
    store
        .publish_snapshot(&workspace.snapshot_path())
        .err()
        .map(|err| not_published(&err))
}

pub(crate) fn not_published(cause: &dyn std::fmt::Display) -> String {
    format!("the read-only Snapshot was not updated: {cause}")
}

/// Opens the store the repo file at or above the working directory names.
///
/// # Errors
///
/// `no_workspace` with the fix `tm init` when no repo file maps the
/// directory, or the Workspace and store errors opening it.
pub(crate) fn open() -> Result<Here, CliError> {
    let (home, cwd, found) = find()?;
    match workspace::workspace_for(&home, &found) {
        Ok((workspace, store)) => opened(found, &cwd, workspace, store),
        Err(err) => Err(match refused(&err) {
            Some(path) => CliError::new(
                "read_only",
                format!(
                    "{}: this process may not open the store for writing, as in a sandbox",
                    path.display()
                ),
            )
            .fix("run it where this process may write the store"),
            None => err.into(),
        }),
    }
}

/// [`open`] for a command that only reads. Where this process may not open
/// the store, as in a sandbox that denies every write, it reads the Snapshot
/// the last write published, with a warning saying when.
///
/// # Errors
///
/// As [`open`], or `read_only` when no Snapshot has been published.
pub(crate) fn read() -> Result<Here, CliError> {
    let (home, cwd, found) = find()?;
    let store_path = match workspace::workspace_for(&home, &found) {
        Ok((workspace, store)) => return opened(found, &cwd, workspace, store),
        Err(err) => match refused(&err) {
            Some(path) => path.to_path_buf(),
            None => return Err(err.into()),
        },
    };
    let workspace = workspace::locate(&home, &found)?;
    let path = workspace.snapshot_path();
    let Ok(published) = fs::metadata(&path).and_then(|metadata| metadata.modified()) else {
        return Err(CliError::new(
            "read_only",
            format!(
                "{}: this process may not open the store, as in a sandbox, and no Snapshot has been published to read instead",
                store_path.display()
            ),
        )
        .fix("run any write where tm may write the store, which publishes one"));
    };
    let store = Store::open_snapshot(&path, workspace.id)?;
    let mut here = opened(found, &cwd, workspace, store)?;
    let published_ms = published
        .duration_since(UNIX_EPOCH)
        .map_or(0, |age| i64::try_from(age.as_millis()).unwrap_or(i64::MAX));
    let published = format!(
        "read from the Snapshot published {}",
        local_time(published_ms)
    );
    here.warnings.insert(0, published.clone());
    here.snapshot = Some(published);
    Ok(here)
}

/// The store this process was refused, when that is why opening failed.
fn refused(err: &WorkspaceError) -> Option<&Path> {
    match err {
        WorkspaceError::OpenStore { path, source } if source.is_refused() => Some(path),
        _ => None,
    }
}

fn opened(found: Found, cwd: &Path, workspace: Workspace, store: Store) -> Result<Here, CliError> {
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
    let cwd = canonical(cwd)?;
    let log_end = store.log_end()?;
    Ok(Here {
        found,
        cwd,
        store,
        workspace,
        log_end,
        snapshot: None,
        warnings,
    })
}

/// The path writes to Issues and Claims take: the store's warnings come
/// before the identity warnings `write` returns, on success and failure; on
/// success the output's own follow, and a failed publish's last.
pub(crate) fn write(
    write: impl FnOnce(&mut Here) -> Result<(Output, Vec<String>), CliError>,
) -> Result<Output, CliError> {
    let (mut output, mut warnings, published) = write_with(write)?;
    warnings.append(&mut output.warnings);
    warnings.extend(published);
    output.warnings = warnings;
    Ok(output)
}

/// As [`write`], for a caller that places its own warnings: the store's and
/// the identity's, then a failed publish's, apart.
pub(crate) fn write_with<T>(
    write: impl FnOnce(&mut Here) -> Result<(T, Vec<String>), CliError>,
) -> Result<(T, Vec<String>, Option<String>), CliError> {
    let mut here = open()?;
    let mut warnings = std::mem::take(&mut here.warnings);
    let written = write(&mut here);
    let published = here.publish();
    match written {
        Ok((value, identity_warnings)) => {
            warnings.extend(identity_warnings);
            Ok((value, warnings, published))
        }
        Err(err) => {
            warnings.extend(published);
            Err(err.warned(warnings))
        }
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
    /// A write that changed nothing gets no undo hint, since following it
    /// fails. `writer` is warned of each Issue it wrote whose Claim another
    /// Agent took over from it, as no write is protected yet (ADR 0009).
    pub(crate) fn undoable(
        &self,
        mut output: Output,
        batch: Uuid,
        writer: &Actor,
    ) -> Result<Output, CliError> {
        // The write has landed, so a failed check only warns.
        match self.takeovers(writer, Some(batch)) {
            Ok(warnings) => output
                .warnings
                .extend(warnings.into_iter().map(|(_, warning)| warning)),
            Err(err) => output.warnings.push(format!(
                "the write landed, but whether another Agent took over your Claims is unknown: {}",
                err.message
            )),
        }
        Ok(output.batched(batch, self.store.can_undo(batch)?))
    }

    /// A warning for each Claim another Agent took over from `writer`, among
    /// the Issues `batch` wrote, or all of them, with the id of the Claim
    /// that took it over.
    pub(crate) fn takeovers(
        &self,
        writer: &Actor,
        batch: Option<Uuid>,
    ) -> Result<Vec<(Uuid, String)>, CliError> {
        let mut warnings = Vec::new();
        for held in self.store.taken_over_from(writer, batch)? {
            let key = self.store.issue(held.issue)?.and_then(|view| view.key);
            let warning = format!(
                "{} is now held by {} for {}; your Claim lapsed",
                key.unwrap_or_else(|| held.issue.to_string()),
                held.taker(),
                held.on_behalf_of
            );
            warnings.push((held.id, warning));
        }
        Ok(warnings)
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
