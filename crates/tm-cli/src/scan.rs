//! `tm git scan`: moves each Issue a `Closes` line names to done once its
//! commit is on the default branch, and comments where it shipped. tm only
//! reads git here.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};
use tm_core::scheme::Category;
use tm_core::{Actor, Change};
use tm_store::Draft;
use uuid::Uuid;

use crate::error::CliError;
use crate::fields::set;
use crate::follow;
use crate::here::{self, Here};
use crate::identity;
use crate::moves;
use crate::output::{Output, printable, short};

/// A `Closes` line on the default branch.
#[derive(Debug, PartialEq, Eq)]
struct Closing {
    key: String,
    commit: String,
    /// The pull request a squash merge's subject names, such as `(#55)`.
    pr: Option<u64>,
}

impl Closing {
    /// The commit, shortened, and its pull request, as output names them.
    fn shipped(&self) -> String {
        self.with_commit(&self.commit[..7])
    }

    fn with_commit(&self, commit: &str) -> String {
        match self.pr {
            Some(pr) => format!("{commit}, #{pr}"),
            None => commit.to_owned(),
        }
    }
}

/// The ref merged work lands on, as this clone last fetched it.
struct Branch {
    /// The full ref, so a tag of the same name cannot shadow it.
    full: String,
    shown: String,
    /// Why commits on it may not be merged yet, when they may not.
    caveat: Option<String>,
}

/// What a scan says, built up an Issue at a time.
#[derive(Default)]
struct Report {
    text: String,
    warnings: Vec<String>,
    moved: Vec<Value>,
    failed: Vec<Value>,
    next: Vec<String>,
    dry_run: bool,
}

impl Report {
    fn fail(&mut self, closing: &Closing, err: &CliError) {
        let (key, shipped) = (&closing.key, closing.shipped());
        let warning = format!("{key} was not moved for {shipped}: {}", err.message);
        self.warnings.push(warning);
        let failed = json!({ "key": key, "commit": closing.commit, "error": err.code });
        self.failed.push(failed);
    }
}

pub(crate) fn run(dry_run: bool) -> Result<Output, CliError> {
    here::write(|here| write(here, dry_run))
}

fn write(here: &mut Here, dry_run: bool) -> Result<(Output, Vec<String>), CliError> {
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    let dir = here.found.dir.clone();
    let branch = default_branch(&dir)?;
    let log = git(
        &dir,
        &[
            "log",
            "--reverse",
            "--no-show-signature",
            "--encoding=UTF-8",
            "--extended-regexp",
            "--regexp-ignore-case",
            "--grep=^[[:space:]]*closes:?[[:space:]]",
            // %B, not %s and %b: %s joins a first paragraph's lines.
            "--format=%H%n%B%x00",
            &branch.full,
            "--",
        ],
    )?;
    let mut report = Report {
        dry_run,
        ..Report::default()
    };
    report.warnings.extend(branch.caveat.clone());
    if git(&dir, &["rev-parse", "--is-shallow-repository"])?.trim() == "true" {
        let warning = "this clone is shallow, so Closes lines in commits it lacks are not seen";
        report.warnings.push(warning.into());
    }
    let (closings, unread) = closings(&log);
    report.warnings.extend(unread);
    for (id, closings) in by_issue(here, closings, &mut report.warnings)? {
        // Each Issue is its own batch, so one that fails leaves the others
        // written and reported.
        let fresh = match fresh(here, id, &closings) {
            Ok(fresh) => fresh,
            Err(err) => {
                report.fail(&closings[0], &err);
                continue;
            }
        };
        let Some(first) = fresh.first() else {
            continue;
        };
        if let Err(err) = close(here, &identity.actor, id, &fresh, &mut report) {
            report.fail(first, &err);
        }
    }
    let shown = printable(&branch.shown);
    if !report.failed.is_empty() {
        let count = report.failed.len();
        let _ = writeln!(report.text, "{count} not moved; the warnings say why");
    } else if report.moved.is_empty() {
        let nothing = "nothing to move: no new Closes line on";
        let _ = writeln!(report.text, "{nothing} {shown} names an open Issue");
    }
    if dry_run {
        let _ = writeln!(report.text, "dry run: nothing was written");
    }
    let data = json!({
        "dry_run": dry_run,
        "branch": branch.shown,
        "moved": report.moved,
        "failed": report.failed,
        "next": report.next,
    });
    let mut output = Output::new(report.text, data);
    output.warnings = report.warnings;
    Ok((output, identity.warnings))
}

