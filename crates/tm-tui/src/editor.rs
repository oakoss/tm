//! Text written in the person's editor: a comment, or a View.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

use uuid::Uuid;

/// What an editor saved, never blank, and the file it is in, which stays
/// until the comment is written so a refused write keeps the text.
#[derive(Debug)]
pub struct Composed {
    pub text: String,
    pub draft: PathBuf,
}

/// Why an editor gave no comment, and the draft kept when it saved some text.
#[derive(Debug)]
pub struct Unwritten {
    pub why: String,
    pub kept: Option<PathBuf>,
}

/// What is written: the file it starts from, and what it is called.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Draft<'a> {
    pub(crate) start: &'a str,
    /// The file's extension, so the editor knows the language.
    pub(crate) suffix: &'a str,
    pub(crate) noun: &'a str,
}

/// A comment starts empty rather than with `#` lines to strip, since it is
/// Markdown and `#` starts a heading.
pub(crate) const COMMENT: Draft<'static> = Draft {
    start: "",
    suffix: "md",
    noun: "comment",
};

/// The text saved in `$VISUAL`, else `$EDITOR`, else `vi`, or why there is
/// none.
pub(crate) fn compose(draft: Draft<'_>) -> Result<Composed, Unwritten> {
    let editor = ["VISUAL", "EDITOR"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|editor| !editor.trim().is_empty())
        .unwrap_or_else(|| "vi".to_owned());
    compose_with(&editor, draft)
}

/// The text `editor` saves in a file holding `draft`'s start, or why there
/// is none. A blank save is nothing written, and an editor that fails
/// leaves its file only when it saved something.
pub(crate) fn compose_with(editor: &str, kind: Draft<'_>) -> Result<Composed, Unwritten> {
    let name = format!("tm-{}-{}.{}", kind.noun, Uuid::now_v7(), kind.suffix);
    let draft = std::env::temp_dir().join(name.replace(' ', "-"));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&draft).map_err(|err| Unwritten {
        why: format!("cannot make {}: {err}", draft.display()),
        kept: None,
    })?;
    if let Err(err) = std::io::Write::write_all(&mut file, kind.start.as_bytes()) {
        let _ = fs::remove_file(&draft);
        return Err(Unwritten {
            why: format!("cannot make {}: {err}", draft.display()),
            kept: None,
        });
    }
    drop(file);
    let ran = run(editor, &draft);
    let text = fs::read_to_string(&draft);
    let failed = match ran {
        Err(err) => Some(format!("cannot run {editor}: {err}")),
        Ok(status) if !status.success() => Some(format!("{editor} exited with {status}")),
        Ok(_) => None,
    };
    match (failed, text) {
        (None, Ok(text)) if !text.trim().is_empty() => Ok(Composed { text, draft }),
        (None, Ok(_)) => {
            let _ = fs::remove_file(&draft);
            Err(Unwritten {
                why: format!("the {} was empty", kind.noun),
                kept: None,
            })
        }
        (Some(why), Ok(text)) if !text.trim().is_empty() => Err(Unwritten {
            why,
            kept: Some(draft),
        }),
        (Some(why), _) => {
            let _ = fs::remove_file(&draft);
            Err(Unwritten { why, kept: None })
        }
        (None, Err(err)) => Err(Unwritten {
            why: format!("cannot read what {editor} saved: {err}"),
            kept: (err.kind() != std::io::ErrorKind::NotFound).then_some(draft),
        }),
    }
}

/// `editor` on `path` through the shell, as git runs it, so an editor with
/// arguments such as `code --wait` works.
#[cfg(unix)]
fn run(editor: &str, path: &Path) -> std::io::Result<std::process::ExitStatus> {
    Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(path)
        .status()
}

#[cfg(not(unix))]
fn run(editor: &str, path: &Path) -> std::io::Result<std::process::ExitStatus> {
    let mut words = editor.split_whitespace();
    let program = words.next().unwrap_or("notepad");
    Command::new(program).args(words).arg(path).status()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn the_text_saved_comes_back_with_its_file_private_to_its_owner() {
        let composed = compose_with("printf '# Heading\\n\\nbody' >", COMMENT).unwrap();
        assert_eq!(composed.text, "# Heading\n\nbody");
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &fs::metadata(&composed.draft).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o600);
        fs::remove_file(&composed.draft).unwrap();
    }

    #[test]
    fn an_editor_that_fails_keeps_what_it_saved() {
        let unwritten = compose_with("printf kept > \"$1\"; false", COMMENT).unwrap_err();
        assert!(unwritten.why.contains("exited with"), "{unwritten:?}");
        let kept = unwritten.kept.unwrap();
        assert_eq!(fs::read_to_string(&kept).unwrap(), "kept");
        fs::remove_file(kept).unwrap();
    }

    #[test]
    fn an_editor_that_fails_having_saved_nothing_leaves_no_file() {
        let record = std::env::temp_dir().join(format!("tm-draft-path-{}", Uuid::now_v7()));
        let editor = format!("printf %s \"$1\" > '{}'; false", record.display());
        let unwritten = compose_with(&editor, COMMENT).unwrap_err();
        assert!(unwritten.why.contains("exited with"), "{unwritten:?}");
        assert_eq!(unwritten.kept, None);
        let draft = fs::read_to_string(&record).unwrap();
        fs::remove_file(&record).unwrap();
        assert!(!Path::new(&draft).exists(), "{draft} left behind");
    }

    #[test]
    fn a_blank_save_is_no_comment_and_leaves_no_file() {
        let record = std::env::temp_dir().join(format!("tm-draft-path-{}", Uuid::now_v7()));
        let editor = format!(
            "printf %s \"$1\" > '{}'; printf ' \\n\\t' >",
            record.display()
        );
        let unwritten = compose_with(&editor, COMMENT).unwrap_err();
        assert_eq!(unwritten.why, "the comment was empty");
        assert_eq!(unwritten.kept, None);
        let draft = fs::read_to_string(&record).unwrap();
        fs::remove_file(&record).unwrap();
        assert!(!Path::new(&draft).exists(), "{draft} left behind");
    }

    #[test]
    fn a_draft_starts_from_its_text_in_a_file_named_for_it() {
        let view = Draft {
            start: "[[views]]\n",
            suffix: "toml",
            noun: "View",
        };
        let composed = compose_with("printf 'name = \"x\"\\n' >>", view).unwrap();
        assert_eq!(composed.text, "[[views]]\nname = \"x\"\n");
        let name = composed.draft.file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with("tm-View-") && name.ends_with(".toml"),
            "{name}"
        );
        fs::remove_file(&composed.draft).unwrap();
        let unwritten = compose_with("printf '' >", view).unwrap_err();
        assert_eq!(unwritten.why, "the View was empty");
    }

    #[test]
    fn an_editor_that_removes_its_file_keeps_nothing() {
        let unwritten = compose_with("rm", COMMENT).unwrap_err();
        assert!(unwritten.why.contains("cannot read"), "{unwritten:?}");
        assert_eq!(unwritten.kept, None);
    }
}
