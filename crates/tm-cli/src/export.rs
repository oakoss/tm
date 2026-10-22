//! `tm export`, `tm import` and `tm rebuild`: the log out, back in, and the
//! projections rebuilt from it (ADR 0013).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use tm_core::Actor;
use tm_store::workspace::{self, Workspace, WorkspaceError};
use tm_store::{RecordOutcome, RecordsImported, Restored, Store, StoreError};
use uuid::Uuid;

use crate::error::{CliError, RECORDS_FIX};
use crate::output::{self, Output};
use crate::{backup, here, identity};

/// Writes the export to stdout. It is JSONL in either mode, so nothing else
/// is printed; a failure before it is written is the usual error.
pub(crate) fn export() -> Result<(), CliError> {
    // Not the Snapshot: a backup must be current, and JSONL has no room to
    // warn that it is not.
    let here = here::open()?;
    let now = now_ms();
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

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

/// Writes the export to tm's home as `exports/<Workspace>/<local date>.jsonl`,
/// replacing that day's, unless it matches the newest one there, then
/// collects old ones (ADR 0015).
pub(crate) fn dated() -> Result<Output, CliError> {
    let here = here::open()?;
    // Held from before the log is read until the rename, so an export that
    // read an older log cannot land after a newer one.
    let exports = backup::Exports::lock(&here.home, here.workspace.id)?;
    let checked = stage(&here, exports.dir())?;
    let today = jiff::Zoned::now().date();
    let newest = newest_export(exports.dir(), today)?;
    let ops = checked.ops;
    // Kept only while it would still restore, header included.
    let unchanged = newest
        .as_ref()
        .and_then(|newest| fs::read(newest).ok())
        .is_some_and(|held| {
            tm_store::read_export(&mut held.as_slice()).ok() == Some((here.workspace.id, ops))
                && after_header(&held) == after_header(&checked.bytes)
        });
    let file = if let (true, Some(newest)) = (unchanged, newest) {
        newest
    } else {
        let target = exports.file(today);
        checked.persist(&target)?;
        target
    };
    let text = if unchanged {
        format!("Nothing changed since {}; wrote nothing\n", file.display())
    } else {
        format!("Wrote {} ({ops} operations)\n", file.display())
    };
    let mut output = Output::new(
        text,
        json!({ "file": file.display().to_string(), "ops": ops, "written": !unchanged }),
    );
    let policy = backup::policy(&here.home);
    output.warnings = exports.collect(today, policy.as_ref().ok());
    output
        .warnings
        .extend(policy.err().as_deref().map(backup::unread));
    Ok(output)
}

/// An export staged beside the backups, and read back with `tm import`'s
/// check, so a backup that could not be restored fails now rather than when
/// it is needed.
pub(crate) struct Checked {
    staged: Staged,
    file: PathBuf,
    bytes: Vec<u8>,
    ops: usize,
}

impl Checked {
    /// Moves the export to `target`, replacing any file there.
    pub(crate) fn persist(self, target: &Path) -> Result<(), CliError> {
        fs::rename(&self.file, target).map_err(|err| io_error(target, &err))?;
        drop(self.staged);
        Ok(())
    }
}

pub(crate) fn stage(here: &here::Here, dir: &Path) -> Result<Checked, CliError> {
    let staged = Staged::new(dir)?;
    let copy = staged.path("export.jsonl");
    let exported = fs::File::create(&copy)
        .map_err(|err| io_error(&copy, &err))
        .and_then(|file| {
            let mut out = io::BufWriter::new(file);
            let exported = here.store.export(&mut out, now_ms())?;
            out.into_inner()
                .map_err(io::IntoInnerError::into_error)
                .and_then(|file| file.sync_all())
                .map_err(|err| io_error(&copy, &err))?;
            Ok(exported)
        })?;
    let bytes = fs::read(&copy).map_err(|err| io_error(&copy, &err))?;
    let (workspace, ops) = tm_store::read_export(&mut bytes.as_slice())?;
    if workspace != here.workspace.id || ops != exported.ops {
        return Err(CliError::internal(
            "export_mismatch",
            format!(
                "{} read back as Workspace {workspace} with {ops} operations, not {} with {}",
                copy.display(),
                here.workspace.id,
                exported.ops
            ),
        ));
    }
    Ok(Checked {
        staged,
        file: copy,
        bytes,
        ops,
    })
}

/// The newest dated export in `dir` up to `today`; one dated later, left by
/// a clock that ran ahead, is passed over.
fn newest_export(dir: &Path, today: jiff::civil::Date) -> Result<Option<PathBuf>, CliError> {
    let entries = fs::read_dir(dir).map_err(|err| io_error(dir, &err))?;
    Ok(entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter_map(|path| {
            let date = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| jiff::civil::Date::strptime("%Y-%m-%d", stem).ok())?;
            (path.extension().is_some_and(|ext| ext == "jsonl") && date <= today)
                .then_some((date, path))
        })
        .max()
        .map(|(_, path)| path))
}