/// `closings` grouped by the Issue each Key names, so an old Key and the one
/// a renamed Project gave it count as one Issue; first-seen order is kept.
fn by_issue(
    here: &Here,
    closings: Vec<Closing>,
    warnings: &mut Vec<String>,
) -> Result<Vec<(Uuid, Vec<Closing>)>, CliError> {
    let mut grouped: Vec<(Uuid, Vec<Closing>)> = Vec::new();
    for closing in closings {
        let Some(id) = here.store.issue_by_key(&closing.key)? else {
            let (prefix, _) = closing.key.rsplit_once('-').unwrap_or_default();
            // A Key of no Project here belongs to some other tracker.
            if here.store.project_by_key(prefix)?.is_some() {
                let (shipped, key) = (closing.shipped(), &closing.key);
                warnings.push(format!("{shipped} closes {key}, which is no Issue"));
            }
            continue;
        };
        match grouped.iter_mut().find(|(issue, _)| *issue == id) {
            Some((_, group)) => group.push(closing),
            None => grouped.push((id, vec![closing])),
        }
    }
    Ok(grouped)
}

/// The commits of `closings` no scan's comment on `id` records yet, each
/// once, oldest first. The record is how a later scan leaves an Issue alone
/// that an undo or a reopen took back; a retracted one still counts.
fn fresh<'a>(here: &Here, id: Uuid, closings: &'a [Closing]) -> Result<Vec<&'a Closing>, CliError> {
    let records: Vec<String> = (here.store.comments(id)?.into_iter())
        .map(|comment| comment.body)
        .filter(|body| body.starts_with(RECORD))
        .collect();
    let mut seen = BTreeSet::new();
    Ok(closings
        .iter()
        .filter(|closing| !records.iter().any(|r| r.contains(&closing.commit)))
        .filter(|closing| seen.insert(closing.commit.as_str()))
        .collect())
}

/// How a scan's comment begins.
const RECORD: &str = "shipped in ";

/// Moves the Issue `id` to done for the first of `fresh`, recording the rest
/// with it. A finished or archived Issue is left alone.
fn close(
    here: &mut Here,
    actor: &Actor,
    id: Uuid,
    fresh: &[&Closing],
    report: &mut Report,
) -> Result<(), CliError> {
    let Some((first, also)) = fresh.split_first() else {
        return Ok(());
    };
    let Some(view) = here.store.issue(id)? else {
        let key = &first.key;
        let message = format!("{key} has a Key but no Issue; tm doctor checks the store");
        return Err(CliError::new("not_found", message));
    };
    if follow::archived(&view) {
        return Ok(());
    }
    let shipped = first.shipped();
    let name = view.key.clone().unwrap_or_else(|| id.to_string());
    match view.category {
        Some(Category::Done) => return Ok(()),
        Some(Category::Cancelled) => {
            let warning = format!("{name} is cancelled, so {shipped} closing it moved nothing");
            report.warnings.push(warning);
            return Ok(());
        }
        _ => {}
    }
    let issue_type = view
        .fields
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(done) = follow::first(here.store.scheme(), issue_type, Category::Done) else {
        let message = format!("Issue type {} has no done Status", printable(issue_type));
        return Err(CliError::new("no_done_status", message));
    };
    let mut record = format!("{RECORD}{}", first.with_commit(&first.commit));
    for closing in also {
        let commit = closing.with_commit(&closing.commit);
        let _ = write!(record, "; also closed by {commit}");
    }
    let mut changes = vec![set("status", json!(done)), Change::Comment { body: record }];
    changes.extend(moves::unwait(&view));
    let drafts = || {
        changes
            .iter()
            .map(|change| Draft {
                actor: actor.clone(),
                project: view.project,
                subject: id,
                change: change.clone(),
            })
            .collect()
    };
    let batch = if report.dry_run {
        None
    } else {
        // A merged Closes line says the work is done, whoever holds it.
        let ops = moves::finish(here, actor, (view.project, id), drafts, false)?;
        Some(ops[0].op_id)
    };
    let before = view.fields.get("status").and_then(Value::as_str);
    let from = printable(before.unwrap_or("(none)"));
    let _ = writeln!(report.text, "{name}  status: {from} → {done}  ({shipped})");
    report.moved.push(json!({
        "id": id,
        "key": view.key,
        "from": before,
        "to": done,
        "commit": first.commit,
        "pr": first.pr,
        "batch": batch,
    }));
    if let Some(batch) = batch {
        landed(here, actor, (id, &name), batch, report);
    }
    Ok(())
}

