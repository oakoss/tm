//! `tm init`: a Workspace dedicated to the repo, mapped by `.config/tm.toml`.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use tm_store::workspace::{self, RepoFile, Workspace, WorkspaceError};
use uuid::Uuid;

use crate::error::CliError;
use crate::output::Output;
use crate::{fields, here, identity, instructions, recover};

const REPO_FILE: &str = ".config/tm.toml";

pub(crate) struct Init {
    pub(crate) key: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) yes: bool,
}

/// An `init` refusal: a check failed, or the file was not confirmed.
pub(crate) fn refused(message: String) -> CliError {
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
pub(crate) fn run(init: &Init, json: bool) -> Result<Output, CliError> {
    let cwd = std::env::current_dir()
        .map_err(|err| CliError::new("io", format!("working directory: {err}")))?;
    let root = workspace::repo_root(&cwd)?.ok_or_else(|| {
        refused("tm init runs inside a git repository; none contains this directory".into())
    })?;
    let home = workspace::home(std::env::var_os("TM_HOME"), std::env::home_dir())?;
    if let Some(found) = workspace::discover(&cwd)? {
        return already_mapped(&home, &found, init, json);
    }
    check_not_below_home_repo(&cwd, &root).map_err(refused)?;
    let identity = identity::resolve(identity::env, || identity::git_email(&root))
        .map_err(|message| CliError::new("no_identity", message))?;
    let config_dir = root.join(".config");
    check_config_dir(&root, &config_dir).map_err(refused)?;
    check_not_ignored(&root).map_err(refused)?;
    let (key, name) = project(&root, init, json).map_err(refused)?;
    let asks = human(json, init.yes).then_some(prompt);
    let beads = recover::beads(&root, (&key, &name[..]), &identity.actor, asks)?;
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
    let batch = match finished {
        Ok(batch) => batch,
        Err(err) => {
            drop(store);
            return Err(clean_up_after(err, &workspace, undo_config));
        }
    };
    let mut created = created(&workspace, &key, &target, batch);
    created.warnings = identity.warnings;
    recover::write_beads(&mut created, &mut store, beads, &identity.actor);
    created.warnings.extend(here::publish(&store, &workspace));
    drop(store);
    Ok(with_lines(created, &root, init.yes, json, &[REPO_FILE]))
}

fn created(workspace: &Workspace, key: &str, target: &Path, batch: Uuid) -> Output {
    Output::new(
        format!(
            "Created Workspace {} at {} with Project {key}, and wrote {REPO_FILE}\n",
            workspace.id,
            workspace.dir.display()
        ),
        json!({
            "created": true,
            "workspace": workspace.id,
            "dir": workspace.dir.display().to_string(),
            "project": key,
            "file": target.display().to_string(),
            "batch": batch,
        }),
    )
}

/// `output` with tm's instruction lines offered for the repo at `root`.
fn with_lines(mut output: Output, root: &Path, yes: bool, json: bool, wrote: &[&str]) -> Output {
    let offer = instructions::offer(root, human(json, yes), ask);
    instructions::report(&mut output, offer, wrote);
    output
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
/// shares the file and that its Workspace and every Project it names exist,
/// and offers tm's instruction lines.
fn already_mapped(
    home: &Path,
    found: &workspace::Found,
    init: &Init,
    json: bool,
) -> Result<Output, CliError> {
    check_spelling(found).map_err(refused)?;
    let root = workspace::repo_root(&found.dir)?.unwrap_or_else(|| found.dir.clone());
    if let Ok(relative) = found.path.strip_prefix(&root) {
        check_git_sees(&root, &relative.to_string_lossy()).map_err(refused)?;
    }
    let ((workspace, store), restored) = opened(home, found, &root, human(json, init.yes))?;
    let keys = std::iter::once(&found.file.project)
        .chain(found.file.paths.iter().map(|mapping| &mapping.project));
    for key in keys {
        let missing = match store.project_by_key(key) {
            Ok(held) => held.is_none().then(|| {
                refused(format!(
                    "{} names Project {key}, which Workspace {} does not have",
                    found.path.display(),
                    workspace.id
                ))
            }),
            Err(err) => Some(err.into()),
        };
        if let Some(err) = missing {
            return Err(match restored {
                Some(restored) => with_restored(err, restored),
                None => err,
            });
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
    let mut mapped = Output::new(
        text,
        json!({
            "created": false,
            "workspace": workspace.id,
            "dir": workspace.dir.display().to_string(),
            "project": found.file.project,
            "file": found.path.display().to_string(),
        }),
    );
    if let Some(restored) = restored {
        mapped.text.insert_str(0, &restored.text);
        mapped.data["restored"] = restored.data;
        mapped.warnings = restored.warnings;
    }
    Ok(with_lines(mapped, &root, init.yes, json, &[]))
}

/// The Workspace `found` names. One this machine lacks is restored from an
/// export a person gives, with what the import printed; otherwise its error
/// also says how to start over.
fn opened(
    home: &Path,
    found: &workspace::Found,
    root: &Path,
    asks: bool,
) -> Result<((Workspace, tm_store::Store), Option<Output>), CliError> {
    match workspace::workspace_for(home, found) {
        Err(err @ WorkspaceError::UnknownWorkspace { .. }) => {
            let restored = if asks {
                recover::restore(&found.path, &found.file.workspace.to_string(), prompt)?
            } else {
                None
            };
            let Some(restored) = restored else {
                return Err(start_over(CliError::from(err), &found.path, root));
            };
            match workspace::workspace_for(home, found) {
                Ok(opened) => Ok((opened, Some(restored))),
                Err(err) => Err(with_restored(err.into(), restored)),
            }
        }
        opened => Ok((opened?, None)),
    }
}

/// `err` saying how a person could start over without the Workspace. Only a
/// person decides that, since the new Workspace would not be the one the
/// repo shares.
fn start_over(mut err: CliError, file: &Path, root: &Path) -> CliError {
    // `:/` makes git read the path from the repo root, wherever init ran; the
    // file init writes may not be the one removed, such as a `.tm.toml`.
    let file = file.strip_prefix(root).unwrap_or(file);
    let steps = [
        format!(
            "git rm -q {}",
            fields::quoted(&format!(":/{}", file.to_string_lossy()))
        ),
        "tm init".to_owned(),
        format!("git add :/{REPO_FILE}"),
    ];
    let _ = write!(
        err.message,
        "; if nobody has an export, a person may start a new Workspace instead: {}, then commit",
        steps.join(", ")
    );
    err.with("start_over", steps.to_vec())
}

/// `err`, after a restore that worked, still reporting what it restored.
fn with_restored(err: CliError, restored: Output) -> CliError {
    let mut warnings = vec![restored.text.trim_end().to_owned()];
    warnings.extend(restored.warnings);
    err.warned(warnings).with("restored", restored.data)
}

/// The new Project's key and name, asked of a person at a terminal.
fn project(root: &Path, init: &Init, json: bool) -> Result<(String, String), String> {
    let dir_name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    named(
        dir_name,
        init.key.clone(),
        init.name.clone(),
        human(json, init.yes).then_some(prompt),
    )
}

/// The key and name, each suggested from `dir_name`. `prompt`, given only
/// for a person at a terminal, asks for whichever the flags left out; it
/// returns the typed line, or `None` at end of input.
fn named(
    dir_name: String,
    key: Option<String>,
    name: Option<String>,
    mut prompt: Option<impl FnMut(&str) -> Result<Option<String>, String>>,
) -> Result<(String, String), String> {
    let suggested = default_key(&dir_name);
    let key = match (key, prompt.as_mut()) {
        (Some(key), _) => {
            let key = key.to_ascii_uppercase();
            check_key(&key)?;
            key
        }
        (None, Some(prompt)) => ask_key(prompt, suggested.as_deref())?,
        (None, None) => suggested.ok_or_else(|| {
            format!("cannot make a Project key from {dir_name:?}; pass one, such as --key TM")
        })?,
    };
    let name = match (name, prompt) {
        (Some(name), _) => name,
        (None, Some(mut prompt)) => {
            let typed = prompt(&format!("Project name [{dir_name}]: "))?.ok_or_else(stopped)?;
            if typed.is_empty() { dir_name } else { typed }
        }
        (None, None) => dir_name,
    };
    if name.trim().is_empty() {
        return Err("the Project name cannot be blank; pass one with --name".into());
    }
    Ok((key, name))
}

/// Asks until a valid key or Enter on a suggestion, saying why a typed one
/// was refused.
fn ask_key(
    prompt: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    suggested: Option<&str>,
) -> Result<String, String> {
    let question = match suggested {
        Some(key) => format!("Project key [{key}]: "),
        None => "Project key: ".to_owned(),
    };
    let mut asking = question.clone();
    loop {
        let typed = prompt(&asking)?.ok_or_else(stopped)?;
        let key = match (typed.as_str(), suggested) {
            ("", Some(key)) => return Ok(key.to_owned()),
            ("", None) => Err("a Project key is needed".to_owned()),
            (typed, _) => {
                let key = typed.to_ascii_uppercase();
                check_key(&key).map(|()| key)
            }
        };
        match key {
            Ok(key) => return Ok(key),
            Err(reason) => asking = format!("{reason}\n{question}"),
        }
    }
}

fn check_key(key: &str) -> Result<(), String> {
    if tm_core::project::is_key(key) {
        Ok(())
    } else {
        Err(format!(
            "{key:?} is not a Project key: a letter, then 1 to 9 letters or digits"
        ))
    }
}

pub(crate) fn stopped() -> String {
    "stopped at the prompt; nothing written".into()
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

/// A short Project key from the directory name: the initials of its words,
/// or the first 3 letters of a single word, or `None` when that is not a key.
fn default_key(dir_name: &str) -> Option<String> {
    let words = words(dir_name);
    let key: String = match words.as_slice() {
        [word] => word.chars().take(3).collect(),
        words => words
            .iter()
            .filter_map(|word| word.chars().next())
            .take(10)
            .collect(),
    };
    let key = key.to_ascii_uppercase();
    tm_core::project::is_key(&key).then_some(key)
}

/// `name` split into ASCII letters and digits at anything else and where a
/// lowercase letter meets an uppercase one, as in `myRepo`.
fn words(name: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut previous = None;
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() {
            previous = None;
            continue;
        }
        let starts = match previous {
            None => true,
            Some(before) => char::is_ascii_lowercase(&before) && c.is_ascii_uppercase(),
        };
        match words.last_mut() {
            Some(word) if !starts => word.push(c),
            _ => words.push(c.to_string()),
        }
        previous = Some(c);
    }
    words
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
    if yes {
        return print_now(&diff(text));
    }
    if !human(json, false) {
        print_now(&diff(text))?;
        return Err(format!(
            "{REPO_FILE} not written: confirm with --yes when not a person at a terminal"
        ));
    }
    if ask(&diff(text), &format!("Write {REPO_FILE}?"))? {
        Ok(())
    } else {
        Err(format!("{REPO_FILE} not written"))
    }
}

/// A person at a terminal, the only one tm asks (ADR 0014): stdin and stdout
/// are terminals, JSON is off, and no Agent is detected. `--yes` asks nothing.
fn human(json: bool, yes: bool) -> bool {
    is_human(
        json || yes,
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        identity::agent_detected(identity::env),
    )
}

/// `quiet` is `--json` or `--yes`, either of which asks nothing.
fn is_human(quiet: bool, terminal: bool, agent: bool) -> bool {
    !quiet && terminal && !agent
}

/// Shows `shown` and asks `question` at the terminal, defaulting to no.
fn ask(shown: &str, question: &str) -> Result<bool, String> {
    print_now(shown)?;
    eprint!("{question} [y/N] ");
    answer(&mut io::stdin().lock())
}

/// A yes read from `input`; anything else, end of input included, is no.
fn answer(input: &mut impl BufRead) -> Result<bool, String> {
    let mut line = Vec::new();
    let read = input
        .read_until(b'\n', &mut line)
        .map_err(|err| format!("cannot read the answer: {err}"))?;
    if read == 0 {
        eprintln!();
    }
    Ok(matches!(line.trim_ascii(), b"y" | b"Y" | b"yes"))
}

/// Asks `question` at the terminal for a line of text.
fn prompt(question: &str) -> Result<Option<String>, String> {
    eprint!("{question}");
    line(&mut io::stdin().lock())
}

/// One line read from `input`, trimmed, or `None` at end of input.
fn line(input: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut line = String::new();
    let read = input
        .read_line(&mut line)
        .map_err(|err| format!("cannot read the answer: {err}"))?;
    if read == 0 {
        eprintln!();
        return Ok(None);
    }
    Ok(Some(line.trim().to_owned()))
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

#[cfg(test)]
mod tests {
    use super::{answer, default_key, is_human, line, named};

    #[test]
    fn only_a_person_at_a_terminal_without_json_is_asked() {
        assert!(is_human(false, true, false));
        assert!(
            !is_human(true, true, false),
            "--json or --yes is never asked"
        );
        assert!(!is_human(false, false, false), "a pipe is never asked");
        assert!(!is_human(false, true, true), "an Agent is never asked");
    }

    #[test]
    fn only_a_yes_is_yes() {
        for (typed, yes) in [
            ("y\n", true),
            ("yes\n", true),
            (" Y \n", true),
            ("\n", false),
            ("no\n", false),
            ("yess\n", false),
            ("", false),
        ] {
            assert_eq!(answer(&mut typed.as_bytes()), Ok(yes), "{typed:?}");
        }
    }

    #[test]
    fn the_suggested_key_is_short() {
        for (dir, key) in [
            ("task-management", Some("TM")),
            ("my_cool.app", Some("MCA")),
            ("myCoolApp", Some("MCA")),
            ("HTTPServer", Some("HTT")),
            ("oakoss", Some("OAK")),
            ("tm", Some("TM")),
            ("web3-app", Some("WA")),
            ("web3App", Some("WEB")),
            ("a-b-c-d-e-f-g-h-i-j-k-l", Some("ABCDEFGHIJ")),
            ("x", None),
            ("2fa-tool", None),
            ("---", None),
        ] {
            assert_eq!(default_key(dir).as_deref(), key, "{dir:?}");
        }
    }

    /// Answers each question with the next of `typed`, recording what was
    /// asked; `None` stands for end of input.
    fn scripted<'a>(
        typed: &'a [Option<&'a str>],
        asked: &'a mut Vec<String>,
    ) -> impl FnMut(&str) -> Result<Option<String>, String> + 'a {
        let mut typed = typed.iter();
        move |question| {
            asked.push(question.to_owned());
            Ok(typed.next().expect("asked too often").map(str::to_owned))
        }
    }

    #[test]
    fn enter_takes_the_suggestions() {
        let mut asked = Vec::new();
        let chosen = named(
            "task-management".into(),
            None,
            None,
            Some(scripted(&[Some(""), Some("")], &mut asked)),
        );
        assert_eq!(chosen, Ok(("TM".into(), "task-management".into())));
        assert_eq!(
            asked,
            ["Project key [TM]: ", "Project name [task-management]: "]
        );
    }

    #[test]
    fn a_refused_key_is_asked_again_with_the_reason() {
        let mut asked = Vec::new();
        let chosen = named(
            "task-management".into(),
            None,
            None,
            Some(scripted(
                &[Some("1x"), Some("y-2"), Some("ops"), Some("Ops tooling")],
                &mut asked,
            )),
        );
        assert_eq!(chosen, Ok(("OPS".into(), "Ops tooling".into())));
        assert_eq!(
            asked[1],
            "\"1X\" is not a Project key: a letter, then 1 to 9 letters or digits\nProject key [TM]: "
        );
        assert_eq!(
            asked[2],
            "\"Y-2\" is not a Project key: a letter, then 1 to 9 letters or digits\nProject key [TM]: ",
            "only the latest reason is shown"
        );

        let mut asked = Vec::new();
        let chosen = named(
            "2fa".into(),
            None,
            Some("Two".into()),
            Some(scripted(&[Some(""), Some("tf")], &mut asked)),
        );
        assert_eq!(chosen, Ok(("TF".into(), "Two".into())));
        assert_eq!(
            asked,
            ["Project key: ", "a Project key is needed\nProject key: "]
        );
    }

    #[test]
    fn flags_are_not_asked_again() {
        let mut asked = Vec::new();
        let chosen = named(
            "repo".into(),
            Some("tm".into()),
            Some("Task".into()),
            Some(scripted(&[], &mut asked)),
        );
        assert_eq!(chosen, Ok(("TM".into(), "Task".into())));
        assert_eq!(asked, Vec::<String>::new());

        let mut asked = Vec::new();
        let chosen = named(
            "repo".into(),
            Some("1x".into()),
            None,
            Some(scripted(&[], &mut asked)),
        );
        assert_eq!(
            chosen,
            Err("\"1X\" is not a Project key: a letter, then 1 to 9 letters or digits".into()),
            "a bad --key is refused, not asked again"
        );
        assert_eq!(asked, Vec::<String>::new());
    }

    #[test]
    fn end_of_input_stops_before_anything_is_written() {
        for typed in [&[None][..], &[Some(""), None]] {
            let mut asked = Vec::new();
            let chosen = named("repo".into(), None, None, Some(scripted(typed, &mut asked)));
            assert_eq!(
                chosen,
                Err("stopped at the prompt; nothing written".into()),
                "{typed:?}"
            );
        }
    }

    #[test]
    fn a_line_is_trimmed_and_end_of_input_is_none() {
        assert_eq!(line(&mut " ops \n".as_bytes()), Ok(Some("ops".into())));
        assert_eq!(line(&mut "\n".as_bytes()), Ok(Some(String::new())));
        assert_eq!(line(&mut "".as_bytes()), Ok(None));
    }
}
