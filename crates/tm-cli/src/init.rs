//! `tm init`: a Workspace dedicated to the repo, mapped by `.config/tm.toml`.

use std::env::VarError;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use tm_store::workspace::{self, RepoFile, Workspace};
use uuid::Uuid;

const REPO_FILE: &str = ".config/tm.toml";

pub(crate) struct Init {
    pub(crate) key: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) yes: bool,
}

/// Runs `tm init` in the working directory, returning what to print last.
/// Everything that can be checked is checked before the file is shown;
/// nothing is written until it is confirmed, and the Workspace is made
/// before the file that names it and removed again if the file cannot be.
pub(crate) fn run(init: Init) -> Result<String, String> {
    let cwd = std::env::current_dir().map_err(|err| format!("working directory: {err}"))?;
    let root = workspace::repo_root(&cwd)
        .map_err(|err| err.to_string())?
        .ok_or("tm init runs inside a git repository; none contains this directory")?;
    let home = workspace::home(std::env::var_os("TM_HOME"), std::env::home_dir())
        .map_err(|err| err.to_string())?;
    if let Some(found) = workspace::discover(&cwd).map_err(|err| err.to_string())? {
        return already_mapped(&home, &found, &init);
    }
    check_not_below_home_repo(&cwd, &root)?;
    let (key, name) = project(&root, init.key, init.name)?;
    let actor = actor(&root)?;
    let config_dir = root.join(".config");
    check_config_dir(&root, &config_dir)?;
    check_not_ignored(&root)?;
    let file = RepoFile {
        workspace: Uuid::now_v7(),
        project: key.clone(),
        paths: Vec::new(),
    };
    let text = file.render();
    print_now(&diff(&text))?;
    if !init.yes && !confirm()? {
        return Err(format!("{REPO_FILE} not written"));
    }
    let made_config = match fs::create_dir(&config_dir) {
        Ok(()) => true,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => false,
        Err(err) => return Err(failed(&config_dir, &err)),
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
        .and_then(
            |()| match workspace::discover(&cwd).map_err(|err| err.to_string())? {
                Some(found) => Err(format!(
                    "{} appeared while waiting; {REPO_FILE} not written",
                    found.path.display()
                )),
                None => Ok(()),
            },
        );
    if let Err(err) = still_fits {
        undo_config();
        return Err(err);
    }
    let (workspace, mut store) = match Workspace::create(&home, file.workspace) {
        Ok(made) => made,
        Err(err) => {
            undo_config();
            return Err(err.to_string());
        }
    };
    let target = root.join(REPO_FILE);
    let finished = store
        .create_project(&actor, &key, &name)
        .map_err(|err| err.to_string())
        .and_then(|_| write_new(&target, &text))
        .and_then(|()| {
            check_git_sees(&root, REPO_FILE).map_err(|err| match fs::remove_file(&target) {
                Ok(()) => err,
                Err(cleanup) => format!(
                    "{err}; {} was left behind ({cleanup}), so remove it",
                    target.display()
                ),
            })
        });
    drop(store);
    if let Err(err) = finished {
        undo_config();
        return Err(match fs::remove_dir_all(&workspace.dir) {
            Ok(()) => err,
            Err(cleanup) => format!(
                "{err}; Workspace {} at {} was left behind ({cleanup}) and nothing names it, so remove it",
                workspace.id,
                workspace.dir.display()
            ),
        });
    }
    Ok(format!(
        "Created Workspace {} at {} with Project {key}, and wrote {REPO_FILE}\n",
        workspace.id,
        workspace.dir.display()
    ))
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
fn already_mapped(home: &Path, found: &workspace::Found, init: &Init) -> Result<String, String> {
    check_spelling(found)?;
    let root = workspace::repo_root(&found.dir)
        .map_err(|err| err.to_string())?
        .unwrap_or_else(|| found.dir.clone());
    if let Ok(relative) = found.path.strip_prefix(&root) {
        check_git_sees(&root, &relative.to_string_lossy())?;
    }
    let (workspace, store) =
        workspace::workspace_for(home, found).map_err(|err| err.to_string())?;
    let keys = std::iter::once(&found.file.project)
        .chain(found.file.paths.iter().map(|mapping| &mapping.project));
    for key in keys {
        if store
            .project_by_key(key)
            .map_err(|err| err.to_string())?
            .is_none()
        {
            return Err(format!(
                "{} names Project {key}, which Workspace {} does not have",
                found.path.display(),
                workspace.id
            ));
        }
    }
    let mut out = format!(
        "{} already maps {} to Workspace {} and Project {}\n",
        found.path.display(),
        found.dir.display(),
        workspace.id,
        found.file.project
    );
    if init.key.is_some() || init.name.is_some() {
        out.push_str("--key and --name apply only when a Workspace is created; nothing changed\n");
    }
    Ok(out)
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

/// Who writes: `TM_ACTOR`, else git's `user.email`. A `TM_ACTOR` that is set
/// but unusable is an error, never a reason to write as someone else.
fn actor(root: &Path) -> Result<String, String> {
    match std::env::var("TM_ACTOR") {
        Ok(actor) if actor.trim().is_empty() => {
            return Err("TM_ACTOR is set but blank; set it to who you are, or unset it".into());
        }
        Ok(actor) => return Ok(actor.trim().to_owned()),
        Err(VarError::NotUnicode(_)) => return Err("TM_ACTOR is not valid UTF-8".into()),
        Err(VarError::NotPresent) => {}
    }
    let output = Command::new("git")
        .args(["config", "--get", "user.email"])
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|err| format!("cannot run git to read user.email: {err}"))?;
    let email = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    // `git config --get` exits 1 when the key is unset.
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let said = if stderr.is_empty() {
        String::new()
    } else {
        format!(" (git said: {stderr})")
    };
    match output.status.code() {
        Some(0) if !email.is_empty() => Ok(email),
        Some(0 | 1) => Err(format!(
            "cannot tell who you are; set TM_ACTOR or git's user.email{said}"
        )),
        _ => Err(format!(
            "git config --get user.email failed ({}){said}",
            output.status
        )),
    }
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

/// Asks at a terminal; anywhere else, such as an agent's shell, `--yes` is
/// the confirmation.
fn confirm() -> Result<bool, String> {
    if !io::stdin().is_terminal() {
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
fn write_new(target: &PathBuf, text: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|err| failed(target, &err))?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|err| match fs::remove_file(target) {
            Ok(()) => failed(target, &err),
            Err(cleanup) => format!(
                "{}; the partial file was left behind ({cleanup}), so remove it",
                failed(target, &err)
            ),
        })
}
