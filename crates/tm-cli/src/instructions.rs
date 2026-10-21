//! The lines `tm init` offers a repo's instruction file, pointing Agents at
//! `tm guide`, and the `SessionStart` hook it prints (ADR 0014). tm writes the
//! lines only on a human's yes; an Agent is given them to apply itself.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::Path;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::output::Output;

const START: &str = "<!-- tm:start -->";
const END: &str = "<!-- tm:end -->";
const LINE: &str = "This repo tracks its work in tm. Run `tm guide` for how to work with it.";
const SETTINGS: &str = ".claude/settings.json";
const LOCAL_SETTINGS: &str = ".claude/settings.local.json";

fn block() -> String {
    format!("{START}\n{LINE}\n{END}\n")
}

/// What became of the lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// The file could not be read, so it was left alone.
    Unreadable,
    /// A human said yes, but the write failed.
    WriteFailed,
}

impl State {
    fn id(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Added => "added",
            Self::Declined => "declined",
            Self::ToAdd => "to_add",
            Self::ToReplace => "to_replace",
            Self::Malformed => "malformed",
            Self::Linked => "linked",
            Self::Unreadable => "unreadable",
            Self::WriteFailed => "write_failed",
        }
    }
}

/// The offer's outcome, for [`report`].
pub(crate) struct Offer {
    file: &'static str,
    create: bool,
    state: State,
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

/// `current` with the lines between their markers, or `None` when it has
/// them already.
///
/// # Errors
///
/// When the markers are out of order or appear more than once, since
/// replacing between them could cut the owner's own text.
fn splice(current: &str) -> Result<Option<String>, String> {
    let lines = block();
    match (current.matches(START).count(), current.matches(END).count()) {
        (0, 0) if current.is_empty() => Ok(Some(lines)),
        (0, 0) => {
            let gap = if current.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            Ok(Some(format!("{current}{gap}{lines}")))
        }
        (1, 1) => {
            let start = current.find(START).unwrap_or_default();
            let end = current.find(END).unwrap_or_default() + END.len();
            if end < start + START.len() {
                return Err(format!("{END} comes before {START}"));
            }
            let spliced = format!(
                "{}{}{}",
                &current[..start],
                lines.trim_end_matches('\n'),
                &current[end..]
            );
            Ok((spliced != current).then_some(spliced))
        }
        _ => Err(format!("{START} and {END} must each appear once")),
    }
}

/// The lines a splice removes and adds, as a diff a human confirms.
fn diff(file: &str, create: bool, before: &str, after: &str) -> String {
    let old = if create {
        "/dev/null".to_owned()
    } else {
        format!("a/{file}")
    };
    let mut out = format!("--- {old}\n+++ b/{file}\n");
    let (before, after): (Vec<&str>, Vec<&str>) =
        (before.lines().collect(), after.lines().collect());
    let same_start = before
        .iter()
        .zip(&after)
        .take_while(|(a, b)| a == b)
        .count();
    let same_end = before[same_start..]
        .iter()
        .rev()
        .zip(after[same_start..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    for line in &before[same_start..before.len() - same_end] {
        let _ = writeln!(out, "-{line}");
    }
    for line in &after[same_start..after.len() - same_end] {
        let _ = writeln!(out, "+{line}");
    }
    out
}

/// Offers the lines for the repo at `root`. When `asks`, a human sees the
/// diff and `ask` decides; otherwise the reader is only told, and nothing is
/// written. A failure leaves the file alone and is reported, never fatal.
pub(crate) fn offer(
    root: &Path,
    asks: bool,
    ask: impl FnOnce(&str, &str) -> Result<bool, String>,
) -> Offer {
    let (file, meta) = target(root);
    let path = root.join(file);
    let create = meta.is_none();
    let mut warnings = Vec::new();
    let hook = needs_hook(root, &mut warnings);
    let read = match &meta {
        None => Ok(String::new()),
        Some(meta) if meta.file_type().is_symlink() => Err(State::Linked),
        Some(_) => fs::read_to_string(&path).map_err(|err| {
            warnings.push(format!("cannot read {file}: {err}"));
            State::Unreadable
        }),
    };
    let readonly = meta
        .as_ref()
        .is_some_and(|meta| meta.permissions().readonly());
    // What the reader is told to do with the lines when tm does not write them.
    let told = |current: &str| {
        if current.contains(START) {
            State::ToReplace
        } else {
            State::ToAdd
        }
    };
    let state = match read.map(|current| (splice(&current), current)) {
        Err(state) => state,
        Ok((Err(_), _)) => State::Malformed,
        Ok((Ok(None), _)) => State::Present,
        Ok((Ok(Some(_)), current)) if !asks => told(&current),
        Ok((Ok(Some(after)), current)) => {
            let question = if create {
                format!("Create {file} with tm's lines?")
            } else {
                format!("Add tm's lines to {file}?")
            };
            match ask(&diff(file, create, &current, &after), &question) {
                Ok(false) => State::Declined,
                // A rename would replace the file whatever its mode says.
                Ok(true) if readonly => {
                    warnings.push(format!("{file} is read-only, so tm left it alone"));
                    State::WriteFailed
                }
                Ok(true) => match write(&path, &after, create) {
                    Ok(()) => State::Added,
                    Err(err) => {
                        warnings.push(format!("writing {file} failed: {err}"));
                        State::WriteFailed
                    }
                },
                Err(err) => {
                    warnings.push(format!("cannot ask about {file}: {err}"));
                    told(&current)
                }
            }
        }
    };
    Offer {
        file,
        create,
        state,
        hook,
        warnings,
    }
}

/// Writes `text` to `path` whole or not at all: a new file only where none
/// is, an existing one by renaming a finished copy over it with its
/// permissions, so a failure midway never cuts the owner's text short.
fn write(path: &Path, text: &str, create: bool) -> io::Result<()> {
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

/// Whether neither Claude Code settings file has a hook running `tm prime`.
/// One that cannot be read is named in `warnings`, and the hook is printed.
fn needs_hook(root: &Path, warnings: &mut Vec<String>) -> bool {
    let mut found = false;
    for file in [SETTINGS, LOCAL_SETTINGS] {
        match fs::read_to_string(root.join(file)) {
            Ok(text) => found |= text.contains("tm prime"),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => warnings.push(format!("cannot read {file}: {err}")),
        }
    }
    !found
}

fn hook_settings() -> Value {
    json!({
        "hooks": {
            "SessionStart": [
                { "hooks": [{ "type": "command", "command": "tm prime" }] }
            ]
        }
    })
}

/// Adds the offer to `output`: what happened to the lines, the hook to add,
/// and which files to commit, `wrote` among them.
pub(crate) fn report(output: &mut Output, offer: Offer, wrote: &[&str]) {
    let Offer {
        file,
        create,
        state,
        hook,
        warnings,
    } = offer;
    output.warnings.extend(warnings);
    let text = &mut output.text;
    let lines = block();
    let _ = match state {
        State::Present | State::Declined => Ok(()),
        State::Added => writeln!(text, "Added tm's lines to {file}"),
        State::ToAdd if create => write!(text, "Create {file} with these lines:\n{lines}"),
        State::ToAdd | State::Unreadable | State::WriteFailed => {
            write!(text, "Add these lines to {file}:\n{lines}")
        }
        State::ToReplace => write!(
            text,
            "Replace the lines between tm's markers in {file} with:\n{lines}"
        ),
        State::Malformed => write!(
            text,
            "{file} has tm's markers out of order or more than once, so it was left alone; \
             replace them with:\n{lines}"
        ),
        State::Linked => write!(
            text,
            "{file} is a symlink, so tm left it alone; add these lines to the file it \
             points to:\n{lines}"
        ),
    };
    if hook {
        let settings =
            serde_json::to_string_pretty(&hook_settings()).expect("a json! value serializes");
        let _ = writeln!(
            text,
            "For Claude Code to start each session from tm prime, add to {SETTINGS}:\n{settings}"
        );
    }
    let mut commit: Vec<&str> = wrote.to_vec();
    if !matches!(state, State::Present | State::Declined) {
        commit.push(file);
    }
    if !commit.is_empty() {
        let _ = writeln!(text, "Commit {}", commit.join(" and "));
    }
    output.data["instructions"] = json!({
        "file": file,
        "create": create,
        "lines": lines,
        "state": state.id(),
    });
    output.data["hook"] = if hook {
        json!({ "file": SETTINGS, "settings": hook_settings() })
    } else {
        Value::Null
    };
    output.data["commit"] = json!(commit);
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

    fn never(_: &str, _: &str) -> Result<bool, String> {
        panic!("only a human is asked")
    }

    fn reported(offer: Offer, wrote: &[&str]) -> Output {
        let mut output = Output::new(String::new(), json!({}));
        report(&mut output, offer, wrote);
        output
    }

    #[test]
    fn the_lines_go_after_the_owners_text_once() {
        assert_eq!(splice("").unwrap(), Some(block()));
        assert_eq!(
            splice("# Repo\n").unwrap(),
            Some(format!("# Repo\n\n{}", block()))
        );
        assert_eq!(
            splice("# Repo").unwrap(),
            Some(format!("# Repo\n\n{}", block()))
        );
        let once = splice("# Repo\n").unwrap().unwrap();
        assert_eq!(splice(&once).unwrap(), None, "a rerun changes nothing");
    }

    #[test]
    fn old_lines_between_the_markers_are_replaced_and_nothing_else() {
        let old = format!("# Repo\n\n{START}\nold pointer\n{END}\n\n## More\n");
        let new = splice(&old).unwrap().unwrap();
        assert_eq!(
            new,
            format!("# Repo\n\n{START}\n{LINE}\n{END}\n\n## More\n")
        );
        assert_eq!(
            diff("AGENTS.md", false, &old, &new),
            format!("--- a/AGENTS.md\n+++ b/AGENTS.md\n-old pointer\n+{LINE}\n")
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

        let told = offer(&dir, false, never);
        assert_eq!((told.file, told.state), ("CLAUDE.md", State::ToAdd));
        let declined = offer(&dir, true, |_, _| Ok(false));
        assert_eq!(declined.state, State::Declined);
        assert_eq!(fs::read_to_string(&path).unwrap(), "# Repo\n");
        let output = reported(declined, &[]);
        assert_eq!(
            output.data["commit"],
            json!([]),
            "a no leaves nothing to commit"
        );
        assert!(!output.text.contains("Commit"), "{}", output.text);

        let mut shown = String::new();
        let added = offer(&dir, true, |diff, question| {
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
        let output = reported(added, &[".config/tm.toml"]);
        assert!(
            output.text.starts_with("Added tm's lines to CLAUDE.md\n"),
            "{}",
            output.text
        );
        assert_eq!(
            output.data["commit"],
            json!([".config/tm.toml", "CLAUDE.md"])
        );
        assert_eq!(offer(&dir, true, never).state, State::Present);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_new_file_is_offered_as_created() {
        let dir = scratch();
        let mut asked = (String::new(), String::new());
        let made = offer(&dir, true, |diff, question| {
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
        let output = reported(offer(&dir, false, never), &[]);
        assert_eq!(output.data["instructions"]["create"], true);
        assert!(
            output
                .text
                .starts_with(&format!("Create AGENTS.md with these lines:\n{}", block())),
            "{}",
            output.text
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_older_block_is_given_to_an_agent_to_replace() {
        let dir = scratch();
        fs::write(dir.join("AGENTS.md"), format!("{START}\nold\n{END}\n")).unwrap();
        let output = reported(offer(&dir, false, never), &[]);
        assert_eq!(output.data["instructions"]["state"], "to_replace");
        assert!(
            output
                .text
                .starts_with("Replace the lines between tm's markers in AGENTS.md with:\n"),
            "{}",
            output.text
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_symlink_is_never_written_through_even_dangling() {
        let dir = scratch();
        let outside = scratch();
        symlink(outside.join("elsewhere.md"), dir.join("AGENTS.md")).unwrap();
        fs::write(dir.join("CLAUDE.md"), "# Repo\n").unwrap();
        let linked = offer(&dir, true, never);
        assert_eq!((linked.file, linked.state), ("AGENTS.md", State::Linked));
        assert!(!outside.join("elsewhere.md").exists());
        let output = reported(linked, &[]);
        assert!(
            output.text.contains("AGENTS.md is a symlink"),
            "{}",
            output.text
        );
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn a_failure_is_a_warning_and_still_reports_the_lines_and_hook() {
        let dir = scratch();
        fs::create_dir(dir.join("AGENTS.md")).unwrap();
        let unreadable = reported(offer(&dir, true, never), &[]);
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

        fs::write(dir.join("AGENTS.md"), "# Repo\n").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let failed = offer(&dir, true, |_, _| Ok(true));
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
        let output = reported(failed, &[]);
        assert_eq!(output.data["commit"], json!(["AGENTS.md"]));
        assert!(
            output.text.starts_with("Add these lines to AGENTS.md:\n"),
            "a failed write still gives the lines: {}",
            output.text
        );

        let path = dir.join("AGENTS.md");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        let protected = offer(&dir, true, |_, _| Ok(true));
        assert_eq!(protected.state, State::WriteFailed);
        assert_eq!(
            protected.warnings,
            ["AGENTS.md is read-only, so tm left it alone"]
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "# Repo\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        fs::write(&path, format!("{START}\nold\n{END}\n")).unwrap();
        let unasked = reported(offer(&dir, true, |_, _| Err("closed".into())), &[]);
        assert_eq!(unasked.data["instructions"]["state"], "to_replace");
        assert_eq!(unasked.warnings, ["cannot ask about AGENTS.md: closed"]);
        assert_eq!(unasked.data["commit"], json!(["AGENTS.md"]));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_hook_in_either_settings_file_counts_and_an_unreadable_one_is_named() {
        let dir = scratch();
        let mut warnings = Vec::new();
        assert!(needs_hook(&dir, &mut warnings));
        fs::create_dir(dir.join(".claude")).unwrap();
        fs::write(dir.join(LOCAL_SETTINGS), r#"{"command":"tm prime"}"#).unwrap();
        assert!(!needs_hook(&dir, &mut warnings));
        assert!(warnings.is_empty(), "{warnings:?}");
        fs::remove_file(dir.join(LOCAL_SETTINGS)).unwrap();
        fs::create_dir(dir.join(SETTINGS)).unwrap();
        assert!(needs_hook(&dir, &mut warnings));
        assert!(
            warnings[0].starts_with("cannot read .claude/settings.json"),
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
