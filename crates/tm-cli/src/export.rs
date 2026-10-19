//! `tm export`, `tm import` and `tm rebuild`: the log out, back in, and the
//! projections rebuilt from it (ADR 0013).

use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use tm_store::workspace::{self, Workspace, WorkspaceError};
use tm_store::{Restored, Store, StoreError};
use uuid::Uuid;

use crate::error::CliError;
use crate::here;
use crate::output::{self, Output};

/// Writes the export to stdout. It is JSONL in either mode, so nothing else
/// is printed; a failure before it is written is the usual error.
pub(crate) fn export() -> Result<(), CliError> {
    let here = here::open()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0);
    let stdout_error = |err: io::Error| CliError::new("io", format!("stdout: {err}"));
    let mut out = io::BufWriter::new(output::stdout().map_err(stdout_error)?);
    let written = match here.store.export(&mut out, now) {
        Err(StoreError::Io(err)) => Err(err),
        other => {
            other?;
            out.flush()
        }
    };
    output::quiet_on_broken_pipe(written).map_err(stdout_error)
}

/// Restores an export into its Workspace on this machine, creating it when
/// missing. A new one is restored aside and renamed into place, so a failure
/// removes only what this import wrote; a dry run restores into a copy.
pub(crate) fn import(file: &str, dry_run: bool) -> Result<Output, CliError> {
    let bytes = read(file)?;
    let (workspace, _) = tm_store::read_export(&mut bytes.as_slice())?;
    let home = workspace::home(std::env::var_os("TM_HOME"), std::env::home_dir())?;
    let cwd = std::env::current_dir()
        .map_err(|err| CliError::new("io", format!("working directory: {err}")))?;
    if let Some(found) = workspace::discover(&cwd)?
        && found.file.workspace != workspace
    {
        return Err(CliError::new(
            "workspace_mismatch",
            format!(
                "{} maps this repo to Workspace {}, but the export holds Workspace {workspace}; run tm import outside the repo to restore it under its own id",
                found.path.display(),
                found.file.workspace
            ),
        )
        .with("stored", found.file.workspace.to_string())
        .with("given", workspace.to_string()));
    }

    let target = Workspace::at(&home, workspace);
    let exists = fs::symlink_metadata(&target.dir).is_ok();
    if exists && !dry_run {
        let restored = open_target(&target)?.restore(&mut bytes.as_slice())?;
        return Ok(output(&restored, false, false));
    }
    // Beside the Workspaces, so the rename stays on one filesystem; a dry
    // run renames nothing and leaves tm's home untouched.
    let staged = Staged::new(&if dry_run {
        std::env::temp_dir()
    } else {
        home.join("workspaces")
    })?;
    let mut store = if exists {
        has_store(&target)?;
        tm_store::snapshot(&target.store_path(), &staged.store())?;
        Store::open_existing(&staged.store(), workspace)?
    } else {
        let mut store = Store::open(&staged.store())?;
        store.set_workspace_id(workspace)?;
        store
    };
    let restored = store.restore(&mut bytes.as_slice())?;
    drop(store);
    if dry_run {
        return Ok(output(&restored, !exists, true));
    }
    // Making the directory claims the Workspace: it fails if another import
    // made it first, and until the store moves in, every other command finds
    // it incomplete and writes nothing.
    match fs::create_dir(&target.dir) {
        Ok(()) => {
            if let Err(err) = publish(&staged.store(), &target) {
                let _ = fs::remove_dir(&target.dir);
                return Err(io_error(&target.dir, &err));
            }
            Ok(output(&restored, true, false))
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            let restored = open_target(&target)?.restore(&mut bytes.as_slice())?;
            Ok(output(&restored, false, false))
        }
        Err(err) => Err(io_error(&target.dir, &err)),
    }
}

/// An existing Workspace's store; a directory without one, which may be
/// another import still moving in, is refused rather than opened.
fn open_target(target: &Workspace) -> Result<Store, CliError> {
    has_store(target)?;
    Ok(Store::open_existing(&target.store_path(), target.id)?)
}

fn has_store(target: &Workspace) -> Result<(), CliError> {
    if fs::symlink_metadata(target.store_path()).is_err() {
        return Err(WorkspaceError::IncompleteWorkspace {
            dir: target.dir.clone(),
        }
        .into());
    }
    Ok(())
}

/// The WAL beside a store, which holds committed writes until a checkpoint.
fn wal(store: &Path) -> PathBuf {
    let mut name = store.as_os_str().to_owned();
    name.push("-wal");
    PathBuf::from(name)
}

/// Moves a closed store into the Workspace it claimed: the WAL first, since
/// a checkpoint that failed on close leaves committed writes in it, then the
/// store, whose arrival makes the Workspace complete.
/// If the store cannot follow, the WAL goes back, so the claimed directory is
/// empty again for the caller to remove.
fn publish(staged: &Path, target: &Workspace) -> io::Result<()> {
    let moved_wal = match fs::rename(wal(staged), wal(&target.store_path())) {
        Ok(()) => true,
        Err(err) if err.kind() == io::ErrorKind::NotFound => false,
        Err(err) => return Err(err),
    };
    fs::rename(staged, target.store_path()).inspect_err(|_| {
        if moved_wal {
            let _ = fs::rename(wal(&target.store_path()), wal(staged));
        }
    })
}

