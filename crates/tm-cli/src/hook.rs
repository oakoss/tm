//! `tm hook`: what the hooks `tm setup` writes run when a session starts,
//! after each tool use, and when it ends.

use std::collections::HashSet;
use std::fs;
use std::io::{self, IsTerminal as _, Read as _};
use std::path::PathBuf;

use serde_json::{Value, json};
use tm_core::actor::SessionId;
use tm_store::workspace;

use crate::error::CliError;
use crate::output::Output;
use crate::{claim, identity, prime};

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum Harness {
    /// Claude Code.
    Claude,
    /// Codex.
    Codex,
}

impl Harness {
    /// The name the commands take.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

/// Makes this process write as the hook's Agent and session. Codex gives its
/// hooks no `CODEX_SESSION_ID`, only the same id as `session_id` on stdin.
pub(crate) fn enter(harness: Harness) -> Result<(), CliError> {
    match harness {
        Harness::Claude => Ok(()),
        Harness::Codex => {
            identity::in_codex_session(&stdin_session()?);
            Ok(())
        }
    }
}

fn stdin_session() -> Result<SessionId, CliError> {
    let failed = |why: String| CliError::new("hook_input", why);
    let mut stdin = io::stdin();
    // Run by hand at a terminal, it would wait for input that never comes.
    if stdin.is_terminal() {
        return Err(failed(
            "tm hook reads the hook's JSON on stdin, which is a terminal here".into(),
        ));
    }
    let mut text = String::new();
    stdin
        .read_to_string(&mut text)
        .map_err(|err| failed(format!("cannot read the hook's input: {err}")))?;
    let input: Value = serde_json::from_str(&text)
        .map_err(|err| failed(format!("the hook's input is not JSON: {err}")))?;
    input
        .get("session_id")
        .and_then(Value::as_str)
        .and_then(SessionId::parse)
        .ok_or_else(|| failed("the hook's input has no usable session_id".into()))
}

/// `tm prime` for a session-start hook, whose plain output the harness adds
/// to the Agent's context. A repo tm does not track fails here, as `tm prime`
/// does, so the hook's failure tells a person to run `tm init`.
pub(crate) fn session_start() -> Result<Output, CliError> {
    prime::run(false)
}

/// `tm release --mine` for a session-end hook. A repo tm does not track has
/// nothing to release, so that ends quietly rather than as a hook failure.
pub(crate) fn session_end() -> Result<Output, CliError> {
    match claim::release_mine() {
        Err(err) if err.code == "no_workspace" => Ok(Output::new(String::new(), json!({}))),
        result => result,
    }
}

/// `tm heartbeat` for a hook: silent unless it has news, then the harness's
/// hook JSON, which is how an async hook reaches the Agent. Each warning is
/// told once per session, or it would repeat after every tool use: a takeover
/// once per Claim that took over, an error once until a heartbeat succeeds.
pub(crate) fn heartbeat(harness: Harness) -> Output {
    let note = |warning: String| (format!("note:{warning}"), warning);
    let (notes, succeeded) = match claim::beat(None) {
        Ok(beat) => {
            let (before, published) = beat.warnings;
            let mut notes: Vec<(String, String)> = before.into_iter().map(note).collect();
            notes.extend(
                beat.taken_over
                    .into_iter()
                    .map(|(claim, warning)| (format!("takeover:{claim}"), warning)),
            );
            notes.extend(beat.unchecked.into_iter().chain(published).map(note));
            (notes, true)
        }
        // A hook pasted for every repo also runs where tm tracks nothing.
        Err(err) if err.code == "no_workspace" => (Vec::new(), true),
        Err(err) => {
            let mut notes: Vec<(String, String)> = err.warnings.into_iter().map(note).collect();
            notes.push((format!("error:{}", err.message), err.message));
            (notes, false)
        }
    };
    let fresh = unreported(notes, succeeded);
    let text = if fresh.is_empty() {
        String::new()
    } else {
        let context = format!("tm: {}", fresh.join("\ntm: "));
        // Both harnesses read this shape; Codex ignores plain output here.
        let shown = match harness {
            Harness::Claude | Harness::Codex => json!({
                "hookSpecificOutput": {
                    "hookEventName": "PostToolUse",
                    "additionalContext": context,
                },
            }),
        };
        format!("{shown}\n")
    };
    Output::new(text, json!({ "reported": fresh }))
}

/// The text of those `(key, text)` notes this session has not been told,
/// recorded in tm's home by key as told; `succeeded` forgets the errors told,
/// so a later failure is told again. Without a session to key them by, or a
/// home to keep them in, every note is fresh.
fn unreported(notes: Vec<(String, String)>, succeeded: bool) -> Vec<String> {
    let Some(path) = reported_path() else {
        return notes.into_iter().map(|(_, text)| text).collect();
    };
    // One JSON string a line, so a note that spans lines is still one key.
    let mut told: Vec<String> = fs::read_to_string(&path)
        .map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        })
        .unwrap_or_default();
    let forgotten = succeeded && told.iter().any(|key| key.starts_with("error:"));
    if forgotten {
        told.retain(|key| !key.starts_with("error:"));
    }
    let seen: HashSet<&String> = told.iter().collect();
    let fresh: Vec<(String, String)> = notes
        .into_iter()
        .filter(|(key, _)| !seen.contains(key))
        .collect();
    if forgotten || !fresh.is_empty() {
        told.extend(fresh.iter().map(|(key, _)| key.clone()));
        let mut lines = told
            .iter()
            .map(|key| Value::String(key.clone()).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        lines.push('\n');
        let written = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&path, lines));
        // Unrecorded, a note is told again next time, which beats never.
        drop(written);
    }
    fresh.into_iter().map(|(_, text)| text).collect()
}

fn reported_path() -> Option<PathBuf> {
    let session = identity::session(identity::env)?;
    let home = workspace::home(std::env::var_os("TM_HOME"), std::env::home_dir()).ok()?;
    Some(
        home.join("hooks")
            .join(format!("{}.reported", session.as_str())),
    )
}
