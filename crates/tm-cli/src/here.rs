//! The Workspace the working directory maps to, opened for a command.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tm_core::{Actor, ProjectId};
use tm_store::workspace::{self, Found, Workspace, WorkspaceError};
use tm_store::{Store, StoreError, Workplace};
use uuid::Uuid;

use crate::backup;
use crate::error::CliError;
use crate::identity::{self, Identity, Var};
use crate::output::{Output, local_time};

/// How a command reads an environment variable, as [`identity::env`] does.
pub(crate) type Env = fn(&str) -> Option<Var>;

/// Where a command looks for its Workspace and its writer.
#[derive(Clone)]
pub(crate) struct Place {
    /// tm's home on this machine.
    pub(crate) home: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) env: Env,
}

impl Place {
    /// This process's: `TM_HOME` or the home directory, and its working
    /// directory and environment.
    pub(crate) fn process() -> Result<Self, CliError> {
        let cwd = std::env::current_dir()
            .map_err(|err| CliError::new("io", format!("working directory: {err}")))?;
        let home = workspace::home(std::env::var_os("TM_HOME"), std::env::home_dir())?;
        Ok(Self {
            home,
            cwd,
            env: identity::env,
        })
    }

    fn found(&self) -> Result<Found, CliError> {
        workspace::discover(&self.cwd)?.ok_or_else(|| {
            CliError::new(
                "no_workspace",
                "no .config/tm.toml maps this directory to a Workspace",
            )
            .fix("tm init")
        })
    }
}

pub(crate) struct Here {
    /// tm's home on this machine.
    pub(crate) home: PathBuf,
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
    pub(crate) env: Env,
}

