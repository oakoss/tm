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
use crate::{claim, covering, here, identity, prime};

pub(crate) use crate::harness::Harness;

/// Makes this process write as the hook's Agent and session. Codex gives its
/// hooks no `CODEX_SESSION_ID`, only the same id as `session_id` on stdin.
/// Claude Code names a sub-agent only by the `agent_id` on stdin, and its
/// commands by the `TM_AGENT` [`pre_bash`] exports. Returns whether it writes
/// as a sub-agent.
pub(crate) fn enter(harness: Harness, input: Option<&Value>) -> Result<bool, CliError> {
    match harness {
        Harness::Claude => {
            let name = input.and_then(sub_agent);
            let found = name.is_some();
            if let Some(name) = name {
                identity::hook_var("TM_AGENT", name);
            }
            Ok(found)
        }
        Harness::Codex => {
            let session = input
                .and_then(|input| input.get("session_id"))
                .and_then(Value::as_str)
                .and_then(SessionId::parse)
                .ok_or_else(|| failed("the hook's input has no usable session_id"))?;
            identity::hook_var("CODEX_SESSION_ID", session.as_str().to_owned());
            Ok(false)
        }
    }
}

fn failed(why: impl Into<String>) -> CliError {
    CliError::new("hook_input", why)
}

/// The hook's JSON on stdin; `None` for no input, as from a hook written
/// before tm read it.
pub(crate) fn input() -> Result<Option<Value>, CliError> {
    let mut stdin = io::stdin();
    // Run by hand at a terminal, it would wait for input that never comes.
    if stdin.is_terminal() {
        return Err(failed(
            "tm hook reads the hook's JSON on stdin, which is a terminal here",
        ));
    }
    let mut text = String::new();
    stdin
        .read_to_string(&mut text)
        .map_err(|err| failed(format!("cannot read the hook's input: {err}")))?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|err| failed(format!("the hook's input is not JSON: {err}")))
}

/// The name a Claude Code sub-agent writes under: its type and the start of
/// its id, which tell apart two of one type, in characters a shell leaves
/// alone. `None` for the main Agent, whose input has no `agent_id`.
fn sub_agent(input: &Value) -> Option<String> {
    let id = input.get("agent_id")?.as_str()?.trim();
    if id.is_empty() {
        return None;
    }
    let kind = input
        .get("agent_type")
        .and_then(Value::as_str)
        .filter(|kind| !kind.trim().is_empty())
        .unwrap_or("agent");
    let safe = |text: &str| -> String {
        text.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "._:-".contains(c) {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    };
    Some(format!(
        "{}-{}",
        safe(kind),
        safe(&id.chars().take(8).collect::<String>())
    ))
}

/// For a Claude Code sub-agent's Bash command that runs tm, the hook output
/// that exports its name first, so the command writes as that sub-agent.
/// Other commands pass untouched: two hooks rewriting one command do not
/// merge, and one of them is dropped.
pub(crate) fn pre_bash(input: Option<&Value>) -> Result<Output, CliError> {
    let none = || Ok(Output::new(String::new(), json!({ "rewritten": false })));
    let Some(input) = input else {
        return none();
    };
    let Some(name) = sub_agent(input) else {
        return none();
    };
    let Some(Value::Object(tool_input)) = input.get("tool_input") else {
        return none();
    };
    let Some(command) = tool_input.get("command").and_then(Value::as_str) else {
        return none();
    };
    if !runs_tm(command) {
        return none();
    }
    // Claude Code's permission rules match `export …; cmd` but not a
    // `TM_AGENT=… cmd` prefix; a `TM_AGENT` the command sets comes later and
    // wins.
    let mut updated = tool_input.clone();
    updated.insert(
        "command".into(),
        Value::String(format!("export TM_AGENT={name}; {command}")),
    );
    let shown = json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "updatedInput": updated,
        },
    });
    Ok(Output::new(
        format!("{shown}\n"),
        json!({ "rewritten": true }),
    ))
}

