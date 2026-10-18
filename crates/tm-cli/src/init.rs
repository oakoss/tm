//! `tm init`: a Workspace dedicated to the repo, mapped by `.config/tm.toml`.

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use tm_store::workspace::{self, RepoFile, Workspace};
use uuid::Uuid;

use crate::error::CliError;
use crate::identity;
use crate::output::Output;

const REPO_FILE: &str = ".config/tm.toml";

pub(crate) struct Init {
    pub(crate) key: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) yes: bool,
}

/// An `init` refusal: a check failed, or the file was not confirmed.
fn refused(message: String) -> CliError {
    CliError::new("init_refused", message)
}

fn io_failed(path: &Path, message: String) -> CliError {
    CliError::new("io", message).with("path", path.display().to_string())
}

/// Names a path a failed `init` could not clean up, for the caller to remove.
fn left_behind(err: CliError, path: &Path) -> CliError {
    let mut paths = match err.details.get("left_behind") {
        Some(serde_json::Value::Array(paths)) => paths.clone(),
        _ => Vec::new(),
    };
    paths.push(json!(path.display().to_string()));
    err.with("left_behind", paths)
}

/// Runs `tm init` in the working directory. Everything that can be checked
/// is checked before the file is shown; nothing is written until it is
/// confirmed, and the Workspace is made before the file that names it and
/// removed again if the file cannot be. In JSON mode nothing is shown or
/// asked, so `--yes` is the confirmation.
pub(crate) fn run(init: Init, json: bool) -> Result<Output, CliError> {
    let cwd = std::env::current_dir()
        .map_err(|err| CliError::new("io", format!("working directory: {err}")))?;
    let root = workspace::repo_root(&cwd)?.ok_or_else(|| {
        refused("tm init runs inside a git repository; none contains this directory".into())
    })?;
    let home = workspace::home(std::env::var_os("TM_HOME"), std::env::home_dir())?;
    if let Some(found) = workspace::discover(&cwd)? {
        return already_mapped(&home, &found, &init);
    }
    check_not_below_home_repo(&cwd, &root).map_err(refused)?;
    let (key, name) = project(&root, init.key, init.name).map_err(refused)?;
    let identity =
        identity::resolve(identity::env, || identity::git_email(&root)).map_err(refused)?;
    let config_dir = root.join(".config");
    check_config_dir(&root, &config_dir).map_err(refused)?;
    check_not_ignored(&root).map_err(refused)?;
    let file = RepoFile {
        workspace: Uuid::now_v7(),
        project: key.clone(),
        paths: Vec::new(),
    };
    let text = file.render();
    confirmed(&text, init.yes, json).map_err(refused)?;
    let made_config = match fs::create_dir(&config_dir) {
        Ok(()) => true,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => false,
        Err(err) => return Err(io_failed(&config_dir, failed(&config_dir, &err))),
    };
    // Removes only the empty `.config` this run made; `remove_dir` cannot
    // delete anything else.
    let undo_config = || {
        if made_config {
            let _ = fs::remove_dir(&config_dir);
        }
    };
    // The repo may have changed while the prompt waited.
    let still_fits = check_config_dir(&root, &config_dir)
        .and_then(|()| check_not_ignored(&root))
        .map_err(refused)
        .and_then(|()| match workspace::discover(&cwd)? {
            Some(found) => Err(refused(format!(
                "{} appeared while waiting; {REPO_FILE} not written",
                found.path.display()
            ))),
            None => Ok(()),
        });
    if let Err(err) = still_fits {
        undo_config();
        return Err(err);
    }
    let (workspace, mut store) = match Workspace::create(&home, file.workspace) {
        Ok(made) => made,
        Err(err) => {
            undo_config();
            return Err(err.into());
        }
    };
    let target = root.join(REPO_FILE);
    let finished = store
        .create_project(&identity.actor, &key, &name)
        .map_err(CliError::from)
        .and_then(|(_, op)| write_new(&target, &text).map(|()| op.op_id))
        .and_then(|batch| {
            check_git_sees(&root, REPO_FILE)
                .map(|()| batch)
                .map_err(|err| match fs::remove_file(&target) {
                    Ok(()) => refused(err),
                    Err(cleanup) => left_behind(
                        refused(format!(
                            "{err}; {} was left behind ({cleanup}), so remove it",
                            target.display()
                        )),
                        &target,
                    ),
                })
        });
    drop(store);
    let batch = match finished {
        Ok(batch) => batch,
        Err(err) => return Err(clean_up_after(err, &workspace, undo_config)),
    };
    Ok(Output {
        text: format!(
            "Created Workspace {} at {} with Project {key}, and wrote {REPO_FILE}\n",
            workspace.id,
            workspace.dir.display()
        ),
        data: json!({
            "created": true,
            "workspace": workspace.id,
            "dir": workspace.dir.display().to_string(),
            "project": key,
            "file": target.display().to_string(),
            "batch": batch,
        }),
        warnings: identity.warnings,
    })
}

