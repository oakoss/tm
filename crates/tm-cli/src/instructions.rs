//! The lines `tm init` offers a repo's instruction file, pointing Agents at
//! `tm guide`, and its pointer to `tm setup claude` (ADR 0014). tm writes the
//! lines only on a human's yes; an Agent is given them to apply itself.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::output::Output;

const START: &str = "<!-- tm:start -->";
const END: &str = "<!-- tm:end -->";
const LINE: &str = "This repo tracks its work in tm. Run `tm guide` for how to work with it.";
const SETTINGS: &str = ".claude/settings.json";

fn block() -> String {
    ending_lines("\n")
}

fn ending_lines(eol: &str) -> String {
    format!("{START}{eol}{LINE}{eol}{END}{eol}")
}

/// What became of the lines, named in `--json` in snake case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    /// Already in the file as they are now.
    Present,
    /// Written on a human's yes.
    Added,
    /// A human said no.
    Declined,
    /// Given to an Agent or script to add.
    ToAdd,
    /// Given to an Agent or script to put in place of an older block.
    ToReplace,
    /// The file's markers are out of order or doubled, so it was left alone.
    Malformed,
    /// The file is a symlink, so tm left it alone rather than write elsewhere.
    Linked,
    /// The file is read-only, so tm left it alone without asking.
    ReadOnly,
    /// The file could not be read, so it was left alone.
    Unreadable,
    /// A human said yes, but the write failed.
    WriteFailed,
}

/// What [`splice`] makes of a file: the text it would become, if any.
#[derive(Debug, PartialEq, Eq)]
enum Splice {
    Present,
    Add(String),
    Replace(String),
}

/// What [`offer`] found in the repo, all [`plan`] needs to decide.
struct Found {
    linked: bool,
    readonly: bool,
    /// The file's text, or the state a failed read leaves it in.
    current: Result<String, State>,
}

/// What the lines need: nothing more to do, or a change a human may confirm,
/// which otherwise leaves the reader with `told`.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    Settled(State),
    Change {
        current: String,
        after: String,
        told: State,
    },
}

/// Decides from what was found, touching nothing. `replace` is whether the
/// file holds an older block, so the lines go in its place.
fn plan(found: Found) -> (Plan, bool) {
    let current = match found.current {
        Ok(current) => current,
        Err(state) => return (Plan::Settled(state), false),
    };
    let (after, told) = match splice(&current) {
        Err(_) => return (Plan::Settled(State::Malformed), false),
        Ok(Splice::Present) => return (Plan::Settled(State::Present), false),
        Ok(Splice::Add(after)) => (after, State::ToAdd),
        Ok(Splice::Replace(after)) => (after, State::ToReplace),
    };
    let replace = told == State::ToReplace;
    let settled = if found.linked {
        State::Linked
    } else if found.readonly {
        State::ReadOnly
    } else {
        return (
            Plan::Change {
                current,
                after,
                told,
            },
            replace,
        );
    };
    (Plan::Settled(settled), replace)
}

/// A change not yet written, as the file's text when read and what it would
/// become: still open, or confirmed by a human.
enum Change {
    Open(String, String),
    Confirmed(String, String),
}

/// The offer's outcome, for [`report`].
pub(crate) struct Offer {
    file: &'static str,
    path: PathBuf,
    create: bool,
    /// The file holds an older block, so the lines go in its place.
    replace: bool,
    state: State,
    change: Option<Change>,
    hook: bool,
    warnings: Vec<String>,
}

/// The file the lines belong in: `AGENTS.md` when it exists, which covers a
/// `CLAUDE.md` that only imports it; else `CLAUDE.md`; else a new `AGENTS.md`.
/// A symlink counts as there, dangling or not, so it is never passed over.
fn target(root: &Path) -> (&'static str, Option<fs::Metadata>) {
    for file in ["AGENTS.md", "CLAUDE.md"] {
        if let Ok(meta) = fs::symlink_metadata(root.join(file)) {
            return (file, Some(meta));
        }
    }
    ("AGENTS.md", None)
}