/// Before Claude Code edits a file, tells the Agent of each other Agent's or
/// session's live Claim covering it, once per Claim and file for each Agent
/// in this session, and never blocks the edit. Silent outside a Workspace;
/// what it could not check, and an error, are told once while they last.
pub(crate) fn pre_edit(input: Result<Option<Value>, CliError>) -> Output {
    report(
        ("pre-edit", "PreToolUse"),
        input.and_then(|input| {
            input.map_or_else(|| Ok(Notes::default()), |input| edit_notes(&input))
        }),
    )
}

/// What a hook has to tell this session: each `lasting` note once, and each
/// of `checked`, what a check it ran could not tell, once until a run that
/// checks no longer has it; `None` when this run checked nothing.
#[derive(Debug, Default)]
struct Notes {
    lasting: Vec<(String, String)>,
    checked: Option<Vec<(String, String)>>,
}

/// The `hook`'s output for `found`: silent with nothing new, else the
/// harness's hook JSON for `event`, which Claude Code and Codex both read.
/// Outside a Workspace says nothing; an error is told once until a run does
/// not fail, and its warnings last.
fn report((hook, event): (&str, &str), found: Result<Notes, CliError>) -> Output {
    let (notes, error) = match found {
        Ok(notes) => (notes, None),
        Err(err) if err.code == "no_workspace" => (Notes::default(), None),
        Err(err) => {
            let lasting = err
                .warnings
                .into_iter()
                .map(|warning| (format!("note:{warning}"), warning))
                .collect();
            let notes = Notes {
                lasting,
                checked: None,
            };
            (notes, Some(err.message))
        }
    };
    let fresh = unreported(hook, notes, error);
    if fresh.is_empty() {
        return Output::new(String::new(), json!({ "reported": fresh }));
    }
    let shown = json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": format!("tm: {}", fresh.join("\ntm: ")),
        },
    });
    Output::new(format!("{shown}\n"), json!({ "reported": fresh }))
}

/// The notes for the file the hook's `input` edits.
fn edit_notes(input: &Value) -> Result<Notes, CliError> {
    let tool = input.get("tool_input");
    let file = tool
        .and_then(|tool| tool.get("file_path").or_else(|| tool.get("notebook_path")))
        .and_then(Value::as_str);
    let Some(file) = file else {
        let keys: Vec<&str> = tool
            .and_then(Value::as_object)
            .map(|tool| tool.keys().map(String::as_str).collect())
            .unwrap_or_default();
        return Err(failed(format!(
            "cannot find the edited file in the hook's input (tool_input holds {}), so Path Claims go unchecked",
            keys.join(", ")
        )));
    };
    let here = here::read()?;
    let base = input
        .get("cwd")
        .and_then(Value::as_str)
        .map_or_else(|| here.cwd.clone(), PathBuf::from);
    let Some(file) = covering::in_repo(&here.found.dir, &base, file) else {
        return Ok(Notes::default());
    };
    enter(Harness::Claude, Some(input))?;
    let me = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|why| failed(format!("cannot tell who is editing: {why}")))?;
    let others = covering::others(&here.store.claims()?, Some(&me.actor));
    if others.is_empty() {
        return Ok(Notes::default());
    }
    let check = covering::overlaps(&here.found.dir, &others, std::slice::from_ref(&file));
    let unchecked = check.unchecked.into_iter().map(|why| (why.clone(), why));
    let mut notes = Notes {
        lasting: Vec::new(),
        checked: check.branches.then(|| unchecked.collect()),
    };
    for overlap in check.found {
        let shown = here
            .store
            .issue(overlap.claim.issue)?
            .and_then(|view| view.key)
            .unwrap_or_else(|| overlap.claim.issue.to_string());
        let text = format!(
            "{}; ask its holder or tm show {shown} before editing it",
            covering::told(&overlap, &shown)
        );
        let key = format!(
            "covers:{}:{}:{}",
            me.actor.name(),
            overlap.claim.id,
            overlap.file
        );
        notes.lasting.push((key, text));
    }
    Ok(notes)
}