/// Removes the Workspace a failed `init` made, naming whatever it could not.
fn clean_up_after(
    mut err: CliError,
    workspace: &Workspace,
    undo_config: impl FnOnce(),
) -> CliError {
    undo_config();
    if let Err(cleanup) = fs::remove_dir_all(&workspace.dir) {
        err.message = format!(
            "{}; Workspace {} at {} was left behind ({cleanup}) and nothing names it, so remove it",
            err.message,
            workspace.id,
            workspace.dir.display()
        );
        err = left_behind(err, &workspace.dir);
    }
    err
}

/// A git repo at `$HOME` holds unrelated directories, so init from one of
/// them most likely ran before that directory's own `git init`. A `$HOME`
/// that does not exist cannot be the repo root.
fn check_not_below_home_repo(cwd: &Path, root: &Path) -> Result<(), String> {
    let Some(home) = std::env::home_dir() else {
        return Ok(());
    };
    if !home.is_absolute() {
        return Err(format!(
            "the home directory must be an absolute path, not \"{}\"",
            home.display()
        ));
    }
    let user_home = match fs::canonicalize(&home) {
        Ok(path) => path,
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(());
        }
        Err(err) => {
            return Err(format!(
                "the home directory {}: {err}; set HOME to a directory tm can read",
                home.display()
            ));
        }
    };
    let cwd = fs::canonicalize(cwd).map_err(|err| format!("working directory: {err}"))?;
    if root == user_home && cwd != user_home {
        return Err(format!(
            "{} is inside the git repository at your home directory {}; run git init here first, or run tm init from {} to map your home directory",
            cwd.display(),
            user_home.display(),
            user_home.display()
        ));
    }
    Ok(())
}

/// Reports the mapping a repo file already makes, after checking that git
/// shares the file and that its Workspace and every Project it names exist.
fn already_mapped(home: &Path, found: &workspace::Found, init: &Init) -> Result<Output, CliError> {
    check_spelling(found).map_err(refused)?;
    let root = workspace::repo_root(&found.dir)?.unwrap_or_else(|| found.dir.clone());
    if let Ok(relative) = found.path.strip_prefix(&root) {
        check_git_sees(&root, &relative.to_string_lossy()).map_err(refused)?;
    }
    let (workspace, store) = workspace::workspace_for(home, found)?;
    let keys = std::iter::once(&found.file.project)
        .chain(found.file.paths.iter().map(|mapping| &mapping.project));
    for key in keys {
        if store.project_by_key(key)?.is_none() {
            return Err(refused(format!(
                "{} names Project {key}, which Workspace {} does not have",
                found.path.display(),
                workspace.id
            )));
        }
    }
    let mut text = format!(
        "{} already maps {} to Workspace {} and Project {}\n",
        found.path.display(),
        found.dir.display(),
        workspace.id,
        found.file.project
    );
    if init.key.is_some() || init.name.is_some() {
        text.push_str("--key and --name apply only when a Workspace is created; nothing changed\n");
    }
    Ok(Output::new(
        text,
        json!({
            "created": false,
            "workspace": workspace.id,
            "dir": workspace.dir.display().to_string(),
            "project": found.file.project,
            "file": found.path.display().to_string(),
        }),
    ))
}