fn io_error(path: &Path, err: &io::Error) -> CliError {
    CliError::new("io", format!("{}: {err}", path.display()))
        .with("path", path.display().to_string())
}

/// An owner-only directory, removed with whatever is left in it.
struct Staged {
    dir: PathBuf,
}

impl Staged {
    fn new(parent: &Path) -> Result<Self, CliError> {
        use std::os::unix::fs::DirBuilderExt as _;
        let dir = parent.join(format!(".tm-import-{}", Uuid::now_v7()));
        fs::create_dir_all(parent).map_err(|err| io_error(parent, &err))?;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|err| io_error(&dir, &err))?;
        Ok(Self { dir })
    }

    fn store(&self) -> PathBuf {
        self.dir.join("store.db")
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Rebuilds the projections from the log.
pub(crate) fn rebuild() -> Result<Output, CliError> {
    let mut here = here::open()?;
    let ops: i64 = here
        .store
        .connection()
        .query_row("SELECT count(*) FROM ops", [], |row| row.get(0))
        .map_err(tm_store::StoreError::from)?;
    let unapplied = here.store.rebuild()?;
    let text = format!(
        "rebuilt from {ops} operation{}; {unapplied} did not apply\n",
        if ops == 1 { "" } else { "s" }
    );
    let mut output = Output::new(text, json!({ "ops": ops, "unapplied": unapplied }));
    output.warnings = here.warnings;
    Ok(output)
}

fn read(file: &str) -> Result<Vec<u8>, CliError> {
    let mut bytes = Vec::new();
    let read = if file == "-" {
        io::stdin().read_to_end(&mut bytes).map(|_| ())
    } else {
        fs::File::open(file).and_then(|mut f| f.read_to_end(&mut bytes).map(|_| ()))
    };
    read.map_err(|err| {
        CliError::new("io", format!("{file}: {err}")).with("path", file.to_owned())
    })?;
    Ok(bytes)
}

fn output(restored: &Restored, created: bool, dry_run: bool) -> Output {
    let Restored { workspace, ops, .. } = *restored;
    let ops_word = if ops == 1 { "operation" } else { "operations" };
    let text = match (restored.already_held, dry_run, created) {
        (true, _, _) => format!("Workspace {workspace} already holds this export\n"),
        (false, true, true) => {
            format!("would create Workspace {workspace} and restore {ops} {ops_word} into it\n")
        }
        (false, true, false) => {
            format!("would restore {ops} {ops_word} into Workspace {workspace}\n")
        }
        (false, false, _) => format!("restored {ops} {ops_word} into Workspace {workspace}\n"),
    };
    let data = json!({
        "workspace": workspace,
        "ops": ops,
        "already_held": restored.already_held,
        "created": created,
        "unapplied": restored.unapplied,
        "dry_run": dry_run,
    });
    let mut output = Output::new(text, data);
    if !restored.already_held && !dry_run {
        output.warnings.push(format!(
            "Workspace {workspace} now lives on this machine too; until sync, stop using tm for it on the machine this export came from, or both will give out the same next Keys"
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_published_store_takes_its_wal_with_it() {
        let root = std::env::temp_dir().join(format!("tm-publish-{}", Uuid::now_v7()));
        let staged = root.join("staged").join("store.db");
        fs::create_dir_all(staged.parent().unwrap()).unwrap();
        fs::write(&staged, "store").unwrap();
        fs::write(wal(&staged), "committed frames").unwrap();
        let target = Workspace::at(&root, Uuid::now_v7());
        fs::create_dir_all(&target.dir).unwrap();

        publish(&staged, &target).unwrap();
        let moved = fs::read_to_string(wal(&target.store_path()));
        let _ = fs::remove_dir_all(&root);
        assert_eq!(moved.unwrap(), "committed frames");
    }

    #[test]
    fn a_store_that_cannot_follow_its_wal_takes_the_wal_back() {
        let root = std::env::temp_dir().join(format!("tm-publish-{}", Uuid::now_v7()));
        let staged = root.join("staged").join("store.db");
        fs::create_dir_all(staged.parent().unwrap()).unwrap();
        fs::write(&staged, "store").unwrap();
        fs::write(wal(&staged), "committed frames").unwrap();
        let target = Workspace::at(&root, Uuid::now_v7());
        fs::create_dir_all(target.store_path().join("occupied")).unwrap();

        let failed = publish(&staged, &target).is_err();
        let back = fs::read_to_string(wal(&staged));
        let left = fs::symlink_metadata(wal(&target.store_path())).is_ok();
        let _ = fs::remove_dir_all(&root);
        assert!(failed);
        assert_eq!(back.unwrap(), "committed frames");
        assert!(!left, "nothing of this import's left in the Workspace");
    }
}