/// Whether `command` names tm as a word; a tm run from a script it calls is
/// missed, and writes as the main Agent.
fn runs_tm(command: &str) -> bool {
    command
        .split(|c: char| c.is_whitespace() || ";|&()`'\"$".contains(c))
        .any(|word| word == "tm" || word.ends_with("/tm"))
}
/// `tm prime` for a session-start hook, whose plain output the harness adds
/// to the Agent's context. A repo tm does not track fails here, as `tm prime`
/// does, so the hook's failure tells a person to run `tm init`.
pub(crate) fn session_start() -> Result<Output, CliError> {
    prime::run(false)
}

/// Releases the session's Claims when it ends: for Claude Code its
/// sub-agents' too, since one cut short gets no `SubagentStop`. A repo tm does
/// not track has nothing to release, so that ends quietly rather than as a
/// hook failure.
pub(crate) fn session_end(harness: Harness) -> Result<Output, CliError> {
    quiet_untracked(match harness {
        Harness::Claude => claim::release_session(),
        Harness::Codex => claim::release_mine(),
    })
}

/// Releases a finished Claude Code sub-agent's Claims; `sub_agent` is whether
/// [`enter`] found one to write as.
pub(crate) fn subagent_stop(sub_agent: bool) -> Result<Output, CliError> {
    if !sub_agent {
        return Err(failed("the hook's input has no agent_id"));
    }
    quiet_untracked(claim::release_mine())
}

fn quiet_untracked(result: Result<Output, CliError>) -> Result<Output, CliError> {
    match result {
        Err(err) if err.code == "no_workspace" => Ok(Output::new(String::new(), json!({}))),
        result => result,
    }
}

/// `tm heartbeat` for a hook: silent unless it has news, then the harness's
/// hook JSON, which is how an async hook reaches the Agent. Each warning is
/// told once per session, or it would repeat after every tool use: a takeover
/// once per Claim that took over, an error once until a heartbeat succeeds.
/// Codex ignores a hook's plain output here, so both harnesses get the JSON.
pub(crate) fn heartbeat() -> Output {
    let note = |warning: String| (format!("note:{warning}"), warning);
    let found = claim::beat(None).map(|beat| {
        let (before, published) = beat.warnings;
        let mut lasting: Vec<(String, String)> = before.into_iter().map(note).collect();
        lasting.extend(
            beat.taken_over
                .into_iter()
                .map(|(claim, warning)| (format!("takeover:{claim}"), warning)),
        );
        lasting.extend(beat.unchecked.into_iter().chain(published).map(note));
        Notes {
            lasting,
            checked: None,
        }
    });
    report(("heartbeat", "PostToolUse"), found)
}

/// The text of those `notes` this session has not been told, recorded in
/// tm's home by key as told. The `hook`'s `error` is forgotten by a run of it
/// that does not fail, and what its check could not tell by a run that checks
/// and can, so each is told again if it comes back. Without a session to key
/// them by, or a home to keep them in, every note is fresh.
fn unreported(hook: &str, notes: Notes, error: Option<String>) -> Vec<String> {
    let failed = format!("now:{hook}:error:");
    let unchecked = format!("now:{hook}:check:");
    let checked = notes.checked.is_some();
    let notes: Vec<(String, String)> = notes
        .lasting
        .into_iter()
        .chain(
            notes
                .checked
                .into_iter()
                .flatten()
                .map(|(key, text)| (format!("{unchecked}{key}"), text)),
        )
        .chain(error.map(|error| (format!("{failed}{error}"), error)))
        .collect();
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
    let before = told.len();
    let passed = |key: &String| {
        let gone = !notes.iter().any(|(now, _)| now == key);
        gone && (key.starts_with(&failed) || checked && key.starts_with(&unchecked))
    };
    told.retain(|key| !passed(key));
    let forgotten = told.len() != before;
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
