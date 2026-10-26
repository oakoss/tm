//! `tm handoff`: an Issue's Claim handed to another Agent with a note, as
//! one batch; the first session of that Agent to start the Issue takes it
//! (ADR 0009).

use serde_json::json;
use tm_store::{Named, StoreError};

use crate::claim::{claim_json, expiry};
use crate::error::CliError;
use crate::fields;
use crate::here::{self, Here};
use crate::identity;
use crate::output::Output;

#[derive(clap::Args)]
pub(crate) struct Handoff {
    /// The Issue's Key or full id.
    issue: String,
    /// The Agent to hand it to, such as codex; its next session takes it
    /// with tm start.
    #[arg(long, value_name = "AGENT")]
    to: String,
    /// What the next session needs to carry on, kept as a comment; - reads it
    /// from stdin.
    note: String,
    /// Hand off a Claim another Agent session holds; only once that session
    /// has stopped work on it.
    #[arg(long)]
    force: bool,
}

pub(crate) fn run(args: &Handoff) -> Result<Output, CliError> {
    here::write(|here| write(here, args))
}

fn write(here: &mut Here, args: &Handoff) -> Result<(Output, Vec<String>), CliError> {
    let reference = args.issue.as_str();
    // Comments are append-only, so a blank note from a failed `$(cmd)` would
    // stay for good.
    if args.note.trim().is_empty() {
        return Err(CliError::new("invalid_value", "the note is blank")
            .with("field", "note")
            .with("reason", "is blank"));
    }
    // Trimmed as an Agent's own name is, so the Agent can take it.
    let to = args.to.trim();
    // Checked here as the store would, before any piped note is read.
    let bad_agent = if to.is_empty() {
        Some("is blank")
    } else if to.chars().any(char::is_control) {
        Some("has control characters")
    } else {
        None
    };
    if let Some(reason) = bad_agent {
        let message = format!("--to {reason}; name the Agent, such as codex");
        return Err(CliError::new("invalid_value", message)
            .with("field", "to")
            .with("reason", reason));
    }
    let id = here.issue(reference)?;
    let key = here.store.issue(id)?.and_then(|view| view.key);
    let named = Named {
        key: key.clone(),
        ..id.into()
    };
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    let actor = &identity.actor;
    let Some(state) = here.store.claim_state(id)? else {
        return Err(CliError::new(
            "not_claim_holder",
            format!(
                "{reference} is unclaimed, so there is no Claim to hand off; {to} takes it with tm start {reference}"
            ),
        )
        .issue(named));
    };
    let held = state.claim().clone();
    // A lapsed Claim ends no one's work; one held for the caller's human is
    // handed on as theirs, while another human's stays held for them.
    let own_human = held.on_behalf_of == actor.on_behalf_of().unwrap_or(actor.name());
    let own = held.taken_by(actor)
        || (!actor.is_agent() && held.on_behalf_of == actor.name())
        || (state.held().is_none() && own_human);
    if !own && !args.force {
        let mut err = CliError::from(StoreError::AlreadyClaimed {
            issue: named,
            holder: Box::new(held),
        });
        err.message
            .push_str("; handing it off would end that Claim, which tm handoff --force does");
        return Err(err);
    }
    // Read once all else is checked, so a mistake keeps the piped text.
    let note = if args.note == "-" {
        let read = fields::read_stdin_text("note")?;
        if read.trim().is_empty() {
            return Err(fields::refused("note", "is blank"));
        }
        read
    } else {
        args.note.clone()
    };
    // Nobody renews an untaken handoff, so one to a mistyped Agent still lapses.
    let ttl_ms = held.ttl_ms.or_else(|| here.found.file.claim_ttl_ms());
    let (ops, handed) = here.store.hand(actor, &held, to, ttl_ms, &note)?;
    let now_ms = here.store.utc_ms()?;
    let shown = key.clone().unwrap_or_else(|| id.to_string());
    let text = format!(
        "{shown} handed to {} for {}, {}; its next session takes it with tm start {shown}\n",
        handed.agent,
        handed.on_behalf_of,
        expiry(&handed, now_ms)
    );
    let data = json!({
        "issue": { "id": id, "key": key },
        "claim": claim_json(&handed, now_ms),
        "note": note,
    });
    let mut output = here.undoable(Output::new(text, data), ops[0].op_id, actor)?;
    let undo = crate::output::short(ops[0].op_id);
    // The handoff has landed, so a failed read only warns.
    match here.store.has_written(to) {
        Ok(true) => {}
        Ok(false) => output.warnings.push(format!(
            "{to} has written nothing in this Workspace; if the name is mistyped, tm undo {undo} releases the handoff"
        )),
        Err(err) => output.warnings.push(format!(
            "the handoff landed, but whether {to} has written here is unknown: {err}"
        )),
    }
    Ok((output, identity.warnings))
}
