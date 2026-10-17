//! Workspaces on this machine and the repo file that maps a repo to one
//! (ADR 0003, ADR 0008).

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Store, StoreError};

/// Where a repo file may live, relative to the directory that holds it.
pub const REPO_FILES: [&str; 2] = [".tm.toml", ".config/tm.toml"];

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("cannot find the home directory; set TM_HOME")]
    NoHome,
    #[error("{name} must be an absolute path, not {path:?}")]
    RelativeHome { name: &'static str, path: PathBuf },
    #[error("{} and {} both map this repo; keep one", first.display(), second.display())]
    TwoRepoFiles { first: PathBuf, second: PathBuf },
    #[error("{}: {reason}", path.display())]
    InvalidRepoFile { path: PathBuf, reason: String },
    #[error(
        "{} names Workspace {id}, but there is none at {}; check TM_HOME, or wait for sync to bring it",
        file.display(),
        dir.display()
    )]
    UnknownWorkspace {
        id: Uuid,
        file: PathBuf,
        dir: PathBuf,
    },
    #[error(
        "{} holds no tm store; restore store.db from a backup, or remove the directory and the repo file naming it and run tm init again",
        dir.display()
    )]
    IncompleteWorkspace { dir: PathBuf },
    #[error("Workspace {id} already exists at {}", dir.display())]
    WorkspaceExists { id: Uuid, dir: PathBuf },
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("{}: {source}", path.display())]
    OpenStore {
        path: PathBuf,
        source: Box<StoreError>,
    },
    #[error(transparent)]
    Store(#[from] StoreError),
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> WorkspaceError + '_ {
    move |source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// This machine's tm directory: `TM_HOME` when set, else `~/.tm`. Pass
/// `std::env::var_os("TM_HOME")` and `std::env::home_dir()`.
///
/// # Errors
///
/// [`WorkspaceError::RelativeHome`] when the directory found is relative, or
/// [`WorkspaceError::NoHome`] when neither is known.
pub fn home(
    tm_home: Option<OsString>,
    user_home: Option<PathBuf>,
) -> Result<PathBuf, WorkspaceError> {
    let (name, path) = if let Some(value) = tm_home.filter(|value| !value.is_empty()) {
        ("TM_HOME", PathBuf::from(value))
    } else {
        let home = user_home
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or(WorkspaceError::NoHome)?;
        ("the home directory", home.join(".tm"))
    };
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(WorkspaceError::RelativeHome { name, path })
    }
}

/// A Workspace's directory under the tm home, named by its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub id: Uuid,
    pub dir: PathBuf,
}

impl Workspace {
    #[must_use]
    pub fn at(home: &Path, id: Uuid) -> Self {
        Self {
            id,
            dir: home.join("workspaces").join(id.hyphenated().to_string()),
        }
    }

    #[must_use]
    pub fn store_path(&self) -> PathBuf {
        self.dir.join("store.db")
    }