/// The undo command and parent hint for a batch that has landed. Its write
/// is done, so a check that fails only warns.
fn landed(here: &Here, actor: &Actor, (id, name): (Uuid, &str), batch: Uuid, report: &mut Report) {
    let unknown = |what: &str, err: &str| format!("{name} moved, but {what} is unknown: {err}");
    match here.store.can_undo(batch) {
        Ok(true) => {
            let _ = writeln!(report.text, "{name}: undo with tm undo {}", short(batch));
        }
        Ok(false) => {}
        Err(err) => {
            let warning = unknown("whether it can be undone", &err.to_string());
            report.warnings.push(warning);
        }
    }
    match follow::next(&here.store, id, &mut report.warnings) {
        Ok(Some((message, command))) => {
            let _ = writeln!(report.text, "{message}");
            report.next.push(command);
        }
        Ok(None) => {}
        Err(err) => {
            let warning = unknown("whether its parent is done", &err.to_string());
            report.warnings.push(warning);
        }
    }
    match here.takeovers(actor, Some(batch)) {
        Ok(takeovers) => {
            let warnings = takeovers.into_iter().map(|(_, warning)| warning);
            report.warnings.extend(warnings);
        }
        Err(err) => {
            let what = "whether another Agent took over your Claims";
            report.warnings.push(unknown(what, &err.message));
        }
    }
}

/// The remote's default branch when it is known, else a local `main` or
/// `master` by its upstream, else that local branch. tm fetches nothing.
fn default_branch(dir: &Path) -> Result<Branch, CliError> {
    let head = "refs/remotes/origin/HEAD";
    let origin = git(dir, &["for-each-ref", "--format=%(symref)", head])?;
    if let Some(full) = origin.lines().next().filter(|full| !full.is_empty()) {
        return Ok(Branch {
            shown: shown(full),
            full: full.to_owned(),
            caveat: None,
        });
    }
    let format = "--format=%(refname) %(upstream) %(upstream:track)";
    let names = ["refs/heads/main", "refs/heads/master"];
    let locals = git(dir, &["for-each-ref", format, names[0], names[1]])?;
    // A pattern also matches refs below it, such as refs/heads/main/wip.
    let lines: Vec<Vec<&str>> = locals.lines().map(words).collect();
    let found = names
        .iter()
        .find_map(|name| lines.iter().find(|parts| parts.first() == Some(name)));
    if let Some(parts) = found {
        let (local, upstream) = (parts[0], parts.get(1).copied().unwrap_or_default());
        let gone = parts.get(2).is_some_and(|track| track.contains("gone"));
        if upstream.starts_with("refs/remotes/") && !gone {
            return Ok(Branch {
                shown: shown(upstream),
                full: upstream.to_owned(),
                caveat: None,
            });
        }
        let shown = shown(local);
        let why = if gone {
            format!("{}, the branch it tracks, is gone", self::shown(upstream))
        } else {
            "it tracks no remote branch".to_owned()
        };
        return Ok(Branch {
            caveat: Some(format!(
                "scanned local {shown}, since {why}, so commits not yet pushed count as merged"
            )),
            shown,
            full: local.to_owned(),
        });
    }
    let message = "cannot tell this repo's default branch: it has no origin/HEAD and no main or master branch";
    let error = CliError::new("no_branch", message);
    let remotes = git(dir, &["remote"])?;
    Err(if remotes.lines().any(|remote| remote == "origin") {
        error.fix("git remote set-head origin --auto")
    } else {
        error
    })
}

fn words(line: &str) -> Vec<&str> {
    line.split(' ').collect()
}

fn shown(full: &str) -> String {
    let shown = ["refs/remotes/", "refs/heads/"]
        .iter()
        .find_map(|prefix| full.strip_prefix(prefix));
    shown.unwrap_or(full).to_owned()
}

