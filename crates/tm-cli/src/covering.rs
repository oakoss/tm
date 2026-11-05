//! Path Claims: which other sessions' live Claims cover a file, by a path
//! they name or as already committed on their branch.

use std::cell::OnceCell;
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

/// Each of `files`, from the checkout `git` asks in, that one of `others`
/// covers, with what git could not tell since `git` was made.
pub(crate) fn overlaps(git: &mut Git, others: &[Claim], files: &[String]) -> Check {
    let (found, branches) = matches(others, files, |branch, file| git.on_branch(branch, file));
    Check {
        found,
        unchecked: std::mem::take(&mut git.unchecked),
        branches,
    }
}

/// Each of `files` that one of `others` covers: by a path it names, else as
/// committed on its branch, which `on_branch` is asked only then. A file on a
/// branch one session holds several Claims on is told once, with the first.
/// Also whether `on_branch` was asked.
fn matches(
    others: &[Claim],
    files: &[String],
    mut on_branch: impl FnMut(&str, &str) -> bool,
) -> (Vec<Overlap>, bool) {
    let mut found = Vec::new();
    let mut asked = false;
    let mut told = BTreeSet::new();
    for claim in others {
        for file in files {
            let why = if let Some(path) = claim.paths.iter().find(|path| paths::covers(path, file))
            {
                Why::Path(path.clone())
            } else if let Some(branch) = &claim.branch {
                asked = true;
                let holder = (&claim.agent, &claim.on_behalf_of, &claim.session);
                if !on_branch(branch, file) || !told.insert((holder, branch, file)) {
                    continue;
                }
                Why::Branch(branch.clone())
            } else {
                continue;
            };
            found.push(Overlap {
                claim: claim.clone(),
                file: file.clone(),
                why,
            });
        }
    }
    (found, asked)
}

/// Git as Path Claims ask it in the checkout at `dir`: the default branch
/// and each branch's files looked up once, and what it could not tell, each
/// said once.
pub(crate) struct Git<'a> {
    dir: &'a Path,
    default: OnceCell<Option<String>>,
    branches: HashMap<String, BTreeSet<String>>,
    unchecked: Vec<String>,
}

impl<'a> Git<'a> {
    pub(crate) fn new(dir: &'a Path) -> Self {
        Self {
            dir,
            default: OnceCell::new(),
            branches: HashMap::new(),
            unchecked: Vec::new(),
        }
    }

    fn tell(&mut self, unchecked: String) {
        if !self.unchecked.contains(&unchecked) {
            self.unchecked.push(unchecked);
        }
    }

    /// The default branch's full ref; `None`, told once, when git cannot say.
    fn default(&mut self) -> Option<String> {
        if self.default.get().is_none() {
            let found = scan::default_branch(self.dir).map(|branch| branch.full);
            if let Err(err) = &found {
                self.tell(format!(
                    "cannot tell what is committed off the default branch: {}",
                    said(err)
                ));
            }
            let _ = self.default.set(found.ok());
        }
        self.default.get().cloned().flatten()
    }

    fn on_branch(&mut self, branch: &str, file: &str) -> bool {
        if !self.branches.contains_key(branch) {
            let files = self.branch_files(branch);
            self.branches.insert(branch.to_owned(), files);
        }
        self.branches[branch].contains(file)
    }

    /// The files committed on `branch` and not on the default branch; none
    /// for the default branch itself or one this clone does not have.
    fn branch_files(&mut self, branch: &str) -> BTreeSet<String> {
        let Some(default) = self.default() else {
            return BTreeSet::new();
        };
        let full = format!("refs/heads/{branch}");
        if full == default {
            return BTreeSet::new();
        }
        let files = match has_branch(self.dir, &full) {
            Ok(true) => names(self.dir, &format!("{default}...{full}")),
            Ok(false) => Ok(BTreeSet::new()),
            Err(why) => Err(why),
        };
        files.unwrap_or_else(|why| {
            self.tell(format!(
                "cannot tell which files {} commits: {why}",
                printable(branch)
            ));
            BTreeSet::new()
        })
    }

