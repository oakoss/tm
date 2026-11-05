//! Path Claims: which other sessions' live Claims cover a file, by a path
//! they name or as already committed on their branch.

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

use tm_core::actor::Actor;
use tm_core::paths;
use tm_store::{AgentClaims, Claim};

use crate::error::CliError;
use crate::output::printable;
use crate::{claim, scan};

/// What a session is told of `overlap`, with its Issue `shown` by Key.
pub(crate) fn told(overlap: &Overlap, shown: &str) -> String {
    let how = match &overlap.why {
        Why::Path(path) if *path == overlap.file => String::new(),
        Why::Path(path) => format!(" through {}", printable(path)),
        Why::Branch(branch) => format!(", committed on {}", printable(branch)),
    };
    format!(
        "{shown}'s Claim by {} covers {}{how}",
        claim::holder(&overlap.claim),
        printable(&overlap.file)
    )
}

/// `file`, a relative one read from `base`, from the root of the checkout it
/// lies in, `/`-separated: the nearest folder above it holding `.git`, which
/// must be `root` or another worktree of the same repository, so a nested
/// worktree counts and a nested repository or submodule does not. A file not
/// written yet is placed by the nearest folder that exists, so a link in the
/// path still resolves.
pub(crate) fn in_repo(root: &Path, base: &Path, file: &str) -> Option<String> {
    let file = base.join(file);
    let (resolved, rest) = existing(&file)?;
    let start = if resolved.is_dir() {
        resolved.as_path()
    } else {
        resolved.parent()?
    };
    let top = start
        .ancestors()
        .find(|dir| dir.join(".git").exists())?
        .to_path_buf();
    let full: PathBuf = rest
        .iter()
        .rev()
        .fold(resolved.clone(), |path, part| path.join(part));
    let root = std::fs::canonicalize(root).ok()?;
    if top == root {
        return inside(&root, &full);
    }
    let (theirs, top) = repository(&top)?;
    let (ours, _) = repository(&root)?;
    (theirs == ours).then(|| inside(&top, &full)).flatten()
}

/// The repository's shared git folder and the worktree's root, resolved, for
/// the worktree holding `dir`.
fn repository(dir: &Path) -> Option<(String, PathBuf)> {
    let said = scan::git(
        dir,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--show-toplevel",
        ],
    )
    .ok()?;
    let mut lines = said.lines();
    let common = lines.next()?.to_owned();
    Some((common, std::fs::canonicalize(lines.next()?).ok()?))
}