/// The new Project's key and name, each defaulting to the repo directory's.
fn project(
    root: &Path,
    key: Option<String>,
    name: Option<String>,
) -> Result<(String, String), String> {
    let dir_name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let key = match key {
        Some(key) => key.to_ascii_uppercase(),
        None => default_key(&dir_name).ok_or_else(|| {
            format!("cannot make a Project key from {dir_name:?}; pass one, such as --key TM")
        })?,
    };
    if !tm_core::project::is_key(&key) {
        return Err(format!(
            "{key:?} is not a Project key: a letter, then 1 to 9 letters or digits"
        ));
    }
    let name = name.unwrap_or(dir_name);
    if name.trim().is_empty() {
        return Err("the Project name cannot be blank; pass one with --name".into());
    }
    Ok((key, name))
}

/// A case-insensitive disk finds `.config/TM.toml` as `.config/tm.toml`, but
/// git records the name on disk, which a case-sensitive clone does not find.
fn check_spelling(found: &workspace::Found) -> Result<(), String> {
    let relative = found.path.strip_prefix(&found.dir).map_err(|_| {
        format!(
            "{} is outside {}",
            found.path.display(),
            found.dir.display()
        )
    })?;
    let mut dir = found.dir.clone();
    for part in relative.components() {
        let name = part.as_os_str();
        let spelled = fs::read_dir(&dir)
            .map_err(|err| failed(&dir, &err))?
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().eq_ignore_ascii_case(name))
            .map(|entry| entry.file_name());
        if spelled.as_deref() != Some(name) {
            return Err(format!(
                "{} is spelled {} on disk; git records that name, so rename it to {}",
                dir.join(name).display(),
                spelled.unwrap_or_default().display(),
                name.display()
            ));
        }
        dir.push(name);
    }
    Ok(())
}

/// The directory name as a Project key: its letters and digits, uppercased
/// and cut to the 10 a key allows, or `None` when that is not a key.
fn default_key(dir_name: &str) -> Option<String> {
    let key: String = dir_name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .take(10)
        .collect();
    tm_core::project::is_key(&key).then_some(key)
}

/// `.config` must be a real directory named exactly that, or absent: git does
/// not track a file reached through a symlink, and records `.Config` as a
/// different path than the `.config` a case-sensitive clone looks for.
fn check_config_dir(root: &Path, config_dir: &Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(config_dir) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(failed(config_dir, &err)),
    };
    let exact = fs::read_dir(root)
        .map_err(|err| failed(root, &err))?
        .filter_map(Result::ok)
        .any(|entry| entry.file_name() == ".config");
    if !exact {
        return Err(format!(
            "{} holds a .config spelled in another case; tm init writes {REPO_FILE}, and git tells the two apart",
            root.display()
        ));
    }
    if metadata.is_symlink() {
        return Err(format!(
            "{} is a symlink; tm init writes {REPO_FILE} only into a real directory git can track",
            config_dir.display()
        ));
    }
    if !metadata.is_dir() {
        return Err(format!(
            "{} is not a directory; tm init writes {REPO_FILE}",
            config_dir.display()
        ));
    }
    Ok(())
}

/// Refuses when git would ignore the repo file, since no clone would get it.
fn check_not_ignored(root: &Path) -> Result<(), String> {
    // Without --verbose, `git check-ignore` exits 0 only when the path ends up
    // ignored; with it, a `!` rule that lets the file through also matches.
    let ignored = git(root, &["check-ignore", "--quiet", "--", REPO_FILE])?;
    match ignored.status.code() {
        Some(1) => Ok(()),
        Some(0) => {
            let rule = git(root, &["check-ignore", "--verbose", "--", REPO_FILE])?;
            Err(format!(
                "git ignores {REPO_FILE} ({}), so no clone would see it; change that rule first",
                String::from_utf8_lossy(&rule.stdout).trim()
            ))
        }
        _ => Err(git_failed("check-ignore", &ignored)),
    }
}

