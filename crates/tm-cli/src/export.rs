//! `tm export`, `tm import` and `tm rebuild`: the log out, back in, and the
//! projections rebuilt from it (ADR 0013).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tm_core::Actor;
use tm_store::workspace::{self, Workspace, WorkspaceError};
use tm_store::{Conflict, RecordOutcome, RecordsImported, Restored, Store, StoreError};
use uuid::Uuid;

use crate::error::{CliError, RECORDS_FIX};
use crate::output::{self, Output};
use crate::{here, identity};

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
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn ops(&self) -> usize {
        self.ops
    }

    /// Moves the export to `target`, replacing any file there.
    pub(crate) fn persist(self, target: &Path) -> Result<(), CliError> {
        fs::rename(&self.file, target).map_err(|err| CliError::io(target, &err))?;
        drop(self.staged);
        Ok(())
    }
}

pub(crate) fn stage(here: &here::Here, dir: &Path) -> Result<Checked, CliError> {
    let staged = Staged::new(dir)?;
    let copy = staged.path("export.jsonl");
    let exported = fs::File::create(&copy)
        .map_err(|err| CliError::io(&copy, &err))
        .and_then(|file| {
            let mut out = io::BufWriter::new(file);
            let exported = here.store.export(&mut out, now_ms())?;
            out.into_inner()
                .map_err(io::IntoInnerError::into_error)
                .and_then(|file| file.sync_all())
                .map_err(|err| CliError::io(&copy, &err))?;
            Ok(exported)
        })?;
    let bytes = fs::read(&copy).map_err(|err| CliError::io(&copy, &err))?;
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
                return Err(CliError::io(&target.dir, &err));
            }
            let mut output = output(&restored, true, false);
            output.warnings.extend(match open_target(&target) {
                Ok(store) => here::publish(&store, &target),
                Err(err) => Some(here::not_published(&err.message)),
            });
            Ok(output)
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => restore_into(&target, &bytes),
        Err(err) => Err(CliError::io(&target.dir, &err)),
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
        for list in &mut imported.shortlists {
            list.batches.clear();
        }
        for memory in &mut imported.memories {
            memory.batches.clear();
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
    let records = imported.records.iter().flat_map(|record| &record.batches);
    let lists = imported.shortlists.iter().flat_map(|list| &list.batches);
    let memories = imported.memories.iter().flat_map(|memory| &memory.batches);
    for batch in records.chain(lists).chain(memories) {
        if undo.contains_key(batch) {
            continue;
        }
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
    // A bare undo skips imports, so each batch is named; a list's batch a
    // record already names, as a place batch is, is not named again.
    let mut named = BTreeSet::new();
    for record in &imported.records {
        for batch in &record.batches {
            named.insert(*batch);
            batch_line(&mut text, &record.source, *batch, undo, runner);
        }
    }
    for list in &imported.shortlists {
        for batch in &list.batches {
            if named.insert(*batch) {
                batch_line(&mut text, &list.source, *batch, undo, runner);
            }
        }
    }
    for memory in &imported.memories {
        for batch in &memory.batches {
            batch_line(&mut text, &memory.source, *batch, undo, runner);
        }
    }
    let create = if dry_run { "would create" } else { "created" };
    for key in &imported.projects {
        let _ = writeln!(text, "{create} Project {}", output::printable(key));
    }
    lists_text(&mut text, imported, dry_run);
    memories_text(&mut text, imported, dry_run);
    for record in &imported.records {
        let source = output::printable(&record.source);
        for conflict in &record.conflicts {
            conflict_line(&mut text, &source, conflict, dry_run);
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
        "shortlists": imported.shortlists,
        "memories": imported.memories,
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

/// A field both tm and the source changed, or one tm's value stands for by
/// `reason`. A place shows its list's name, else its id.
fn conflict_line(text: &mut String, subject: &str, conflict: &Conflict, dry_run: bool) {
    let field = output::printable(&conflict.field);
    let shown = |value: &Value| {
        if value.is_null() && conflict.field == "shortlist" {
            return "no list".to_owned();
        }
        let place = value.get("name").filter(|name| !name.is_null());
        let value = place.or_else(|| value.get("id")).unwrap_or(value);
        output::printable(&value.to_string())
    };
    let (tm, source) = (shown(&conflict.tm), shown(&conflict.source));
    // A reason means tm refused the source's value, not that both changed it.
    let _ = match (conflict.reason, dry_run) {
        (Some(reason), false) if conflict.tm.is_null() => {
            writeln!(text, "{subject} {field}: not placed on {source} ({reason})")
        }
        (Some(reason), true) if conflict.tm.is_null() => writeln!(
            text,
            "{subject} {field}: would not be placed on {source} ({reason})"
        ),
        (Some(reason), false) => writeln!(
            text,
            "{subject} {field}: not placed on {source} ({reason}); stays on {tm}"
        ),
        (Some(reason), true) => writeln!(
            text,
            "{subject} {field}: would not be placed on {source} ({reason}); would stay on {tm}"
        ),
        (None, false) => writeln!(
            text,
            "conflict: {subject} {field}: kept tm's {tm}, the source has {source}"
        ),
        (None, true) => writeln!(
            text,
            "conflict: {subject} {field}: would keep tm's {tm}, the source has {source}"
        ),
    };
}

/// Names a batch an import wrote for `source`, with whether `runner` can
/// undo it. Writing to a String cannot fail; what a file names is escaped.
fn batch_line(
    text: &mut String,
    source: &str,
    batch: Uuid,
    undo: &BTreeMap<Uuid, Undo>,
    runner: &str,
) {
    let source = output::printable(source);
    let _ = match undo.get(&batch) {
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

/// Each memory a project line holds that the import created or changed, or
/// left as tm holds it.
fn memories_text(text: &mut String, imported: &RecordsImported, dry_run: bool) {
    for memory in &imported.memories {
        let source = output::printable(&memory.source);
        let key = output::printable(memory.key.as_deref().unwrap_or("(no key)"));
        if memory.forgotten {
            let _ = writeln!(
                text,
                "memory {key} ({source}) is forgotten here; left as it is"
            );
            continue;
        }
        let how = match (memory.created, memory.written.is_empty(), dry_run) {
            (true, _, true) => "would create",
            (true, _, false) => "created",
            (false, false, true) => "would update",
            (false, false, false) => "updated",
            (false, true, _) => "",
        };
        if !how.is_empty() {
            let _ = writeln!(text, "{how} memory {key} ({source})");
        }
        for conflict in &memory.conflicts {
            conflict_line(text, &source, conflict, dry_run);
        }
    }
}

/// Each Short-list the file names: how it was found, what it gained and
/// lost, its own fields changed, its conflicts, and the members `tm next`
/// passes over, by Key, else by their record, else by id.
fn lists_text(text: &mut String, imported: &RecordsImported, dry_run: bool) {
    let by_issue: BTreeMap<Uuid, &str> = imported
        .records
        .iter()
        .map(|record| (record.issue, record.source.as_str()))
        .collect();
    for list in &imported.shortlists {
        let source = output::printable(&list.source);
        let name = output::printable(list.name.as_deref().unwrap_or("(unnamed)"));
        if list.deleted {
            let placed = if dry_run { "would be" } else { "was" };
            let _ = writeln!(
                text,
                "Short-list {name} ({source}) is deleted here; nothing {placed} placed on it"
            );
        } else {
            let how = match (list.created, list.by_name, dry_run) {
                (true, _, true) => "would create",
                (true, _, false) => "created",
                (false, true, true) => "would match by name",
                (false, true, false) => "matched by name",
                (false, false, _) => "found",
            };
            let over = if u64::try_from(list.members).is_ok_and(|count| count > list.size) {
                ", over size"
            } else {
                ""
            };
            let (placed, off, members, size) = (
                list.placed.len(),
                list.taken_off.len(),
                list.members,
                list.size,
            );
            let _ = if dry_run {
                writeln!(
                    text,
                    "{how} Short-list {name} ({source}): would place {placed}, take off {off}, hold {members} of {size}{over}"
                )
            } else {
                writeln!(
                    text,
                    "{how} Short-list {name} ({source}): {placed} placed, {off} taken off, {members} of {size}{over}"
                )
            };
            let set: Vec<String> = list
                .written
                .iter()
                .filter(|field| *field != "created")
                .map(|field| output::printable(field))
                .collect();
            let removed: Vec<String> = list.removed.iter().map(|f| output::printable(f)).collect();
            let (write, remove) = if dry_run {
                ("would set", "would remove")
            } else {
                ("set", "removed")
            };
            for (verb, fields) in [(write, set), (remove, removed)] {
                if !fields.is_empty() {
                    let _ = writeln!(text, "Short-list {name}: {verb} {}", fields.join(", "));
                }
            }
        }
        let subject = format!("Short-list {name} ({source})");
        for conflict in &list.conflicts {
            conflict_line(text, &subject, conflict, dry_run);
        }
        for member in &list.passed_over {
            let who = match (&member.key, by_issue.get(&member.issue)) {
                (Some(key), _) => output::printable(key),
                (None, Some(source)) => output::printable(source),
                (None, None) => member.issue.to_string(),
            };
            let reasons: Vec<&str> = member
                .reasons
                .iter()
                .map(|r| crate::shortlist::reason(*r))
                .collect();
            let _ = writeln!(
                text,
                "Short-list {name}: tm next passes over {who} [{}]",
                reasons.join(", ")
            );
        }
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

/// An owner-only directory, removed with whatever is left in it.
pub(crate) struct Staged {
    dir: PathBuf,
}

impl Staged {
    pub(crate) fn new(parent: &Path) -> Result<Self, CliError> {
        use std::os::unix::fs::DirBuilderExt as _;
        let dir = parent.join(format!(".tm-import-{}", Uuid::now_v7()));
        fs::create_dir_all(parent).map_err(|err| CliError::io(parent, &err))?;
        // Named by its parent: the staging name is new each run, so it would
        // point at nothing and differ every time.
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|err| CliError::io(parent, &err))?;
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
            shortlists: Vec::new(),
            memories: Vec::new(),
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
    fn a_records_import_says_how_it_found_each_list() {
        let list = |source: &str, by_name: bool, deleted: bool| tm_store::ListOutcome {
            line: 1,
            source: source.into(),
            list: Uuid::now_v7(),
            name: Some("docs".into()),
            created: false,
            by_name,
            deleted,
            batches: Vec::new(),
            written: Vec::new(),
            removed: Vec::new(),
            conflicts: Vec::new(),
            placed: Vec::new(),
            taken_off: Vec::new(),
            passed_over: Vec::new(),
            members: 0,
            size: 10,
        };
        let mut found = list("x:found", false, false);
        found.conflicts.push(tm_store::Conflict {
            field: "size".into(),
            tm: json!(5),
            source: json!(3),
            reason: None,
        });
        found.written = vec!["name".into(), "actors/alice".into()];
        found.removed = vec!["actors/bob".into()];
        let member = Uuid::now_v7();
        found.passed_over.push(tm_store::Passed {
            issue: member,
            key: Some("TM-4".into()),
            reasons: vec![tm_store::PassedOver::Blocked],
        });
        let imported = RecordsImported {
            projects: Vec::new(),
            shortlists: vec![
                found,
                list("x:named", true, false),
                list("x:gone", false, true),
            ],
            memories: Vec::new(),
            records: Vec::new(),
        };
        let text = records_output(&imported, None, false, &BTreeMap::new(), "jace").text;
        for line in [
            "found Short-list docs (x:found): 0 placed, 0 taken off, 0 of 10",
            "Short-list docs: set name, actors/alice",
            "Short-list docs: removed actors/bob",
            "conflict: Short-list docs (x:found) size: kept tm's 5, the source has 3\n",
            "Short-list docs: tm next passes over TM-4 [blocked]",
            "matched by name Short-list docs (x:named)",
            "Short-list docs (x:gone) is deleted here; nothing was placed on it",
        ] {
            assert!(text.contains(line), "{text}");
        }
        let text = records_output(&imported, None, true, &BTreeMap::new(), "jace").text;
        for line in [
            "found Short-list docs (x:found): would place 0, take off 0, hold 0 of 10",
            "Short-list docs: would set name, actors/alice",
            "Short-list docs: would remove actors/bob",
            "would match by name Short-list docs (x:named)",
            "conflict: Short-list docs (x:found) size: would keep tm's 5",
            "Short-list docs (x:gone) is deleted here; nothing would be placed on it",
        ] {
            assert!(text.contains(line), "{text}");
        }
    }

    /// A place tm refused says why, and where the Issue stays, not that
    /// tm kept a value it never had.
    #[test]
    fn a_refused_place_says_why_and_where_the_issue_stays() {
        let place = |name: Option<&str>| json!({"id": Uuid::now_v7().to_string(), "name": name});
        let conflict = |tm: Value, source: Value, reason| Conflict {
            field: "shortlist".into(),
            tm,
            source,
            reason,
        };
        let conflicts = [
            conflict(
                place(Some("mine")),
                place(Some("docs")),
                Some("the source's list is deleted here"),
            ),
            conflict(
                Value::Null,
                place(Some("fin")),
                Some("the Issue is finished"),
            ),
            conflict(Value::Null, place(Some("docs")), None),
        ];
        let said = |dry_run: bool| {
            let mut text = String::new();
            for conflict in &conflicts {
                conflict_line(&mut text, "x:1", conflict, dry_run);
            }
            text
        };
        assert_eq!(
            said(false),
            "x:1 shortlist: not placed on \"docs\" (the source's list is deleted here); stays on \"mine\"\n\
             x:1 shortlist: not placed on \"fin\" (the Issue is finished)\n\
             conflict: x:1 shortlist: kept tm's no list, the source has \"docs\"\n"
        );
        assert_eq!(
            said(true),
            "x:1 shortlist: would not be placed on \"docs\" (the source's list is deleted here); would stay on \"mine\"\n\
             x:1 shortlist: would not be placed on \"fin\" (the Issue is finished)\n\
             conflict: x:1 shortlist: would keep tm's no list, the source has \"docs\"\n"
        );
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