/// `current` with the lines between their markers.
///
/// # Errors
///
/// When the markers are out of order or appear more than once, since
/// replacing between them could cut the owner's own text.
fn splice(current: &str) -> Result<Splice, String> {
    // CRLF if the file has any, so new lines match it.
    let eol = if current.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let lines = ending_lines(eol);
    match (current.matches(START).count(), current.matches(END).count()) {
        (0, 0) if current.is_empty() => Ok(Splice::Add(lines)),
        (0, 0) => {
            let gap = if current.ends_with('\n') {
                eol.to_owned()
            } else {
                eol.repeat(2)
            };
            Ok(Splice::Add(format!("{current}{gap}{lines}")))
        }
        (1, 1) => {
            let start = current.find(START).unwrap_or_default();
            let end = current.find(END).unwrap_or_default() + END.len();
            if end < start + START.len() {
                return Err(format!("{END} comes before {START}"));
            }
            // Either line ending counts as present, since an editor may
            // convert the lines a person or Agent pasted.
            if current[start..end].replace("\r\n", "\n") == block().trim_end_matches('\n') {
                return Ok(Splice::Present);
            }
            let spliced = format!(
                "{}{}{}",
                &current[..start],
                lines.strip_suffix(eol).unwrap_or(&lines),
                &current[end..]
            );
            Ok(Splice::Replace(spliced))
        }
        _ => Err(format!("{START} and {END} must each appear once")),
    }
}

/// What the lines need in the repo at `root`, read and decided but neither
/// asked nor written: [`Offer::ask`] puts it to a human, and [`Offer::write`]
/// carries out a yes. Until then the reader is only told. A failure leaves
/// the file alone and is reported, never fatal.
pub(crate) fn offer(root: &Path) -> Offer {
    let (file, meta) = target(root);
    let path = root.join(file);
    let create = meta.is_none();
    let mut warnings = Vec::new();
    let hook = needs_hook(root, &mut warnings);
    // A symlink is read through, to see whether its file needs the lines,
    // but never written through.
    let linked = meta
        .as_ref()
        .is_some_and(|meta| meta.file_type().is_symlink());
    let current = match &meta {
        None => Ok(String::new()),
        Some(_) => fs::read_to_string(&path).map_err(|err| {
            if linked {
                warnings.push(format!("cannot read the file {file} points to: {err}"));
                return State::Linked;
            }
            warnings.push(format!("cannot read {file}: {err}"));
            State::Unreadable
        }),
    };
    // A rename would replace a read-only file whatever its mode says.
    let readonly = meta
        .as_ref()
        .is_some_and(|meta| meta.permissions().readonly());
    let (planned, replace) = plan(Found {
        linked,
        readonly,
        current,
    });
    let (state, change) = match planned {
        Plan::Settled(state) => (state, None),
        Plan::Change {
            current,
            after,
            told,
        } => (told, Some(Change::Open(current, after))),
    };
    Offer {
        file,
        path,
        create,
        replace,
        state,
        change,
        hook,
        warnings,
    }
}

impl Offer {
    /// Puts an open change to a human, who sees its diff; `ask` decides.
    pub(crate) fn ask(&mut self, ask: impl FnOnce(&str, &str) -> Result<bool, String>) {
        let Some(Change::Open(current, after)) = self.change.take() else {
            return;
        };
        let file = self.file;
        let question = if self.create {
            format!("Create {file} with tm's lines?")
        } else {
            format!("Add tm's lines to {file}?")
        };
        let before = (!self.create).then_some(current.as_str());
        match ask(&crate::diff::unified(file, before, &after), &question) {
            Ok(true) => self.change = Some(Change::Confirmed(current, after)),
            Ok(false) => self.state = State::Declined,
            Err(err) => {
                self.warnings
                    .push(format!("cannot ask about {file}: {err}"));
                self.change = Some(Change::Open(current, after));
            }
        }
    }