/// An export without its header line, which carries the time it was made.
fn after_header(export: &[u8]) -> &[u8] {
    export
        .iter()
        .position(|&byte| byte == b'\n')
        .map_or(&[][..], |end| &export[end + 1..])
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
                "{} maps this repo to Workspace {}, but the export holds Workspace {workspace}; import its records into this one, or run tm import outside the repo to restore it under its own id",
                found.path.display(),
                found.file.workspace
            ),
        )
        .with("stored", found.file.workspace.to_string())
        .with("given", workspace.to_string())
        .fix(RECORDS_FIX));
    }

    let target = Workspace::at(&home, workspace);
    let exists = fs::symlink_metadata(&target.dir).is_ok();
    if exists && !dry_run {
        return restore_into(&target, &bytes);
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
        tm_store::copy_store(&target.store_path(), &staged.store())?;
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
            let mut output = output(&restored, true, false);
            output.warnings.extend(match open_target(&target) {
                Ok(store) => here::publish(&store, &target),
                Err(err) => Some(here::not_published(&err.message)),
            });
            Ok(output)
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => restore_into(&target, &bytes),
        Err(err) => Err(io_error(&target.dir, &err)),
    }
}

/// Restores into an existing Workspace's store, then publishes its Snapshot.
fn restore_into(target: &Workspace, bytes: &[u8]) -> Result<Output, CliError> {
    let mut store = open_target(target)?;
    let restored = store.restore(&mut &*bytes)?;
    let mut output = output(&restored, false, false);
    output.warnings.extend(here::publish(&store, target));
    Ok(output)
}

/// A tracker whose export `tm import --from` reads.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum Adapter {
    Beads,
}

/// What an import wrote, and what of the file it had no place for.
type Written = (RecordsImported, Option<BTreeMap<String, usize>>);