fn git(dir: &Path, args: &[&str]) -> Result<String, CliError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|err| CliError::new("git_failed", format!("cannot run git: {err}")))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let said = if said.is_empty() {
            String::new()
        } else {
            format!(": {said}")
        };
        return Err(CliError::new(
            "git_failed",
            format!("git {} failed ({}){said}", args[0], output.status),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// What follows `Closes` or `Closes:` on `line`, when it starts so.
fn after_closes(line: &str) -> Option<&str> {
    let rest = line
        .get(..6)
        .filter(|word| word.eq_ignore_ascii_case("closes"))
        .map(|_| &line[6..])?;
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    rest.starts_with(char::is_whitespace).then_some(rest)
}

/// Each Key a `Closes` line in a commit body names, oldest commit first. A
/// line reads `Closes TM-1`, `Closes: TM-1.` or `Closes TM-1, TM-2`; one
/// whose first word is not a Key is prose. Also returns a warning for each
/// line that goes on after its Keys, for each Closes in a subject line, and
/// for each record git printed without a commit id.
fn closings(log: &str) -> (Vec<Closing>, Vec<String>) {
    let mut seen = BTreeSet::new();
    let mut closings = Vec::new();
    let mut warnings = Vec::new();
    // git messages hold no NUL, so it ends each record.
    for record in log.split('\0') {
        let record = record.trim_start_matches('\n');
        let mut lines = record.splitn(3, '\n');
        let (Some(commit), subject) = (lines.next(), lines.next().unwrap_or_default()) else {
            continue;
        };
        if commit.is_empty() {
            continue;
        }
        if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
            let warning = format!("skipped a commit git printed as {}", printable(commit));
            warnings.push(warning);
            continue;
        }
        let pr = subject
            .trim_end()
            .strip_suffix(')')
            .and_then(|rest| rest.rsplit_once("(#"))
            .and_then(|(_, number)| number.parse().ok());
        if after_closes(subject.trim()).is_some() {
            let short = &commit[..7];
            let unread = "a Closes in the subject is not read; put it in the body";
            let warning = format!("{short}: {unread}");
            warnings.push(warning);
        }
        for line in lines.next().unwrap_or_default().lines() {
            let line = line.trim();
            let Some(rest) = after_closes(line) else {
                continue;
            };
            let words = rest.split(|c: char| c == ',' || c.is_whitespace());
            for (n, word) in words.filter(|word| !word.is_empty()).enumerate() {
                let bare = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
                let key = bare.to_ascii_uppercase();
                if tm_core::project::parse_issue_key(&key).is_none() {
                    if n > 0 {
                        warnings.push(format!(
                            "{}: read the Keys of \"{}\" up to {}, and ignored the rest",
                            &commit[..7],
                            printable(line),
                            printable(word)
                        ));
                    }
                    break;
                }
                if seen.insert((key.clone(), commit)) {
                    closings.push(Closing {
                        key,
                        commit: commit.to_owned(),
                        pr,
                    });
                }
            }
        }
    }
    (closings, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "cccccccccccccccccccccccccccccccccccccccc";
    /// Keys in punctuation, an indented line, and one Key named twice.
    const TWICE: &str = "closes: tm-2, (TM-3);\n  Closes TM-1\nCloses TM-1\n";

    fn record(commit: &str, subject: &str, body: &str) -> String {
        format!("{commit}\n{subject}\n{body}\0\n")
    }

    #[test]
    fn closes_lines_name_keys_in_commit_order() {
        let log = [
            record(A, "feat: one (#7)", "Body.\n\nCloses TM-1\nRefs TM-9\n"),
            record(B, "fix", TWICE),
            record(C, "chore", "Closes tm-u8g.4\nClosesx TM-5\nCloses\n"),
            record(C, "chore", "Closes the gap TM-6 left\nCloses `TM-4`:\n"),
            record(C, "chore", "Closes TM-7, see TM-8\n"),
            record(C, "Closes TM-10", ""),
            record("Good signature", "s", "Closes TM-9\n"),
        ]
        .concat();
        let closing = |key: &str, commit: &str, pr| Closing {
            key: key.into(),
            commit: commit.into(),
            pr,
        };
        let (closings, warnings) = closings(&log);
        assert_eq!(
            closings,
            [
                closing("TM-1", A, Some(7)),
                closing("TM-2", B, None),
                closing("TM-3", B, None),
                closing("TM-1", B, None),
                closing("TM-4", C, None),
                closing("TM-7", C, None),
            ]
        );
        let line = "\"Closes TM-7, see TM-8\"";
        let ignored = format!("ccccccc: read the Keys of {line} up to see, and ignored the rest");
        assert_eq!(warnings[0], ignored);
        let subject = "ccccccc: a Closes in the subject is not read; put it in the body";
        assert_eq!(warnings[1], subject);
        assert!(warnings[2].starts_with("skipped a commit git printed as"));
        assert_eq!(warnings.len(), 3);
    }
}