    /// Creates the Workspace's directory and store. The directory must not
    /// exist yet, so two creates never share one; if the store cannot be
    /// made, the directory is removed so the same id can be created again.
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::WorkspaceExists`], [`WorkspaceError::Io`] when the
    /// directory cannot be made, or any error opening the store.
    pub fn create(home: &Path, id: Uuid) -> Result<(Self, Store), WorkspaceError> {
        let workspace = Self::at(home, id);
        let parent = home.join("workspaces");
        fs::create_dir_all(&parent).map_err(io_error(&parent))?;
        if let Err(err) = fs::create_dir(&workspace.dir) {
            return Err(if err.kind() == io::ErrorKind::AlreadyExists {
                WorkspaceError::WorkspaceExists {
                    id,
                    dir: workspace.dir,
                }
            } else {
                io_error(&workspace.dir)(err)
            });
        }
        let made = Store::open(&workspace.store_path()).and_then(|mut store| {
            store.set_workspace_id(id)?;
            Ok(store)
        });
        match made {
            Ok(store) => Ok((workspace, store)),
            Err(err) => {
                let _ = fs::remove_dir_all(&workspace.dir);
                Err(err.into())
            }
        }
    }
}

/// The contents of a repo file. It names a Workspace and Projects only, so
/// it holds nothing that differs between machines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoFile {
    pub workspace: Uuid,
    pub project: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<PathProject>,
}

/// A monorepo directory mapped to its own Project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathProject {
    pub prefix: String,
    pub project: String,
}

impl RepoFile {
    /// The file's text, exactly as `tm init` shows and writes it.
    ///
    /// # Panics
    ///
    /// Never: every field is a string, a UUID or a table of strings.
    #[must_use]
    pub fn render(&self) -> String {
        toml::to_string(self).expect("a repo file always serializes")
    }

    /// Reads a repo file, uppercasing its Project keys. A prefix must be a
    /// plain relative path such as `crates/tm-tui`, mapped once.
    ///
    /// # Errors
    ///
    /// A description of the first problem in `text`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut file: Self = toml::from_str(text).map_err(|err| err.to_string())?;
        for key in std::iter::once(&mut file.project)
            .chain(file.paths.iter_mut().map(|mapping| &mut mapping.project))
        {
            *key = key.to_ascii_uppercase();
            if !tm_core::project::is_key(key) {
                return Err(format!(
                    "project {key:?} is not a Project key: a letter, then 1 to 9 letters or digits"
                ));
            }
        }
        let mut seen = HashSet::new();
        for mapping in &file.paths {
            let prefix = Path::new(&mapping.prefix);
            let plain = prefix.components().next().is_some()
                && prefix
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)));
            if !plain {
                return Err(format!(
                    "prefix {:?} must be a relative path inside the repo, such as \"crates/tm-tui\"",
                    mapping.prefix
                ));
            }
            if !seen.insert(prefix.components().collect::<PathBuf>()) {
                return Err(format!("prefix {:?} is mapped twice", mapping.prefix));
            }
        }
        Ok(file)
    }

    /// The Project key for `relative`, a path inside the directory holding
    /// the file: the longest prefix covering it in whole segments, else the
    /// file's own Project.
    #[must_use]
    pub fn project_for(&self, relative: &Path) -> &str {
        self.paths
            .iter()
            .filter(|mapping| relative.starts_with(Path::new(&mapping.prefix)))
            .max_by_key(|mapping| Path::new(&mapping.prefix).components().count())
            .map_or(&self.project, |mapping| &mapping.project)
    }
}

/// A repo file found by [`discover`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub dir: PathBuf,
    pub file: RepoFile,
}

/// The repo holding `start`: the nearest ancestor of its real path, `start`
/// included, with a `.git` entry (a directory, or a file in a worktree or
/// submodule).
///
/// # Errors
///
/// [`WorkspaceError::Io`] when `start` cannot be resolved.
pub fn repo_root(start: &Path) -> Result<Option<PathBuf>, WorkspaceError> {
    let start = fs::canonicalize(start).map_err(io_error(start))?;
    for dir in start.ancestors() {
        if exists(&dir.join(".git"))? {
            return Ok(Some(dir.to_path_buf()));
        }
    }
    Ok(None)
}

/// Whether anything, even a dangling symlink, is at `path`. A file where a
/// parent directory would be, such as a file named `.config`, means nothing is.
fn exists(path: &Path) -> Result<bool, WorkspaceError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(false)
        }
        Err(err) => Err(io_error(path)(err)),
    }
}

/// The repo file that applies at `start`: the nearest one walking up its real
/// path, never past the repo root, so a repo nested in a dotfiles `$HOME`
/// never reads `$HOME`'s file. Outside a repo there is none.
///
/// # Errors
///
/// [`WorkspaceError::TwoRepoFiles`] when one directory holds both names,
/// [`WorkspaceError::InvalidRepoFile`] when one is not a readable file of
/// the right shape, or [`WorkspaceError::Io`] when a place one could be
/// cannot be checked.
pub fn discover(start: &Path) -> Result<Option<Found>, WorkspaceError> {
    let Some(root) = repo_root(start)? else {
        return Ok(None);
    };
    let start = fs::canonicalize(start).map_err(io_error(start))?;
    for dir in start.ancestors() {
        let mut present = Vec::new();
        for name in REPO_FILES {
            let path = dir.join(name);
            if exists(&path)? {
                present.push(path);
            }
        }
        match present.as_slice() {
            [] => {}
            [path] => {
                return read_repo_file(path).map(|file| {
                    Some(Found {
                        path: path.clone(),
                        dir: dir.to_path_buf(),
                        file,
                    })
                });
            }
            [first, second, ..] => {
                return Err(WorkspaceError::TwoRepoFiles {
                    first: first.clone(),
                    second: second.clone(),
                });
            }
        }
        if dir == root {
            break;
        }
    }
    Ok(None)
}

fn read_repo_file(path: &Path) -> Result<RepoFile, WorkspaceError> {
    let invalid = |reason: String| WorkspaceError::InvalidRepoFile {
        path: path.to_path_buf(),
        reason,
    };
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(invalid("is a symlink to a missing file".into()));
        }
        Err(err) => return Err(io_error(path)(err)),
    };
    if !metadata.is_file() {
        return Err(invalid("is not a file".into()));
    }
    let text = fs::read_to_string(path).map_err(io_error(path))?;
    RepoFile::parse(&text).map_err(invalid)
}

/// Opens the Workspace a repo file names, which must exist on this machine,
/// through [`Store::open_existing`], so nothing is created.
///
/// # Errors
///
/// [`WorkspaceError::UnknownWorkspace`] when it does not exist,
/// [`WorkspaceError::IncompleteWorkspace`] when its directory holds no tm
/// store, [`WorkspaceError::Io`] when a path is the wrong kind or cannot be
/// checked, or [`WorkspaceError::OpenStore`] naming the store for any other
/// error opening it, [`StoreError::WorkspaceMismatch`] included.
pub fn workspace_for(home: &Path, found: &Found) -> Result<(Workspace, Store), WorkspaceError> {
    let workspace = Workspace::at(home, found.file.workspace);
    let store_path = workspace.store_path();
    let incomplete = || WorkspaceError::IncompleteWorkspace {
        dir: workspace.dir.clone(),
    };
    match fs::metadata(&workspace.dir) {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(if exists(&workspace.dir)? {
                io_error(&workspace.dir)(io::Error::other("is a symlink to a missing directory"))
            } else {
                WorkspaceError::UnknownWorkspace {
                    id: found.file.workspace,
                    file: found.path.clone(),
                    dir: workspace.dir,
                }
            });
        }
        Err(err) => return Err(io_error(&workspace.dir)(err)),
    }
    match fs::metadata(&store_path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(io_error(&store_path)(io::Error::other("is not a file"))),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(if exists(&store_path)? {
                io_error(&store_path)(io::Error::other("is a symlink to a missing file"))
            } else {
                incomplete()
            });
        }
        Err(err) => return Err(io_error(&store_path)(err)),
    }
    match Store::open_existing(&store_path, workspace.id) {
        Ok(store) => Ok((workspace, store)),
        Err(StoreError::NotAStore(_)) => Err(incomplete()),
        Err(source) => Err(WorkspaceError::OpenStore {
            path: store_path,
            source: Box::new(source),
        }),
    }
}