/// Writes a file's Issue records, or an adapter's conversion of its export,
/// into the Workspace this repo maps (ADR 0013); a dry run copies its store
/// before opening anything, since opening may rebuild it, and writes into the
/// copy.
pub(crate) fn import_records(
    file: &str,
    dry_run: bool,
    adapter: Option<Adapter>,
) -> Result<Output, CliError> {
    let bytes = read(file)?;
    let (home, cwd, found) = here::find()?;
    let identity = identity::resolve(identity::env, || identity::git_email(&found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    let cwd = here::canonical(&cwd)?;
    // Records name their own Project; an adapter's go where `tm create`'s do.
    let project = |store: &Store| match adapter {
        None => Ok(String::new()),
        Some(_) => here::mapped_project(store, &found, &cwd).map(|(_, key)| key.to_owned()),
    };
    let write = |store: &mut Store, project: &str| -> Result<Written, StoreError> {
        match adapter {
            None => store
                .import_records(&mut bytes.as_slice(), &identity.actor)
                .map(|imported| (imported, None)),
            Some(Adapter::Beads) => store
                .import_beads(&mut bytes.as_slice(), project, &identity.actor)
                .map(|beads| (beads.imported, Some(beads.dropped))),
        }
    };
    let mut warnings = identity.warnings.clone();
    let ((imported, dropped), undo) = if dry_run {
        let target = workspace::locate(&home, &found)?;
        let staged = Staged::new(&std::env::temp_dir())?;
        tm_store::copy_store(&target.store_path(), &staged.store())?;
        let mut copy = Store::open_existing(&staged.store(), target.id)?;
        // The copy's batches, on success or refusal, are gone with it.
        let key = project(&copy)?;
        let (mut imported, dropped) = write(&mut copy, &key).map_err(|err| match err {
            StoreError::RecordRefused {
                line,
                record,
                cause,
                ..
            } => StoreError::RecordRefused {
                line,
                record,
                cause,
                written: Vec::new(),
            },
            other => other,
        })?;
        for record in &mut imported.records {
            record.batches.clear();
        }
        ((imported, dropped), BTreeMap::new())
    } else {
        let mut here = here::open()?;
        warnings.append(&mut here.warnings);
        let key = project(&here.store).map_err(|err| err.warned(warnings.clone()))?;
        let written = write(&mut here.store, &key);
        warnings.extend(here.after_write());
        let (imported, dropped) =
            written.map_err(|err| CliError::from(err).warned(warnings.clone()))?;
        let undo = undo_states(&mut here.store, &identity.actor, &imported);
        ((imported, dropped), undo)
    };
    let mut output = records_output(
        &imported,
        dropped.as_ref(),
        dry_run,
        &undo,
        identity.actor.name(),
    );
    output.warnings.extend(warnings);
    Ok(output)
}

/// Whether `runner` can undo each batch an import wrote to `store`. Tried as
/// the runner, since an undo writes as whoever runs it and only a comment's
/// author may retract it.
pub(crate) fn undo_states(
    store: &mut Store,
    runner: &Actor,
    imported: &RecordsImported,
) -> BTreeMap<Uuid, Undo> {
    let mut undo = BTreeMap::new();
    for batch in imported.records.iter().flat_map(|record| &record.batches) {
        let state = match store.undo_plan(runner, *batch) {
            Ok(_) => Undo::Can,
            Err(
                err @ (StoreError::Busy(_)
                | StoreError::Sqlite(_)
                | StoreError::Clock(_)
                | StoreError::Corrupt(_)),
            ) => Undo::Unknown(err.to_string()),
            Err(_) => Undo::Cannot,
        };
        undo.insert(*batch, state);
    }
    undo
}

/// Names what an adapter read and tm has no place for, with how often.
fn not_kept(output: &mut Output, dropped: &BTreeMap<String, usize>) {
    if !dropped.is_empty() {
        let counts: Vec<String> = dropped
            .iter()
            .map(|(what, times)| format!("{} ({times})", output::printable(what)))
            .collect();
        let _ = writeln!(output.text, "not kept: {}", counts.join(", "));
    }
    output.data["dropped"] = json!(dropped);
}

/// Whether the runner could undo a batch an import wrote.
pub(crate) enum Undo {
    Can,
    Cannot,
    /// The check itself failed, saying nothing about the batch.
    Unknown(String),
}

/// A records import's report; `dropped` is what an adapter read and could
/// not keep.
pub(crate) fn records_output(
    imported: &RecordsImported,
    dropped: Option<&BTreeMap<String, usize>>,
    dry_run: bool,
    undo: &BTreeMap<Uuid, Undo>,
    runner: &str,
) -> Output {
    let count =
        |pick: fn(&RecordOutcome) -> bool| imported.records.iter().filter(|r| pick(r)).count();
    let created = count(|record| record.created);
    let updated = count(|record| {
        !record.created && (!record.written.is_empty() || !record.removed.is_empty())
    });
    let unchanged = imported.records.len() - created - updated;
    let mut text = format!(
        "{} {} record{}: {created} created, {updated} updated, {unchanged} unchanged\n",
        if dry_run { "would import" } else { "imported" },
        imported.records.len(),
        if imported.records.len() == 1 { "" } else { "s" },
    );
    // Writing to a String cannot fail; what a file names is escaped. A bare
    // undo skips imports, so each record's batches are named.
    for record in &imported.records {
        let source = output::printable(&record.source);
        for batch in &record.batches {
            let _ = match undo.get(batch) {
                Some(Undo::Can) => writeln!(text, "{source}: undo with tm undo {batch}"),
                Some(Undo::Cannot) => writeln!(
                    text,
                    "{source}: batch {batch}, which {} cannot undo as things stand",
                    output::printable(runner)
                ),
                Some(Undo::Unknown(reason)) => writeln!(
                    text,
                    "{source}: batch {batch}; whether it can be undone was not checked: {}",
                    output::printable(reason)
                ),
                None => writeln!(text, "{source}: batch {batch}"),
            };
        }
    }
    let create = if dry_run { "would create" } else { "created" };
    for key in &imported.projects {
        let _ = writeln!(text, "{create} Project {}", output::printable(key));
    }
    for record in &imported.records {
        let source = output::printable(&record.source);
        for conflict in &record.conflicts {
            let _ = writeln!(
                text,
                "conflict: {source} {}: kept tm's {}, the source has {}",
                output::printable(&conflict.field),
                output::printable(&conflict.tm.to_string()),
                output::printable(&conflict.source.to_string()),
            );
        }
        for comment in &record.changed_comments {
            let _ = writeln!(
                text,
                "changed at the source, not rewritten: {source} comment {}",
                output::printable(comment)
            );
        }
    }
    let data = json!({
        "projects": imported.projects,
        "records": imported.records,
        "created": created,
        "updated": updated,
        "unchanged": unchanged,
        "dry_run": dry_run,
    });
    let mut output = Output::new(text, data);
    if let Some(dropped) = dropped {
        not_kept(&mut output, dropped);
    }
    output.warnings.push(
        "a record import keeps each Issue's current state and comments, not its history, operation ids or retracted comments".into(),
    );
    output
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

pub(crate) fn io_error(path: &Path, err: &io::Error) -> CliError {
    CliError::new("io", format!("{}: {err}", path.display()))
        .with("path", path.display().to_string())
}

/// An owner-only directory, removed with whatever is left in it.
pub(crate) struct Staged {
    dir: PathBuf,
}

impl Staged {
    pub(crate) fn new(parent: &Path) -> Result<Self, CliError> {
        use std::os::unix::fs::DirBuilderExt as _;
        let dir = parent.join(format!(".tm-import-{}", Uuid::now_v7()));
        fs::create_dir_all(parent).map_err(|err| io_error(parent, &err))?;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|err| io_error(&dir, &err))?;
        Ok(Self { dir })
    }

    pub(crate) fn store(&self) -> PathBuf {
        self.path("store.db")
    }

    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
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
    output.warnings = std::mem::take(&mut here.warnings);
    // A rebuild rewrites the projections but leaves the log as it was.
    output
        .warnings
        .extend(here::publish(&here.store, &here.workspace));
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
    fn a_records_import_names_every_batch_it_wrote() {
        let (first, second) = (Uuid::now_v7(), Uuid::now_v7());
        let imported = RecordsImported {
            projects: Vec::new(),
            records: vec![RecordOutcome {
                line: 1,
                source: "beads:a".into(),
                issue: Uuid::now_v7(),
                created: true,
                batches: vec![first, second],
                written: vec!["title".into()],
                removed: Vec::new(),
                conflicts: Vec::new(),
                changed_comments: Vec::new(),
            }],
        };
        let third = Uuid::now_v7();
        let mut imported = imported;
        imported.records[0].batches.push(third);
        let undo = BTreeMap::from([
            (first, Undo::Can),
            (second, Undo::Cannot),
            (third, Undo::Unknown("the store is busy".into())),
        ]);
        let text = records_output(&imported, None, false, &undo, "jace").text;
        for line in [
            format!("beads:a: undo with tm undo {first}"),
            format!("beads:a: batch {second}, which jace cannot undo as things stand"),
            format!(
                "beads:a: batch {third}; whether it can be undone was not checked: the store is busy"
            ),
        ] {
            assert!(text.contains(&line), "{text}");
        }
    }

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