    /// Writes the change a human said yes to.
    pub(crate) fn write(&mut self) {
        let Some(Change::Confirmed(current, after)) = self.change.take() else {
            return;
        };
        let file = self.file;
        // The file may have been edited since tm read it; writing the text
        // computed then would lose that edit.
        if !self.create && fs::read_to_string(&self.path).ok() != Some(current) {
            self.warnings.push(format!(
                "{file} changed after tm asked, so tm left it alone"
            ));
            return;
        }
        self.state = match write(&self.path, &after, self.create) {
            Ok(()) => State::Added,
            Err(err) => {
                self.warnings.push(format!("writing {file} failed: {err}"));
                State::WriteFailed
            }
        };
    }
}

/// Writes `text` to `path` whole or not at all: a new file only where none
/// is, an existing one by renaming a finished copy over it with its
/// permissions, so a failure midway never cuts the owner's text short.
pub(crate) fn write(path: &Path, text: &str, create: bool) -> io::Result<()> {
    if create {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let written = file
            .write_all(text.as_bytes())
            .and_then(|()| file.sync_all());
        if written.is_err() {
            let _ = fs::remove_file(path);
        }
        return written;
    }
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let copy = path.with_file_name(format!(".{name}.tm-{}", Uuid::now_v7()));
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&copy)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::set_permissions(&copy, fs::metadata(path)?.permissions())?;
        fs::rename(&copy, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&copy);
    }
    written
}

/// Whether `tm setup claude` would add any of tm's hooks. A settings file it
/// cannot read or merge into is named in `warnings`, and setup still pointed
/// at.
fn needs_hook(root: &Path, warnings: &mut Vec<String>) -> bool {
    crate::setup::needed(root).unwrap_or_else(|err| {
        warnings.push(err);
        true
    })
}

impl Offer {
    /// The instruction file, when tm wrote it or someone is to.
    pub(crate) fn needs_commit(&self) -> Option<&'static str> {
        (!matches!(self.state, State::Present | State::Declined)).then_some(self.file)
    }
}

