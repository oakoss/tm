//! What `tm init` offers for Issues kept elsewhere: a `bd export` for a new
//! Workspace, and an export restoring the Workspace a fresh clone names.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use serde_json::json;
use tm_core::Actor;
use tm_store::{BeadsExport, Store};

use crate::error::CliError;
use crate::output::{self, Output};
use crate::{export, fields};

pub(crate) const MAKE_BEADS: &str = "bd export -o beads.jsonl";
pub(crate) const IMPORT_BEADS: &str = "tm import --from beads beads.jsonl";

/// Reads a line for a question; `None` at end of input.
pub(crate) trait Prompt: FnMut(&str) -> Result<Option<String>, String> {}
impl<F: FnMut(&str) -> Result<Option<String>, String>> Prompt for F {}

/// What `tm init` does about a repo's `.beads/`.
#[derive(Debug)]
pub(crate) enum Beads {
    /// The repo has no `.beads/` directory.
    Absent,
    /// Not asked, or the person skipped it: the commands are printed.
    Told,
    /// A `bd export` read and checked, written once the Workspace exists.
    Chosen { file: String, export: BeadsExport },
}

/// Why a typed path was not taken: the file, so ask again, or anything else,
/// which stops init.
enum Refused {
    Again(String),
    Stop(CliError),
}

/// For a repo with `.beads/`, asks a person for a `bd export` to import into
/// Project `key`, checking it before anything is written; anyone else is told
/// the commands. End of input stops init.
pub(crate) fn beads(
    root: &Path,
    key: &str,
    prompt: Option<impl Prompt>,
) -> Result<Beads, CliError> {
    if !root.join(".beads").is_dir() {
        return Ok(Beads::Absent);
    }
    let Some(mut prompt) = prompt else {
        return Ok(Beads::Told);
    };
    let question = format!(
        "This repo has .beads/; {MAKE_BEADS} makes an export of its Issues.\nPath to a bd export to import (Enter to skip): "
    );
    let chosen = ask_path(&mut prompt, &question, |file| {
        let bytes =
            fs::read(file).map_err(|err| Refused::Again(format!("cannot read {file}: {err}")))?;
        tm_store::read_beads(&mut bytes.as_slice(), key)
            .map_err(|err| Refused::Again(format!("{file} is not a bd export: {err}")))
    })?;
    Ok(match chosen {
        Some((file, export)) => Beads::Chosen { file, export },
        None => Beads::Told,
    })
}

/// Writes the chosen export into the new Workspace's store, as `tm import
/// --from beads` would, and reports it; anything else is reported as told.
pub(crate) fn write_beads(output: &mut Output, store: &mut Store, beads: Beads, runner: &Actor) {
    let (file, export) = match beads {
        Beads::Absent => return,
        Beads::Told => return told(output),
        Beads::Chosen { file, export } => (file, export),
    };
    match store.write_beads(export, runner) {
        Ok(beads) => {
            let undo = export::undo_states(store, runner, &beads.imported);
            let imported = export::records_output(
                &beads.imported,
                Some(&beads.dropped),
                false,
                &undo,
                runner.name(),
            );
            output.text.push_str(&imported.text);
            output.data["beads"] = json!({
                "state": "imported",
                "file": file,
                "commands": [],
                "import": imported.data,
            });
            output.warnings.extend(imported.warnings);
        }
        Err(err) => {
            let err = CliError::from(err);
            output.warnings.push(format!(
                "the Workspace was made, but importing {file} failed: {}",
                err.message
            ));
            // An absolute path runs from anywhere and never reads as a flag.
            let path =
                std::path::absolute(&file).map_or(file.clone(), |path| path.display().to_string());
            let retry = format!("tm import --from beads {}", fields::quoted(&path));
            output.text.push_str("To try the import again, run:\n");
            let _ = writeln!(output.text, "  {retry}");
            output.data["beads"] = json!({
                "state": "failed",
                "file": file,
                "commands": [retry],
                "error": err.message,
                "written": err.details.get("written").cloned().unwrap_or_else(|| json!([])),
            });
        }
    }
}

/// The commands that bring beads' Issues in, for whoever runs them next.
fn told(output: &mut Output) {
    let _ = write!(
        output.text,
        "To bring beads' Issues into tm, run:\n  {MAKE_BEADS}\n  {IMPORT_BEADS}\n"
    );
    output.data["beads"] = json!({
        "state": "to_import",
        "file": null,
        "commands": [MAKE_BEADS, IMPORT_BEADS],
    });
}

