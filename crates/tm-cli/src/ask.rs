//! `tm ask` and `tm answer`: a question an Issue waits on, and its reply,
//! each as one batch.

use std::fmt::Write as _;

use serde_json::json;
use tm_store::Claim;
use uuid::Uuid;

use crate::error::CliError;
use crate::fields;
use crate::follow;
use crate::here::{self, Here};
use crate::identity;
use crate::output::Output;

#[derive(clap::Args)]
pub(crate) struct Ask {
    /// The Issue's Key or full id.
    issue: String,
    /// What only the person can decide; - reads it from stdin.
    question: String,
    /// Who answers it; the human you work for when not given.
    #[arg(long, value_name = "PERSON")]
    of: Option<String>,
}

#[derive(clap::Args)]
pub(crate) struct Answer {
    /// The Issue's Key or full id.
    issue: String,
    /// The reply; - reads it from stdin.
    reply: String,
}

pub(crate) fn ask(args: &Ask) -> Result<Output, CliError> {
    here::write(|here| write_ask(here, args))
}

pub(crate) fn answer(args: &Answer) -> Result<Output, CliError> {
    here::write(|here| write_answer(here, args))
}

fn write_ask(here: &mut Here, args: &Ask) -> Result<(Output, Vec<String>), CliError> {
    let reference = args.issue.as_str();
    refuse_blank("question", &args.question)?;
    let (id, key, waiting) = issue(here, reference)?;
    if let Some(waiting) = waiting {
        return Err(CliError::new(
            "already_waiting",
            format!("{reference} already waits on {waiting}; tm answer {reference} \"...\" first"),
        )
        .with("waiting_on", waiting));
    }
    if here
        .store
        .issue(id)?
        .is_some_and(|view| follow::finished(&view))
    {
        return Err(CliError::new(
            "finished",
            format!("{reference} is finished, so it waits on no one"),
        ));
    }
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    let actor = &identity.actor;
    let of = match (&args.of, actor.on_behalf_of()) {
        (None, Some(human)) => human.to_owned(),
        (Some(of), None) => of.trim().to_owned(),
        (Some(_), Some(human)) => {
            return Err(CliError::new(
                "usage",
                format!("an Agent's question waits on its human, {human}; leave out --of"),
            ));
        }
        (None, None) => {
            return Err(CliError::new(
                "usage",
                format!(
                    "a person names who answers with --of: tm ask {reference} --of PERSON \"...\""
                ),
            ));
        }
    };
    refuse_blank("of", &of)?;
    let state = here.store.claim_state(id)?;
    let live = state.as_ref().and_then(|state| state.held().cloned());
    if let Some(other) = live.as_ref().filter(|claim| !claim.taken_by(actor)) {
        return Err(CliError::new(
            "already_claimed",
            format!(
                "{reference} is claimed by {}; tm comment {reference} leaves it a note",
                crate::claim::holder(other)
            ),
        ));
    }
    // A lapsed Claim of the asker's is kept too: the write would otherwise
    // renew it for this session alone, and the next could not resume.
    let held: Option<Claim> = state
        .map(|state| state.claim().clone())
        .filter(|claim| claim.taken_by(actor));
    let question = read("question", &args.question)?;
    let ops = here.store.ask(actor, id, &question, &of, held.as_ref())?;
    let shown = key.clone().unwrap_or_else(|| id.to_string());
    let mut text = format!("{shown} waits on {of}: {question}\n");
    if let Some(held) = &held {
        let _ = writeln!(
            text,
            "the Claim is kept for {}, which resumes it with tm start {shown} once answered",
            held.agent
        );
    }
    if actor.is_agent() {
        text.push_str("tm next takes other work\n");
    }
    let data = json!({
        "issue": { "id": id, "key": key },
        "waiting_on": of,
        "question": question,
        "kept_for": held.as_ref().map(|held| &held.agent),
    });
    let mut output = here.undoable(Output::new(text, data), ops[0].op_id, actor)?;
    // The question has landed, so a failed read only warns.
    match here.store.has_written(&of) {
        Ok(true) => {}
        Ok(false) => output.warnings.push(format!(
            "{of} has written nothing in this Workspace; if the name is mistyped, their sessions never show the question"
        )),
        Err(err) => output.warnings.push(format!(
            "the question landed, but whether {of} has written here is unknown: {err}"
        )),
    }
    Ok((output, identity.warnings))
}

fn write_answer(here: &mut Here, args: &Answer) -> Result<(Output, Vec<String>), CliError> {
    let reference = args.issue.as_str();
    refuse_blank("reply", &args.reply)?;
    let (id, key, waiting) = issue(here, reference)?;
    let Some(waiting) = waiting else {
        return Err(CliError::new(
            "not_waiting",
            format!("{reference} waits on no one; tm comment {reference} adds a comment"),
        ));
    };
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    let actor = &identity.actor;
    // Read before the reply, so a question asked anew meanwhile is refused.
    let question = here.store.open_question(id)?.ok_or_else(|| {
        CliError::new(
            "not_waiting",
            format!("{reference} has no open question; tm comment {reference} adds a comment"),
        )
    })?;
    let reply = read("reply", &args.reply)?;
    let ops = here.store.answer(actor, id, question.id, &reply)?;
    let kept_for = here
        .store
        .claim_state(id)?
        .map(|state| state.claim().clone())
        .filter(|claim| claim.handed)
        .map(|claim| claim.agent);
    let shown = key.clone().unwrap_or_else(|| id.to_string());
    let mut text = format!("{shown} answered for {waiting}\n");
    if let Some(agent) = &kept_for {
        let _ = writeln!(text, "{agent} resumes it with tm start {shown}");
    }
    let data = json!({
        "issue": { "id": id, "key": key },
        "answered_for": waiting,
        "reply": reply,
        "kept_for": kept_for,
    });
    let output = here.undoable(Output::new(text, data), ops[0].op_id, actor)?;
    Ok((output, identity.warnings))
}

/// The Issue `reference` names, its Key, and who it waits on.
fn issue(here: &Here, reference: &str) -> Result<(Uuid, Option<String>, Option<String>), CliError> {
    let id = here.issue(reference)?;
    let view = here
        .store
        .issue(id)?
        .ok_or_else(|| CliError::new("not_found", format!("no Issue is {reference}")))?;
    let waiting = view.waiting_on().map(str::to_owned);
    Ok((id, view.key, waiting))
}

// Comments are append-only, so a blank one from a failed `$(cmd)` would stay
// for good.
fn refuse_blank(field: &str, text: &str) -> Result<(), CliError> {
    if text.trim().is_empty() {
        return Err(
            CliError::new("invalid_value", format!("the {field} is blank"))
                .with("field", field)
                .with("reason", "is blank"),
        );
    }
    Ok(())
}

/// `given`, or stdin for `-`; read once all else is checked, so a mistake
/// keeps the piped text.
fn read(field: &str, given: &str) -> Result<String, CliError> {
    if given != "-" {
        return Ok(given.to_owned());
    }
    let read = fields::read_stdin_text(field)?;
    if read.trim().is_empty() {
        return Err(fields::refused(field, "is blank"));
    }
    Ok(read)
}