/// Confirms git shares `relative`, a path from the repo root: it is a file
/// of this repo under its exact name, new or tracked before, and the index
/// holds no other spelling of it. A nested repo, an ignore rule, or a
/// case-insensitive match on disk or in the index would each keep it from
/// a clone.
fn check_git_sees(root: &Path, relative: &str) -> Result<(), String> {
    let listed = |pathspec: &str, others: bool| -> Result<Vec<String>, String> {
        let mut args = vec!["ls-files", "--cached", "-z"];
        if others {
            args.extend(["--others", "--exclude-standard"]);
        }
        args.extend(["--", pathspec]);
        let output = git(root, &args)?;
        if !output.status.success() {
            return Err(git_failed("ls-files", &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .map(str::to_owned)
            .collect())
    };
    let seen = listed(relative, true)?;
    if seen != [relative] {
        return Err(format!(
            "git does not see {relative} as a file of this repo (it lists: {}), so no clone would get it",
            if seen.is_empty() {
                "nothing".into()
            } else {
                seen.join(", ")
            }
        ));
    }
    let spellings = listed(&format!(":(icase){relative}"), false)?;
    if let Some(other) = spellings.iter().find(|entry| *entry != relative) {
        return Err(format!(
            "git tracks {other}, which this disk does not tell apart from {relative}; a clone would look for {relative}, so rename or remove {other} first"
        ));
    }
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|err| format!("cannot run git {}: {err}", args[0]))
}

fn git_failed(command: &str, output: &std::process::Output) -> String {
    format!(
        "git {command} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

fn diff(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = format!(
        "--- /dev/null\n+++ b/{REPO_FILE}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in lines {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn print_now(text: &str) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|err| format!("cannot write output: {err}"))
}

/// Shows the file and asks, unless `--yes` confirms it already. JSON mode
/// shows and asks nothing.
fn confirmed(text: &str, yes: bool, json: bool) -> Result<(), String> {
    if json {
        return if yes {
            Ok(())
        } else {
            Err(format!("{REPO_FILE} not written: confirm with --yes"))
        };
    }
    print_now(&diff(text))?;
    if yes || confirm()? {
        Ok(())
    } else {
        Err(format!("{REPO_FILE} not written"))
    }
}

/// Asks at a terminal; anywhere else, such as an agent's shell, `--yes` is
/// the confirmation.
fn confirm() -> Result<bool, String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(format!(
            "{REPO_FILE} not written: confirm with --yes when not at a terminal"
        ));
    }
    eprint!("Write {REPO_FILE}? [y/N] ");
    let mut answer = Vec::new();
    let read = io::stdin()
        .lock()
        .read_until(b'\n', &mut answer)
        .map_err(|err| format!("cannot read the answer: {err}"))?;
    if read == 0 {
        eprintln!();
    }
    Ok(matches!(answer.trim_ascii(), b"y" | b"Y" | b"yes"))
}

fn failed(path: &Path, err: &io::Error) -> String {
    format!("{}: {err}", path.display())
}

/// Writes `text` to a file that must not exist yet, so a concurrent
/// `tm init` fails rather than overwriting. A file this call created but
/// could not finish is removed, so a retry is not blocked by it.
fn write_new(target: &PathBuf, text: &str) -> Result<(), CliError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|err| io_failed(target, failed(target, &err)))?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|err| match fs::remove_file(target) {
            Ok(()) => io_failed(target, failed(target, &err)),
            Err(cleanup) => left_behind(
                io_failed(
                    target,
                    format!(
                        "{}; the partial file was left behind ({cleanup}), so remove it",
                        failed(target, &err)
                    ),
                ),
                target,
            ),
        })
}