/// For a repo file naming a Workspace this machine lacks, asks a person for
/// an export of it and restores it as `tm import` does. `None` means skipped.
pub(crate) fn restore(
    file: &Path,
    workspace: &str,
    mut prompt: impl Prompt,
) -> Result<Option<Output>, CliError> {
    let question = format!(
        "{} names Workspace {workspace}, which this machine lacks.\nPath to an export of it (Enter to skip): ",
        file.display()
    );
    let restored = ask_path(&mut prompt, &question, |path| {
        // `tm import -` reads stdin, which here would wait with no prompt.
        if path == "-" {
            return Err(Refused::Again(
                "- is not a file; give the export's path".into(),
            ));
        }
        export::import(path, false).map_err(|err| match err.code {
            // Its own advice, a records import into this Workspace, cannot
            // work while the Workspace is missing.
            "workspace_mismatch" => Refused::Again(format!(
                "{path} is an export of Workspace {}, not {workspace}",
                err.details
                    .get("given")
                    .and_then(|id| id.as_str())
                    .unwrap_or("another")
            )),
            _ if about_file(&err, path) => Refused::Again(err.message),
            _ => Refused::Stop(err),
        })
    })?;
    Ok(restored.map(|(_, output)| output))
}

/// Whether `tm import` refused the file at `path` itself, rather than failed
/// for a reason another file would not change.
fn about_file(err: &CliError, path: &str) -> bool {
    match err.code {
        "invalid_export" | "workspace_mismatch" | "export_format_changed" => true,
        "io" => err.details.get("path") == Some(&json!(path)),
        _ => false,
    }
}

