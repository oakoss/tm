//! `tm hook`: the heartbeat the hooks `tm setup` writes run after each tool
//! use.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use tm_core::actor::SessionId;
use tm_store::workspace;

use crate::claim;
use crate::output::Output;

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum Harness {
    /// Claude Code.
    Claude,
}

impl Harness {
    /// The name the commands take.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
        }
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
        let shown = match harness {
            Harness::Claude => json!({
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
    let session = ["TM_SESSION", "CLAUDE_CODE_SESSION_ID"]
        .into_iter()
        .find_map(|var| SessionId::parse(&std::env::var(var).ok()?))?;
    let home = workspace::home(std::env::var_os("TM_HOME"), std::env::home_dir()).ok()?;
    Some(
        home.join("hooks")
            .join(format!("{}.reported", session.as_str())),
    )
}