impl Here {
    /// When this command committed a write, publishes the Snapshot and takes
    /// the day's backup; the warnings of what failed.
    pub(crate) fn after_write(&self) -> Vec<String> {
        match self.store.log_end() {
            Ok(end) if end == self.log_end => Vec::new(),
            _ => publish(&self.store, &self.workspace)
                .into_iter()
                .chain(backup::daily(self, jiff::Zoned::now().date()))
                .collect(),
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
    open_at(&Place::process()?)
}

/// [`open`] at `place`.
pub(crate) fn open_at(place: &Place) -> Result<Here, CliError> {
    let found = place.found()?;
    match workspace::workspace_for(&place.home, &found) {
        Ok((workspace, store)) => opened(place, found, workspace, store),
        Err(err) => Err(match refused(&err) {
            Some(path) => CliError::new(
                "read_only",
                format!(
                    "{}: this process may not open the store for writing, as in a sandbox; run it where it may",
                    path.display()
                ),
            ),
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
    read_at(&Place::process()?)
}

/// [`read`] at `place`.
pub(crate) fn read_at(place: &Place) -> Result<Here, CliError> {
    let found = place.found()?;
    let store_path = match workspace::workspace_for(&place.home, &found) {
        Ok((workspace, store)) => return opened(place, found, workspace, store),
        Err(err) => match refused(&err) {
            Some(path) => path.to_path_buf(),
            None => return Err(err.into()),
        },
    };
    let workspace = workspace::locate(&place.home, &found)?;
    let path = workspace.snapshot_path();
    let Ok(published) = fs::metadata(&path).and_then(|metadata| metadata.modified()) else {
        return Err(CliError::new(
            "read_only",
            format!(
                "{}: this process may not open the store, as in a sandbox, and no Snapshot has been published to read instead; any write where tm may write the store publishes one",
                store_path.display()
            ),
        ));
    };
    let store = Store::open_snapshot(&path, workspace.id)?;
    let mut here = opened(place, found, workspace, store)?;
    let published = snapshot_read(published);
    here.warnings.insert(0, published.clone());
    here.snapshot = Some(published);
    Ok(here)
}

/// The warning that what is shown was read from a Snapshot published at
/// `published`.
pub(crate) fn snapshot_read(published: SystemTime) -> String {
    let published_ms = published
        .duration_since(UNIX_EPOCH)
        .map_or(0, |age| i64::try_from(age.as_millis()).unwrap_or(i64::MAX));
    format!(
        "read from the Snapshot published {}",
        local_time(published_ms)
    )
}

/// The warning every command gives while logged operations have not applied
/// in full, a lost Claim apart.
///
/// # Errors
///
/// Reading the store.
pub(crate) fn unapplied(store: &Store) -> Result<Option<String>, StoreError> {
    let unapplied = store
        .unapplied()?
        .iter()
        .filter(|row| !row.is_lost_claim())
        .count();
    Ok((unapplied > 0).then(|| {
        format!(
            "{unapplied} logged operation{} did not apply in full; tm doctor will list {}",
            if unapplied == 1 { "" } else { "s" },
            if unapplied == 1 { "it" } else { "them" }
        )
    }))
}

/// The store this process was refused, when that is why opening failed.
fn refused(err: &WorkspaceError) -> Option<&Path> {
    match err {
        WorkspaceError::OpenStore { path, source } if source.is_refused() => Some(path),
        _ => None,
    }
}

fn opened(
    place: &Place,
    found: Found,
    workspace: Workspace,
    store: Store,
) -> Result<Here, CliError> {
    let warnings = unapplied(&store)?.into_iter().collect();
    let cwd = canonical(&place.cwd)?;
    let log_end = store.log_end()?;
    let store = store.working_in(Workplace {
        dir: Some(found.dir.display().to_string()),
        harnesses: identity::harness_sessions(place.env),
    });
    Ok(Here {
        home: place.home.clone(),
        found,
        cwd,
        store,
        workspace,
        log_end,
        snapshot: None,
        warnings,
        env: place.env,
    })
}

/// The path writes to Issues and Claims take: the store's warnings come
/// before the identity warnings `write` returns, on success and failure; on
/// success the output's own follow, and those of a failed publish or backup
/// last.
pub(crate) fn write(
    write: impl FnOnce(&mut Here) -> Result<(Output, Vec<String>), CliError>,
) -> Result<Output, CliError> {
    write_at(&Place::process()?, write)
}

/// [`write`] at `place`.
pub(crate) fn write_at(
    place: &Place,
    write: impl FnOnce(&mut Here) -> Result<(Output, Vec<String>), CliError>,
) -> Result<Output, CliError> {
    let (mut output, mut warnings, published) = write_with_at(place, write)?;
    warnings.append(&mut output.warnings);
    warnings.extend(published);
    // A command may warn of a Claim the store also reports passing.
    let mut seen = HashSet::new();
    warnings.retain(|warning| seen.insert(warning.clone()));
    output.warnings = warnings;
    Ok(output)
}

/// As [`write`], for a caller that places its own warnings: the store's and
/// the identity's, then those of a failed publish or backup, apart.
pub(crate) fn write_with<T>(
    write: impl FnOnce(&mut Here) -> Result<(T, Vec<String>), CliError>,
) -> Result<(T, Vec<String>, Vec<String>), CliError> {
    write_with_at(&Place::process()?, write)
}

fn write_with_at<T>(
    place: &Place,
    write: impl FnOnce(&mut Here) -> Result<(T, Vec<String>), CliError>,
) -> Result<(T, Vec<String>, Vec<String>), CliError> {
    let mut here = open_at(place)?;
    let mut warnings = std::mem::take(&mut here.warnings);
    let written = write(&mut here);
    if written.is_ok() {
        warnings.extend(here.record_name());
    }
    warnings.extend(here.passed());
    let published = here.after_write();
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
    let place = Place::process()?;
    let found = place.found()?;
    Ok((place.home, place.cwd, found))
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

    /// A warning for each Issue whose protected field a human's or a forced
    /// write set while another session's Claim held it. The write has
    /// landed, so a Key that cannot be read falls back to the id.
    fn passed(&mut self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut warnings = Vec::new();
        for held in self.store.passed_claims() {
            if !seen.insert(held.issue) {
                continue;
            }
            match self.store.issue(held.issue) {
                Ok(view) => warnings.push(crate::claim::held_warning(
                    view.and_then(|view| view.key).as_deref(),
                    &held,
                )),
                Err(err) => warnings.push(format!(
                    "{}; its Key could not be read: {err}",
                    crate::claim::held_warning(None, &held)
                )),
            }
        }
        warnings
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

    /// Records git's `user.name` as the name of the person writing as git's
    /// `user.email` (ADR 0019), once their command has written: a dry run or
    /// a refusal leaves the log as it was. Under `TM_ACTOR` or as an Agent
    /// git's name is not the writer's; a warning when recording fails.
    fn record_name(&mut self) -> Option<String> {
        let env = self.env;
        if env("TM_ACTOR").is_some() || identity::agent_detected(env) {
            return None;
        }
        if self.store.appended() == 0 {
            return None;
        }
        let email = identity::git_email(&self.found.dir).ok()?;
        let name = match identity::git_name(&self.found.dir) {
            Ok(name) => name?,
            Err(why) => return Some(format!("cannot record your name: {why}")),
        };
        let project = self.project().ok()?;
        let recorded = self
            .store
            .record_name(&Actor::human(&email), project, &name);
        recorded
            .err()
            .map(|err| format!("cannot record your name {name:?} from git's user.name: {err}"))
    }

    /// Who writes from here. A command that goes on without one asks
    /// [`identity::writer_in`] instead.
    ///
    /// # Errors
    ///
    /// `no_identity` when tm cannot tell.
    pub(crate) fn writer(&self) -> Result<Identity, CliError> {
        identity::required_in(self.env, &self.found.dir)
    }

    /// Who reads from here, as [`identity::reader`] tells writers apart.
    ///
    /// # Errors
    ///
    /// Why tm cannot tell, for the caller to word.
    pub(crate) fn reader(&self) -> Result<Identity, String> {
        identity::reader_in(self.env, &self.found.dir)
    }

    /// The Issue a Key, matched without case and through retired Project
    /// keys, a full UUID, or an Issue's id at an import's source names, in
    /// that order. A UUID prefix is never guessed at.
    ///
    /// # Errors
    ///
    /// `not_found` when nothing has that Key or id, naming `tm show` when it
    /// is an imported comment's id, or `invalid_value` when the text is no
    /// Key and no imported id, or an imported id more than one Issue holds.
    pub(crate) fn issue(&self, reference: &str) -> Result<Uuid, CliError> {
        let (issue, refused) = self.lookup(reference)?;
        if let Some(issue) = issue {
            return Ok(issue);
        }
        if !self.store.comments_by_import_id(reference)?.is_empty() {
            return Err(CliError::new(
                "not_found",
                format!("{reference} is a comment; tm show {reference} shows its Issue"),
            )
            .with("kind", "issue")
            .with("reference", reference));
        }
        Err(refused)
    }

    /// The Issue a reference names, as [`Here::issue`] reads it, or an
    /// imported comment's id, such as Trekker's `CMT-243`, names with the
    /// comment; for where the comment is what was asked for.
    ///
    /// # Errors
    ///
    /// As [`Here::issue`], or `invalid_value` for an imported comment id more
    /// than one comment holds.
    pub(crate) fn find(&self, reference: &str) -> Result<(Uuid, Option<Uuid>), CliError> {
        let (issue, refused) = self.lookup(reference)?;
        if let Some(issue) = issue {
            return Ok((issue, None));
        }
        match self.store.comments_by_import_id(reference)?.as_slice() {
            [] => Err(refused),
            [(issue, comment)] => Ok((*issue, Some(*comment))),
            several => Err(imported_twice(reference, several.iter().map(|(_, c)| *c))),
        }
    }

    /// The Issue a full UUID, a Key or an imported Issue id names, else the
    /// error to give when nothing else does either.
    fn lookup(&self, reference: &str) -> Result<(Option<Uuid>, CliError), CliError> {
        let not_found = || {
            CliError::new("not_found", format!("no Issue is {reference}"))
                .with("kind", "issue")
                .with("reference", reference)
        };
        let mut refused = not_found();
        if let Ok(id) = Uuid::parse_str(reference) {
            if self.store.issue(id)?.is_some() {
                return Ok((Some(id), refused));
            }
        } else {
            match self.store.issue_by_key(reference) {
                Ok(Some(issue)) => return Ok((Some(issue), refused)),
                Ok(None) => {}
                Err(err @ StoreError::Invalid { .. }) => refused = err.into(),
                Err(err) => return Err(err.into()),
            }
        }
        match self.store.issues_by_import_id(reference)?.as_slice() {
            [] => Ok((None, refused)),
            [only] => Ok((Some(*only), refused)),
            several => Err(imported_twice(reference, several.iter().copied())),
        }
    }
}

fn imported_twice(reference: &str, ids: impl Iterator<Item = Uuid>) -> CliError {
    CliError::new(
        "invalid_value",
        format!("{reference} is imported more than once; name it by its Key"),
    )
    .with("field", "issue")
    .with("matches", ids.map(|id| id.to_string()).collect::<Vec<_>>())
}

/// A repo mapped to a new Workspace with Project `TM`, under a home of its
/// own, for tests that run a command's core in this process.
#[cfg(test)]
pub(crate) fn fixture() -> Place {
    let root = std::env::temp_dir().join(format!("tm-here-{}", Uuid::now_v7()));
    let (home, repo) = (root.join("home"), root.join("repo"));
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(repo.join(".config")).unwrap();
    let id = Uuid::now_v7();
    let (_, mut store) = Workspace::create(&home, id).unwrap();
    store
        .create_project(&Actor::human("jace"), "TM", "Core")
        .unwrap();
    let file = format!("workspace = \"{id}\"\nproject = \"TM\"\n");
    fs::write(repo.join(".config/tm.toml"), file).unwrap();
    // As `TM_ACTOR=jace` with no harness, so git and this process's own
    // environment are never read.
    let jace = |name: &str| (name == "TM_ACTOR").then(|| Ok("jace".to_owned()));
    Place {
        home,
        cwd: repo,
        env: jace,
    }
}