/// Adds the offer to `output`: what happened to the lines and the hook to
/// add.
pub(crate) fn report(output: &mut Output, offer: Offer) {
    let Offer {
        file,
        create,
        replace,
        state,
        hook,
        warnings,
        ..
    } = offer;
    output.warnings.extend(warnings);
    let text = &mut output.text;
    let lines = block();
    // Where the file holds an older block, adding would leave two.
    let todo = |place: &str| {
        if replace {
            format!("replace tm's markers and the lines between them in {place} with")
        } else {
            format!("add these lines to {place}")
        }
    };
    let _ = match state {
        State::Present | State::Declined => Ok(()),
        State::Added => writeln!(text, "Added tm's lines to {file}"),
        State::ToAdd if create => write!(text, "Create {file} with these lines:\n{lines}"),
        State::ToAdd | State::ToReplace | State::Unreadable | State::WriteFailed if replace => {
            write!(
                text,
                "Replace tm's markers and the lines between them in {file} with:\n{lines}"
            )
        }
        State::ToAdd | State::ToReplace | State::Unreadable | State::WriteFailed => {
            write!(text, "Add these lines to {file}:\n{lines}")
        }
        State::Malformed => write!(
            text,
            "{file} has tm's markers out of order or more than once, so it was left alone; \
             replace them with:\n{lines}"
        ),
        State::Linked => write!(
            text,
            "{file} is a symlink, so tm left it alone; {}:\n{lines}",
            todo("the file it points to")
        ),
        State::ReadOnly => write!(
            text,
            "{file} is read-only, so tm left it alone; {}:\n{lines}",
            todo("it")
        ),
    };
    if hook {
        let _ = writeln!(
            text,
            "For Claude Code to start each session from tm prime, run tm setup claude, which adds \
             tm's hooks to {SETTINGS}"
        );
    }
    output.data["instructions"] = json!({
        "file": file,
        "create": create,
        "replace": replace,
        "lines": lines,
        "state": state,
    });
    output.data["hook"] = if hook {
        json!({ "file": SETTINGS, "setup": "tm setup claude" })
    } else {
        Value::Null
    };
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    use std::path::PathBuf;

    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tm-instructions-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_plan_needs_no_filesystem() {
        let planned = |current: Result<&str, State>, linked, readonly| {
            plan(Found {
                linked,
                readonly,
                current: current.map(str::to_owned),
            })
        };
        let change = |current: &str, after: String, told| Plan::Change {
            current: current.to_owned(),
            after,
            told,
        };
        let settled = Plan::Settled;
        let (block, repo) = (block(), "# Repo\n");
        let older = format!("{repo}{START}\nold\n{END}\n");
        let malformed = format!("{END}\n{START}\n");

        let add = change("", block.clone(), State::ToAdd);
        assert_eq!(planned(Ok(""), false, false), (add, false));
        let add = change(repo, format!("{repo}\n{block}"), State::ToAdd);
        assert_eq!(planned(Ok(repo), false, false), (add, false));
        let swap = change(&older, format!("{repo}{block}"), State::ToReplace);
        assert_eq!(planned(Ok(&older), false, false), (swap, true));
        let present = (settled(State::Present), false);
        assert_eq!(planned(Ok(&block), false, false), present);
        assert_eq!(planned(Ok(&block), true, true), present, "nothing to write");
        let malformed = planned(Ok(&malformed), false, false);
        assert_eq!(malformed, (settled(State::Malformed), false));

        let linked = (settled(State::Linked), false);
        assert_eq!(planned(Ok(repo), true, false), linked);
        let both = planned(Ok(repo), true, true);
        assert_eq!(both, linked, "a link is never written");
        assert!(planned(Ok(&older), true, false).1, "says replace");
        let readonly = settled(State::ReadOnly);
        assert_eq!(planned(Ok(repo), false, true), (readonly, false));
        assert!(planned(Ok(&older), false, true).1, "says replace");

        let unread = planned(Err(State::Unreadable), false, false);
        assert_eq!(unread, (settled(State::Unreadable), false));
    }

    /// An offer asked when `asks` and written, as `tm init` does in turn.
    fn offered(
        dir: &Path,
        asks: bool,
        ask: impl FnOnce(&str, &str) -> Result<bool, String>,
    ) -> Offer {
        let mut offered = offer(dir);
        if asks {
            offered.ask(ask);
        }
        offered.write();
        offered
    }

    fn never(_: &str, _: &str) -> Result<bool, String> {
        panic!("only a human is asked")
    }

    fn reported(offer: Offer) -> Output {
        let mut output = Output::new(String::new(), json!({}));
        report(&mut output, offer);
        output
    }

    #[test]
    fn the_lines_go_after_the_owners_text_once() {
        assert_eq!(splice("").unwrap(), Splice::Add(block()));
        assert_eq!(
            splice("# Repo\n").unwrap(),
            Splice::Add(format!("# Repo\n\n{}", block()))
        );
        assert_eq!(
            splice("# Repo").unwrap(),
            Splice::Add(format!("# Repo\n\n{}", block()))
        );
        let Splice::Add(once) = splice("# Repo\n").unwrap() else {
            panic!("the lines are added")
        };
        assert_eq!(
            splice(&once).unwrap(),
            Splice::Present,
            "a rerun changes nothing"
        );
    }

    #[test]
    fn a_crlf_file_keeps_its_line_endings() {
        let crlf = ending_lines("\r\n");
        for present in [
            format!("# Repo\r\n{crlf}"),
            format!("# Repo\r\n\r\n{}", block()),
            format!("# Repo\n{crlf}"),
        ] {
            assert_eq!(
                splice(&present).unwrap(),
                Splice::Present,
                "either line ending is present: {present:?}"
            );
        }
        assert_eq!(
            splice("# Repo\r\n").unwrap(),
            Splice::Add(format!("# Repo\r\n\r\n{crlf}"))
        );
        assert_eq!(
            splice("# Repo").unwrap(),
            Splice::Add(format!("# Repo\n\n{}", block())),
            "no CRLF seen, so LF"
        );
        let older = format!("# Repo\r\n{START}\r\nold\r\n{END}\r\n");
        assert_eq!(
            splice(&older).unwrap(),
            Splice::Replace(format!("# Repo\r\n{crlf}"))
        );
    }

    #[test]
    fn old_lines_between_the_markers_are_replaced_and_nothing_else() {
        let old = format!("# Repo\n\n{START}\nold pointer\n{END}\n\n## More\n");
        let Splice::Replace(new) = splice(&old).unwrap() else {
            panic!("an older block is replaced")
        };
        assert_eq!(
            new,
            format!("# Repo\n\n{START}\n{LINE}\n{END}\n\n## More\n")
        );
        assert_eq!(
            crate::diff::unified("AGENTS.md", Some(&old), &new),
            format!("--- a/AGENTS.md\n+++ b/AGENTS.md\n@@ -4 +4 @@\n-old pointer\n+{LINE}\n")
        );
    }

    #[test]
    fn markers_out_of_order_or_doubled_are_refused() {
        for text in [
            format!("{END}\nx\n{START}\n"),
            format!("{START}\na\n{END}\n{START}\nb\n{END}\n"),
            format!("{START}\nno end\n"),
        ] {
            assert!(splice(&text).is_err(), "{text}");
        }
    }

    #[test]
    fn only_a_human_who_says_yes_gets_the_file_written() {
        let dir = scratch();
        let path = dir.join("CLAUDE.md");
        fs::write(&path, "# Repo\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        let told = offered(&dir, false, never);
        assert_eq!((told.file, told.state), ("CLAUDE.md", State::ToAdd));
        let declined = offered(&dir, true, |_, _| Ok(false));
        assert_eq!(declined.state, State::Declined);
        assert_eq!(fs::read_to_string(&path).unwrap(), "# Repo\n");
        assert_eq!(
            declined.needs_commit(),
            None,
            "a no leaves nothing to commit"
        );

        let mut shown = String::new();
        let added = offered(&dir, true, |diff, question| {
            shown = format!("{diff}{question}");
            Ok(true)
        });
        assert_eq!(added.state, State::Added);
        assert!(
            shown.contains(&format!("+{START}\n+{LINE}\n+{END}\n")),
            "{shown}"
        );
        assert!(shown.ends_with("Add tm's lines to CLAUDE.md?"), "{shown}");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("# Repo\n\n{}", block())
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640,
            "the rewrite keeps the file's permissions"
        );
        assert_eq!(added.needs_commit(), Some("CLAUDE.md"));
        let output = reported(added);
        assert!(
            output.text.starts_with("Added tm's lines to CLAUDE.md\n"),
            "{}",
            output.text
        );
        assert_eq!(offered(&dir, true, never).state, State::Present);

        fs::write(&path, "# Repo\n").unwrap();
        let raced = offered(&dir, true, |_, _| {
            fs::write(&path, "# Repo\nedited while asked\n").unwrap();
            Ok(true)
        });
        assert_eq!(raced.state, State::ToAdd);
        assert_eq!(
            raced.warnings,
            ["CLAUDE.md changed after tm asked, so tm left it alone"]
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# Repo\nedited while asked\n",
            "an edit made while the prompt waited is kept"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_new_file_is_offered_as_created() {
        let dir = scratch();
        let mut asked = (String::new(), String::new());
        let made = offered(&dir, true, |diff, question| {
            asked = (diff.to_owned(), question.to_owned());
            Ok(true)
        });
        assert_eq!(
            (made.file, made.create, made.state),
            ("AGENTS.md", true, State::Added)
        );
        assert!(
            asked.0.starts_with("--- /dev/null\n+++ b/AGENTS.md\n"),
            "{}",
            asked.0
        );
        assert_eq!(asked.1, "Create AGENTS.md with tm's lines?");
        assert_eq!(fs::read_to_string(dir.join("AGENTS.md")).unwrap(), block());

        fs::remove_file(dir.join("AGENTS.md")).unwrap();
        let output = reported(offered(&dir, false, never));
        assert_eq!(output.data["instructions"]["create"], true);
        assert!(
            output
                .text
                .starts_with(&format!("Create AGENTS.md with these lines:\n{}", block())),
            "{}",
            output.text
        );

        // A file that appears between the yes and the write is not replaced.
        let path = dir.join("AGENTS.md");
        let mut raced = offer(&dir);
        raced.ask(|_, _| Ok(true));
        fs::write(&path, "# mine\n").unwrap();
        raced.write();
        assert_eq!(raced.state, State::WriteFailed);
        assert_eq!(fs::read_to_string(&path).unwrap(), "# mine\n");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_older_block_is_given_to_an_agent_to_replace() {
        let dir = scratch();
        let old_block = format!("{START}\nold\n{END}\n");
        fs::write(dir.join("AGENTS.md"), format!("# Repo\n\n{old_block}")).unwrap();
        let output = reported(offered(&dir, false, never));
        assert_eq!(output.data["instructions"]["state"], "to_replace");
        let told = "Replace tm's markers and the lines between them in AGENTS.md with:\n";
        assert!(output.text.starts_with(told), "{}", output.text);

        let given = &output.text[told.len()..told.len() + block().len()];
        let followed = format!("# Repo\n\n{given}");
        assert_eq!(
            splice(&followed).unwrap(),
            Splice::Present,
            "doing as told leaves the lines in place: {followed}"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_symlink_is_never_written_through_even_dangling() {
        let dir = scratch();
        let outside = scratch();
        symlink(outside.join("elsewhere.md"), dir.join("AGENTS.md")).unwrap();
        fs::write(dir.join("CLAUDE.md"), "# Repo\n").unwrap();
        let linked = offered(&dir, true, never);
        assert_eq!((linked.file, linked.state), ("AGENTS.md", State::Linked));
        assert!(
            linked.warnings[0].starts_with("cannot read the file AGENTS.md points to"),
            "{:?}",
            linked.warnings
        );
        assert!(!outside.join("elsewhere.md").exists());
        let output = reported(linked);
        assert!(
            output.text.contains("AGENTS.md is a symlink"),
            "{}",
            output.text
        );

        let real = outside.join("real.md");
        fs::remove_file(dir.join("AGENTS.md")).unwrap();
        symlink(&real, dir.join("AGENTS.md")).unwrap();
        fs::write(&real, block()).unwrap();
        assert_eq!(
            offered(&dir, true, never).state,
            State::Present,
            "a link to a file with the lines is read through"
        );
        fs::write(&real, format!("{START}\nold\n{END}\n")).unwrap();
        let older = reported(offered(&dir, true, never));
        assert_eq!(older.data["instructions"]["state"], "linked");
        assert!(
            older.text.contains(
                "left it alone; replace tm's markers and the lines between them in the file it \
                 points to"
            ),
            "{}",
            older.text
        );
        assert_eq!(
            fs::read_to_string(&real).unwrap(),
            format!("{START}\nold\n{END}\n")
        );
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn a_failure_is_a_warning_and_still_reports_the_lines_and_hook() {
        let dir = scratch();
        fs::create_dir(dir.join("AGENTS.md")).unwrap();
        let unreadable = reported(offered(&dir, true, never));
        assert_eq!(unreadable.data["instructions"]["state"], "unreadable");
        assert!(
            unreadable.warnings[0].starts_with("cannot read AGENTS.md"),
            "{:?}",
            unreadable.warnings
        );
        assert!(
            unreadable.text.contains("Add these lines to AGENTS.md:"),
            "{}",
            unreadable.text
        );
        assert!(
            unreadable.data["hook"].is_object(),
            "the hook is still given"
        );
        fs::remove_dir(dir.join("AGENTS.md")).unwrap();

        fs::write(dir.join("AGENTS.md"), format!("{START}\nold\n{END}\n")).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let replacing = reported(offered(&dir, true, |_, _| Ok(true)));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(replacing.data["instructions"]["state"], "write_failed");
        assert!(
            replacing.text.starts_with(
                "Replace tm's markers and the lines between them in AGENTS.md with:\n"
            ),
            "a failed write over an older block says to replace it: {}",
            replacing.text
        );

        fs::write(dir.join("AGENTS.md"), "# Repo\n").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let failed = offered(&dir, true, |_, _| Ok(true));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(failed.state, State::WriteFailed);
        assert!(
            failed.warnings[0].starts_with("writing AGENTS.md failed"),
            "{:?}",
            failed.warnings
        );
        assert_eq!(
            fs::read_to_string(dir.join("AGENTS.md")).unwrap(),
            "# Repo\n"
        );
        assert_eq!(failed.needs_commit(), Some("AGENTS.md"));
        let output = reported(failed);
        assert!(
            output.text.starts_with("Add these lines to AGENTS.md:\n"),
            "a failed write still gives the lines: {}",
            output.text
        );

        let path = dir.join("AGENTS.md");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        let protected = reported(offered(&dir, true, never));
        assert_eq!(protected.data["instructions"]["state"], "read_only");
        assert!(
            protected
                .text
                .starts_with("AGENTS.md is read-only, so tm left it alone; add these lines"),
            "{}",
            protected.text
        );
        assert_eq!(protected.data["instructions"]["replace"], false);
        assert_eq!(fs::read_to_string(&path).unwrap(), "# Repo\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&path, format!("{START}\nold\n{END}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        let older = reported(offered(&dir, true, never));
        assert_eq!(older.data["instructions"]["replace"], true);
        assert!(
            older
                .text
                .contains("left it alone; replace tm's markers and the lines between them in it"),
            "{}",
            older.text
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        fs::write(&path, format!("{START}\nold\n{END}\n")).unwrap();
        let unasked = offered(&dir, true, |_, _| Err("closed".into()));
        assert_eq!(unasked.needs_commit(), Some("AGENTS.md"));
        let unasked = reported(unasked);
        assert_eq!(unasked.data["instructions"]["state"], "to_replace");
        assert_eq!(unasked.warnings, ["cannot ask about AGENTS.md: closed"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn setup_is_pointed_at_until_every_hook_is_in_and_an_unreadable_file_is_named() {
        let dir = scratch();
        let mut warnings = Vec::new();
        assert!(needs_hook(&dir, &mut warnings));
        fs::create_dir(dir.join(".claude")).unwrap();
        let group = |command: &str| format!(r#"[{{"hooks":[{{"command":{command:?}}}]}}]"#);
        let prime = format!(r#"{{"hooks":{{"SessionStart":{}}}}}"#, group("tm prime"));
        fs::write(dir.join(SETTINGS), prime).unwrap();
        assert!(
            needs_hook(&dir, &mut warnings),
            "a tm prime alone lacks the heartbeat"
        );
        let all = format!(
            r#"{{"hooks":{{"SessionStart":{},"PostToolUse":{},"SessionEnd":{}}}}}"#,
            group("tm prime"),
            group(r#"cd "${CLAUDE_PROJECT_DIR:?}" && tm hook heartbeat claude"#),
            group(r#"cd "${CLAUDE_PROJECT_DIR:?}" && tm release --mine"#),
        );
        fs::write(dir.join(SETTINGS), &all).unwrap();
        assert!(!needs_hook(&dir, &mut warnings));
        assert!(warnings.is_empty(), "{warnings:?}");

        // A symlinked file is read through; only one setup must write to is
        // named, since setup leaves a link alone.
        let shared = dir.join("shared.json");
        fs::write(&shared, &all).unwrap();
        fs::remove_file(dir.join(SETTINGS)).unwrap();
        symlink(&shared, dir.join(SETTINGS)).unwrap();
        assert!(!needs_hook(&dir, &mut warnings));
        assert!(warnings.is_empty(), "{warnings:?}");
        fs::write(&shared, "{}").unwrap();
        assert!(needs_hook(&dir, &mut warnings));
        assert!(warnings.remove(0).contains("is a symlink"));

        fs::remove_file(dir.join(SETTINGS)).unwrap();
        fs::create_dir(dir.join(SETTINGS)).unwrap();
        assert!(needs_hook(&dir, &mut warnings));
        assert!(
            warnings[0].contains(".claude/settings.json"),
            "{warnings:?}"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn agents_md_wins_then_claude_md_then_a_new_agents_md() {
        let dir = scratch();
        assert_eq!(target(&dir).0, "AGENTS.md");
        assert!(target(&dir).1.is_none());
        fs::write(dir.join("CLAUDE.md"), "x").unwrap();
        assert_eq!(target(&dir).0, "CLAUDE.md");
        fs::write(dir.join("AGENTS.md"), "x").unwrap();
        assert_eq!(target(&dir).0, "AGENTS.md");
        assert!(target(&dir).1.is_some());
        fs::remove_dir_all(&dir).unwrap();
    }
}