    /// The files the checkout changes: committed since the default branch,
    /// staged, unstaged or untracked.
    pub(crate) fn changed(&mut self) -> Vec<String> {
        let mut files = BTreeSet::new();
        if let Some(default) = self.default() {
            match names(self.dir, &format!("{default}...HEAD")) {
                Ok(names) => files.extend(names),
                Err(why) => self.tell(format!("cannot list your committed changes: {why}")),
            }
        }
        let status = scan::git(
            self.dir,
            &["status", "--porcelain", "-z", "--untracked-files=all"],
        );
        match status {
            Ok(status) => files.extend(porcelain(&status)),
            Err(err) => self.tell(format!(
                "cannot list your uncommitted changes: {}",
                said(&err)
            )),
        }
        files.into_iter().collect()
    }
}

/// Whether the branch `full` names is here. A missing ref and a broken one
/// fail alike, and only the broken one says why on stderr; a ref to a missing
/// commit is here and fails at the diff.
fn has_branch(dir: &Path, full: &str) -> Result<bool, String> {
    let output = scan::git_output(dir, &["rev-parse", "--verify", "--quiet", full])
        .map_err(|err| said(&err))?;
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
        .map_err(|err| said(&err))?
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect())
}

/// An error as one line, with its fix when it has one.
fn said(err: &CliError) -> String {
    match &err.fix {
        Some(fix) => format!("{} ({fix})", err.message),
        None => err.message.clone(),
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

    fn claim(session: &str, paths: &[&str], branch: Option<&str>) -> Claim {
        Claim {
            id: uuid::Uuid::now_v7(),
            issue: uuid::Uuid::now_v7(),
            agent: "claude".to_owned(),
            on_behalf_of: "jace".to_owned(),
            expires_ms: None,
            ttl_ms: None,
            session: Some(session.to_owned()),
            handed: false,
            branch: branch.map(str::to_owned),
            paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        }
    }

    fn whys(found: &[Overlap]) -> Vec<(String, Why)> {
        found
            .iter()
            .map(|overlap| (overlap.file.clone(), overlap.why.clone()))
            .collect()
    }

    #[test]
    fn a_path_covers_a_file_before_its_branch_is_asked() {
        let others = [claim("s1", &["src/**"], Some("feat/x"))];
        let files = ["src/a.rs".to_owned(), "README.md".to_owned()];
        let mut asked = Vec::new();
        let (found, branches) = matches(&others, &files, |branch, file| {
            asked.push((branch.to_owned(), file.to_owned()));
            true
        });
        assert_eq!(
            whys(&found),
            [
                ("src/a.rs".to_owned(), Why::Path("src/**".to_owned())),
                ("README.md".to_owned(), Why::Branch("feat/x".to_owned())),
            ]
        );
        assert_eq!(asked, [("feat/x".to_owned(), "README.md".to_owned())]);
        assert!(branches);

        let (_, branches) = matches(&others, &files[..1], |_, _| panic!("asked"));
        assert!(!branches, "a file a path covers asks no branch");
    }

    #[test]
    fn a_branch_file_is_told_once_per_holding_session() {
        let others = [
            claim("s1", &[], Some("feat/x")),
            claim("s1", &[], Some("feat/x")),
            claim("s2", &[], Some("feat/x")),
            claim("s3", &[], Some("feat/y")),
        ];
        let files = ["a.rs".to_owned()];
        let (found, _) = matches(&others, &files, |branch, _| branch == "feat/x");
        let sessions: Vec<_> = found
            .iter()
            .map(|overlap| overlap.claim.session.clone().unwrap())
            .collect();
        assert_eq!(sessions, ["s1", "s2"]);
    }

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