/// Asks until `check` accepts a typed path or Enter skips, saying why a path
/// was refused.
fn ask_path<T>(
    prompt: &mut impl Prompt,
    question: &str,
    mut check: impl FnMut(&str) -> Result<T, Refused>,
) -> Result<Option<(String, T)>, CliError> {
    let mut asking = question.to_owned();
    loop {
        let typed = prompt(&asking)
            .and_then(|typed| typed.ok_or_else(crate::init::stopped))
            .map_err(crate::init::refused)?;
        if typed.is_empty() {
            return Ok(None);
        }
        match check(&typed) {
            Ok(value) => return Ok(Some((typed, value))),
            // The reason may quote the file, so it is escaped for the terminal.
            Err(Refused::Again(reason)) => {
                asking = format!("{}\n{question}", output::printable(&reason));
            }
            Err(Refused::Stop(err)) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use uuid::Uuid;

    use super::*;

    fn scratch(beads: bool) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tm-recover-{}", Uuid::now_v7()));
        fs::create_dir_all(dir.join(if beads { ".beads" } else { "src" })).unwrap();
        dir
    }

    fn fixture() -> String {
        format!(
            "{}/../tm-store/tests/fixtures/beads.jsonl",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    /// Answers each question with the next of `typed`, recording what was
    /// asked; `None` stands for end of input.
    fn scripted<'a>(typed: &'a [Option<&'a str>], asked: &'a mut Vec<String>) -> impl Prompt + 'a {
        let mut typed = typed.iter();
        move |question: &str| {
            asked.push(question.to_owned());
            Ok(typed.next().expect("asked too often").map(str::to_owned))
        }
    }

    #[test]
    fn only_a_repo_with_a_beads_directory_is_offered_the_import() {
        let dir = scratch(false);
        let mut asked = Vec::new();
        let absent = beads(&dir, "TM", Some(scripted(&[], &mut asked))).unwrap();
        assert!(matches!(absent, Beads::Absent), "{absent:?}");
        fs::remove_dir_all(&dir).unwrap();

        let dir = scratch(true);
        let told = beads(&dir, "TM", None::<fn(&str) -> _>).unwrap();
        assert!(matches!(told, Beads::Told), "nobody is asked: {told:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_refused_file_is_asked_again_and_enter_skips() {
        let dir = scratch(true);
        let junk = dir.join("junk.jsonl");
        fs::write(&junk, "not json\n").unwrap();
        let junk = junk.to_str().unwrap();
        let file = fixture();
        let mut asked = Vec::new();
        let chosen = beads(
            &dir,
            "TM",
            Some(scripted(
                &[Some("missing.jsonl"), Some(junk), Some(&file)],
                &mut asked,
            )),
        )
        .unwrap();
        assert!(
            matches!(&chosen, Beads::Chosen { file: chosen, .. } if *chosen == file),
            "{chosen:?}"
        );
        assert!(asked[0].ends_with("Path to a bd export to import (Enter to skip): "));
        assert!(
            asked[1].starts_with("cannot read missing.jsonl: "),
            "{asked:?}"
        );
        assert!(
            asked[2].starts_with(&format!("{junk} is not a bd export: ")),
            "{asked:?}"
        );
        assert!(
            asked[2].ends_with(&asked[0]) && !asked[2].contains("missing.jsonl"),
            "only the latest reason is shown"
        );

        let mut asked = Vec::new();
        let _ = beads(
            &dir,
            "TM",
            Some(scripted(&[Some("\u{1b}[2Jgone"), Some("")], &mut asked)),
        );
        assert!(
            asked[1].starts_with("cannot read \\u{1b}[2Jgone"),
            "a reason quoting the input cannot drive the terminal: {asked:?}"
        );

        let mut asked = Vec::new();
        let skipped = beads(&dir, "TM", Some(scripted(&[Some("")], &mut asked))).unwrap();
        assert!(matches!(skipped, Beads::Told), "{skipped:?}");

        let mut asked = Vec::new();
        let stopped = beads(&dir, "TM", Some(scripted(&[None], &mut asked)));
        assert_eq!(
            stopped.unwrap_err().message,
            "stopped at the prompt; nothing written"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_a_refusal_of_the_file_asks_for_another() {
        let io = |path: &str| CliError::new("io", "x").with("path", path);
        assert!(about_file(&CliError::new("invalid_export", "x"), "a"));
        assert!(about_file(&CliError::new("workspace_mismatch", "x"), "a"));
        assert!(about_file(
            &CliError::new("export_format_changed", "x"),
            "a"
        ));
        assert!(about_file(&io("a"), "a"));
        assert!(!about_file(&io("/home/workspaces"), "a"), "tm's own home");
        assert!(!about_file(&CliError::new("busy", "x"), "a"));
    }

    #[test]
    fn a_chosen_export_is_written_and_reported() {
        let dir = scratch(true);
        let runner = Actor::human("jace");
        let mut store = Store::open(&dir.join("store.db")).unwrap();
        store.create_project(&runner, "TM", "Task").unwrap();
        let file = fixture();
        let export = tm_store::read_beads(&mut fs::read(&file).unwrap().as_slice(), "TM").unwrap();
        let mut output = Output::new(String::new(), json!({}));
        write_beads(
            &mut output,
            &mut store,
            Beads::Chosen {
                file: file.clone(),
                export,
            },
            &runner,
        );
        assert_eq!(output.data["beads"]["state"], "imported");
        assert_eq!(output.data["beads"]["file"], file);
        assert_eq!(output.data["beads"]["commands"], json!([]));
        assert_eq!(output.data["beads"]["import"]["created"], 16);
        assert!(
            output.text.starts_with("imported 16 records: 16 created"),
            "{}",
            output.text
        );
        let batch = &output.data["beads"]["import"]["records"][0]["batches"][0];
        assert!(
            output
                .text
                .contains(&format!("undo with tm undo {}", batch.as_str().unwrap())),
            "the person learns how to undo the import: {}",
            output.text
        );
        assert!(
            output.text.contains("not kept: dependency created_at (35)"),
            "{}",
            output.text
        );
        assert!(
            output
                .warnings
                .iter()
                .any(|w| w.starts_with("a record import keeps")),
            "{:?}",
            output.warnings
        );

        let elsewhere =
            tm_store::read_beads(&mut fs::read(&file).unwrap().as_slice(), "ZZ").unwrap();
        let mut empty = Store::open(&dir.join("empty.db")).unwrap();
        let mut failed = Output::new(String::new(), json!({}));
        let typed = "my exports/beads.jsonl";
        write_beads(
            &mut failed,
            &mut empty,
            Beads::Chosen {
                file: typed.to_owned(),
                export: elsewhere,
            },
            &runner,
        );
        let beads = &failed.data["beads"];
        let cwd = std::env::current_dir().unwrap();
        let retry = format!("tm import --from beads '{}/{typed}'", cwd.display());
        assert_eq!(beads["state"], "failed");
        assert_eq!(beads["file"], typed);
        assert_eq!(
            beads["commands"],
            json!([retry]),
            "one shell word, from anywhere"
        );
        assert!(beads["written"].is_array(), "{beads}");
        assert!(!beads["error"].as_str().unwrap().is_empty(), "{beads}");
        assert!(
            failed
                .text
                .contains(&format!("To try the import again, run:\n  {retry}\n")),
            "{}",
            failed.text
        );
        assert!(
            failed.warnings[0].starts_with(&format!(
                "the Workspace was made, but importing {typed} failed: "
            )),
            "{:?}",
            failed.warnings
        );

        let mut told = Output::new(String::new(), json!({}));
        write_beads(&mut told, &mut store, Beads::Told, &runner);
        assert_eq!(told.data["beads"]["state"], "to_import");
        let mut absent = Output::new(String::new(), json!({}));
        write_beads(&mut absent, &mut store, Beads::Absent, &runner);
        assert_eq!(absent.data, json!({}));
        assert_eq!(absent.text, "");
        fs::remove_dir_all(&dir).unwrap();
    }
}