/// `full`, a resolved path, from the resolved `root`, `/`-separated.
fn inside(root: &Path, full: &Path) -> Option<String> {
    let inside = full.strip_prefix(root).ok()?;
    let parts: Vec<&str> = inside
        .components()
        .map(|part| match part {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The nearest part of `file` that exists, resolved, and the names below it
/// that do not, innermost first.
fn existing(file: &Path) -> Option<(PathBuf, Vec<&std::ffi::OsStr>)> {
    if !file.is_absolute() {
        return None;
    }
    let mut at = file;
    let mut rest = Vec::new();
    loop {
        if let Ok(resolved) = std::fs::canonicalize(at) {
            return Some((resolved, rest));
        }
        rest.push(at.file_name()?);
        at = at.parent()?;
    }
}

/// Why a Claim covers a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Why {
    /// A path the Claim names, which may be a glob.
    Path(String),
    /// Already committed on the Claim's branch and not on the default one.
    Branch(String),
}

/// A file another session's Claim covers.
#[derive(Debug, Clone)]
pub(crate) struct Overlap {
    pub(crate) claim: Claim,
    pub(crate) file: String,
    pub(crate) why: Why,
}

/// What a check found, and what it could not check, each said once.
#[derive(Debug, Default)]
pub(crate) struct Check {
    pub(crate) found: Vec<Overlap>,
    pub(crate) unchecked: Vec<String>,
    /// Whether a file needed a branch's files, so `unchecked` says all a
    /// branch could not tell.
    pub(crate) branches: bool,
}

/// The live Claims naming paths or a branch that `me` did not take by its
/// name in its session; a sub-agent's and a handed Claim's count as others.
pub(crate) fn others(claims: &AgentClaims, me: Option<&Actor>) -> Vec<Claim> {
    let own = |claim: &Claim| {
        me.is_some_and(|me| claim.agent == me.name() && claim.session.as_deref() == me.session())
    };
    claims
        .held
        .iter()
        .filter(|claim| !own(claim))
        .filter(|claim| !claim.paths.is_empty() || claim.branch.is_some())
        .cloned()
        .collect()
}

/// Each of `files`, from the root at `dir`, that one of `others` covers: by
/// a path it names, else as committed on its branch, which git is asked about
/// once per branch and only when a path does not cover the file. A file on a
/// branch one session holds several Claims on is told once, with the first.
pub(crate) fn overlaps(dir: &Path, others: &[Claim], files: &[String]) -> Check {
    let mut check = Check::default();
    let mut default: Option<Result<String, String>> = None;
    let mut committed: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut told = BTreeSet::new();
    for claim in others {
        for file in files {
            let why = if let Some(path) = claim.paths.iter().find(|path| paths::covers(path, file))
            {
                Why::Path(path.clone())
            } else if let Some(branch) = &claim.branch {
                check.branches = true;
                if !committed.contains_key(branch) {
                    let default = default.get_or_insert_with(|| {
                        scan::default_branch(dir)
                            .map(|branch| branch.full)
                            .map_err(said)
                    });
                    let files = match default {
                        Ok(default) => branch_files(dir, default, branch).map_err(|why| {
                            format!(
                                "cannot tell which files {} commits: {why}",
                                printable(branch)
                            )
                        }),
                        Err(why) => Err(format!(
                            "cannot tell which files Claims' branches commit: {why}"
                        )),
                    };
                    committed.insert(
                        branch.clone(),
                        files.unwrap_or_else(|unchecked| {
                            if !check.unchecked.contains(&unchecked) {
                                check.unchecked.push(unchecked);
                            }
                            BTreeSet::new()
                        }),
                    );
                }
                let holder = (&claim.agent, &claim.on_behalf_of, &claim.session);
                if !committed[branch].contains(file) || !told.insert((holder, branch, file)) {
                    continue;
                }
                Why::Branch(branch.clone())
            } else {
                continue;
            };
            check.found.push(Overlap {
                claim: claim.clone(),
                file: file.clone(),
                why,
            });
        }
    }
    check
}

/// The files committed on `branch` and not on `default`, both full refs but
/// `branch`; none for the default branch itself or one this clone does not
/// have.
fn branch_files(dir: &Path, default: &str, branch: &str) -> Result<BTreeSet<String>, String> {
    let full = format!("refs/heads/{branch}");
    if full == default {
        return Ok(BTreeSet::new());
    }
    if !has_branch(dir, &full)? {
        return Ok(BTreeSet::new());
    }
    names(dir, &format!("{default}...{full}"))
}

/// Whether the branch `full` names is here. A missing ref and a broken one
/// fail alike, and only the broken one writes to stderr (so trace is off); a
/// ref to a missing commit is here and fails at the diff.
fn has_branch(dir: &Path, full: &str) -> Result<bool, String> {
    let mut command = std::process::Command::new("git");
    command
        .args(["rev-parse", "--verify", "--quiet", full])
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_TRACE") {
            command.env_remove(name);
        }
    }
    let output = command
        .output()
        .map_err(|err| format!("cannot run git: {err}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    match (output.status.success(), stderr.trim()) {
        (true, _) => Ok(true),
        (false, "") => Ok(false),
        (false, why) => Err(why.to_owned()),
    }
}

/// The files `git diff` names for `range`, as stored, however git would quote
/// them for a terminal.
fn names(dir: &Path, range: &str) -> Result<BTreeSet<String>, String> {
    let said_by_git = scan::git(dir, &["diff", "-z", "--name-only", "--no-renames", range]);
    Ok(said_by_git
        .map_err(said)?
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect())
}

/// The files the checkout at `dir` changes: committed since the default
/// branch, staged, unstaged or untracked, with what git could not list.
pub(crate) fn changed(dir: &Path) -> (Vec<String>, Vec<String>) {
    let mut files = BTreeSet::new();
    let mut unchecked = Vec::new();
    let committed = scan::default_branch(dir)
        .map_err(said)
        .and_then(|default| names(dir, &format!("{}...HEAD", default.full)));
    match committed {
        Ok(names) => files.extend(names),
        Err(why) => unchecked.push(format!("cannot list your committed changes: {why}")),
    }
    let status = scan::git(
        dir,
        &["status", "--porcelain", "-z", "--untracked-files=all"],
    );
    match status {
        Ok(status) => files.extend(porcelain(&status)),
        Err(err) => unchecked.push(format!(
            "cannot list your uncommitted changes: {}",
            said(err)
        )),
    }
    (files.into_iter().collect(), unchecked)
}

/// An error as one line, with its fix when it has one.
fn said(err: CliError) -> String {
    match err.fix {
        Some(fix) => format!("{} ({fix})", err.message),
        None => err.message,
    }
}

/// The paths `git status --porcelain -z` names, a rename's new name and not
/// the old one that follows it.
fn porcelain(status: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut entries = status.split('\0').filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        let (code, path) = entry.split_at(entry.len().min(3));
        paths.push(path.to_owned());
        if code.contains(['R', 'C']) {
            entries.next();
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_names_each_changed_file_once() {
        let status = " M src/lib.rs\0R  new.rs\0old.rs\0?? docs/a b.md\0A  added.rs\0";
        assert_eq!(
            porcelain(status),
            ["src/lib.rs", "new.rs", "docs/a b.md", "added.rs"]
        );
        assert_eq!(porcelain(""), Vec::<String>::new());
    }

    #[test]
    fn a_file_is_placed_from_the_root_through_links_and_before_it_exists() {
        let dir = std::env::temp_dir().join(format!("tm-covering-{}", uuid::Uuid::now_v7()));
        let root = dir.join("repo");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        let real = std::fs::canonicalize(&root).unwrap();
        let cases = [
            (root.as_path(), real.join("src/x.rs"), Some("src/x.rs")),
            (link.as_path(), real.join("src/x.rs"), Some("src/x.rs")),
            (
                root.as_path(),
                link.join("src/new/deep.rs"),
                Some("src/new/deep.rs"),
            ),
            (root.as_path(), dir.join("elsewhere.rs"), None),
            (root.as_path(), root.join("src/../../elsewhere.rs"), None),
        ];
        for (at, file, want) in cases {
            let got = in_repo(at, Path::new("/"), file.to_str().unwrap());
            assert_eq!(
                got.as_deref(),
                want,
                "{} from {}",
                file.display(),
                at.display()
            );
        }
        assert_eq!(
            in_repo(&root, &root, "src/rel.rs").as_deref(),
            Some("src/rel.rs"),
            "a relative path is read from the base"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
