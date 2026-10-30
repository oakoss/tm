//! `tm ask`, `tm answer` and `tm withdraw`: a question an Issue waits on, and
//! how it is settled, each as one batch.

use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::Actor;
use tm_core::op::{is_number, question_options, same_reply};
use tm_store::{Claim, CommentView, IssueView};
use uuid::Uuid;

use crate::error::CliError;
use crate::fields;
use crate::follow;
use crate::here::{self, Here};
use crate::identity;
use crate::output::{Output, printable, short};
use crate::prompt;
use crate::show::numbered;

#[derive(clap::Args)]
pub(crate) struct Ask {
    /// The Issue's Key or full id.
    issue: String,
    /// What only the person can decide; - reads it from stdin.
    question: String,
    /// Who answers it; the human you work for when not given.
    #[arg(long, value_name = "PERSON")]
    of: Option<String>,
    /// A reply to offer, picked by its number or words; give two or more.
    #[arg(long = "option", value_name = "REPLY")]
    options: Vec<String>,
}

#[derive(clap::Args)]
pub(crate) struct Answer {
    /// The Issue's Key or full id.
    issue: Option<String>,
    /// The reply: an option's number or words, or your own; - reads it
    /// from stdin.
    reply: Option<String>,
}

#[derive(clap::Args)]
pub(crate) struct Withdraw {
    /// The Issue's Key or full id.
    issue: String,
    /// Why the question no longer needs an answer; - reads it from stdin.
    reason: String,
}

pub(crate) fn ask(args: &Ask) -> Result<Output, CliError> {
    here::write(|here| write_ask(here, args))
}

pub(crate) fn answer(args: &Answer, json: bool) -> Result<Output, CliError> {
    match (&args.issue, &args.reply) {
        (Some(issue), Some(reply)) => here::write(|here| write_answer(here, issue, Given(reply))),
        (Some(issue), None) => {
            at_terminal(&format!("tm answer {issue} \"...\""), json)?;
            here::write(|here| write_answer(here, issue, Typed))
        }
        (None, _) => {
            at_terminal("tm answer KEY \"...\"", json)?;
            here::write(walk)
        }
    }
}

pub(crate) fn withdraw(args: &Withdraw) -> Result<Output, CliError> {
    here::write(|here| write_withdraw(here, args))
}

/// Where a reply comes from.
#[derive(Clone, Copy)]
enum Reply<'a> {
    Given(&'a str),
    Typed,
}
use Reply::{Given, Typed};

/// Refuses to prompt anyone but a person at a terminal reading text, naming
/// the form that passes the reply instead.
fn at_terminal(form: &str, json: bool) -> Result<(), CliError> {
    if prompt::human() && !json {
        return Ok(());
    }
    Err(CliError::new(
        "usage",
        format!("give the reply: {form}; only a person at a terminal is asked for one"),
    ))
}

fn write_ask(here: &mut Here, args: &Ask) -> Result<(Output, Vec<String>), CliError> {
    let reference = args.issue.as_str();
    refuse_blank("question", &args.question)?;
    let options = offered(&args.options)?;
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
    let ops = here
        .store
        .ask(actor, id, &question, &options, &of, held.as_ref())?;
    let shown = key.clone().unwrap_or_else(|| id.to_string());
    let mut text = format!("{shown} waits on {of}: {question}\n");
    if !options.is_empty() {
        let _ = writeln!(text, "  options: {}", numbered(&options));
    }
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
        "options": options,
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

fn write_answer(
    here: &mut Here,
    reference: &str,
    reply: Reply<'_>,
) -> Result<(Output, Vec<String>), CliError> {
    if let Given(given) = reply {
        refuse_blank("reply", given)?;
    }
    let (id, key, waiting) = issue(here, reference)?;
    let waiting = waiting.ok_or_else(|| not_waiting(reference))?;
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    let actor = &identity.actor;
    // Read before the reply, so a question asked anew meanwhile is refused.
    let question = open_question(here, id, reference)?;
    let (reply, choice) = match reply {
        Given(given) => resolve(&read("reply", given)?, &question.options)?,
        Typed => {
            let shown = key.clone().unwrap_or_else(|| id.to_string());
            match ask_reply(&shown, None, &question)? {
                Some(typed) => typed,
                None => return Err(CliError::new("stopped", prompt::stopped())),
            }
        }
    };
    let (text, data, batch) = answered(here, actor, (id, key.as_ref()), &question, &reply)?;
    let mut data = data;
    data["answered_for"] = json!(waiting);
    data["choice"] = json!(choice);
    let output = here.undoable(Output::new(text, data), batch, actor)?;
    Ok((output, identity.warnings))
}

/// Answers `question` on the Issue with `reply`: the text that says so, its
/// JSON, and the batch.
fn answered(
    here: &mut Here,
    actor: &Actor,
    (id, key): (Uuid, Option<&String>),
    question: &CommentView,
    reply: &str,
) -> Result<(String, Value, Uuid), CliError> {
    let ops = here.store.answer(actor, id, question.id, reply)?;
    let kept_for = kept_for(here, id)?;
    let shown = key.cloned().unwrap_or_else(|| id.to_string());
    let mut text = format!("{shown} answered: {}\n", printable(reply));
    if let Some(agent) = &kept_for {
        let _ = writeln!(text, "{agent} resumes it with tm start {shown}");
    }
    let data = json!({
        "issue": { "id": id, "key": key },
        "reply": reply,
        "kept_for": kept_for,
    });
    Ok((text, data, ops[0].op_id))
}

/// Each question waiting on the person at the terminal in turn, oldest
/// first; a blank reply passes one over and end of input stops.
fn walk(here: &mut Here) -> Result<(Output, Vec<String>), CliError> {
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    let actor = identity.actor.clone();
    let mut warnings = identity.warnings;
    let waiting = crate::prime::waiting(&here.store, Some(&actor), &mut warnings)?;
    if waiting.is_empty() {
        let text = format!("no question waits on {}\n", printable(actor.name()));
        return Ok((Output::new(text, json!({ "answered": [] })), warnings));
    }
    let mut asked = Vec::with_capacity(waiting.len());
    for (view, question) in &waiting {
        let shown = view.key.clone().unwrap_or_else(|| view.id.to_string());
        match question {
            Some(question) => asked.push((view, shown, question)),
            None => warnings.push(format!(
                "{shown} waits on you, but its question could not be read; tm show {shown}"
            )),
        }
    }
    let count = asked.len();
    let (mut text, mut answers, mut failed) = (String::new(), Vec::new(), Vec::new());
    for (i, (view, shown, question)) in asked.into_iter().enumerate() {
        let typed = match ask_reply(&shown, Some((view, i + 1, count)), question) {
            Ok(Some(typed)) => typed,
            Ok(None) => break,
            // What was answered stays reported.
            Err(err) => {
                warnings.push(format!("stopped reading replies: {}", err.message));
                break;
            }
        };
        let (reply, choice) = typed;
        if reply.is_empty() {
            continue;
        }
        // One failed answer leaves the rest to answer.
        match answered(here, &actor, (view.id, view.key.as_ref()), question, &reply) {
            Ok((answer, mut data, batch)) => {
                let _ = write!(text, "{answer}");
                let _ = writeln!(text, "  undo with: tm undo {}", short(batch));
                data["choice"] = json!(choice);
                data["batch"] = json!(batch);
                answers.push(data);
            }
            Err(err) => {
                eprintln!("{shown} was not answered: {}", err.message);
                failed.push(
                    json!({ "issue": { "id": view.id, "key": view.key }, "error": err.code }),
                );
            }
        }
    }
    if answers.is_empty() && !failed.is_empty() {
        return Err(CliError::new(
            "not_answered",
            format!("no reply was written: {} failed", failed.len()),
        )
        .warned(warnings));
    }
    let _ = write!(text, "answered {} of {count}", answers.len());
    if !failed.is_empty() {
        let _ = write!(text, ", {} failed", failed.len());
    }
    text.push('\n');
    let data = json!({ "answered": answers, "failed": failed });
    Ok((Output::new(text, data), warnings))
}

/// Shows `question` and reads a reply at the terminal until one resolves:
/// the reply and its choice, an empty reply for a blank line, or `None` at
/// end of input.
fn ask_reply(
    shown: &str,
    place: Option<(&IssueView, usize, usize)>,
    question: &CommentView,
) -> Result<Option<(String, Option<usize>)>, CliError> {
    let mut asked = match place {
        Some((view, at, of)) => format!(
            "\n({at} of {of}) {shown}  {}\n",
            printable(
                view.fields
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            )
        ),
        None => format!("{shown}\n"),
    };
    let _ = writeln!(asked, "  {} asked:", printable(&question.actor));
    for line in question.body.lines() {
        let _ = writeln!(asked, "    {}", printable(line));
    }
    if !question.options.is_empty() {
        let _ = writeln!(asked, "  options: {}", numbered(&question.options));
    }
    // On stderr with the prompt, so stdout holds only what was written.
    eprint!("{asked}");
    let skip = if place.is_some() { ", blank skips" } else { "" };
    loop {
        let typed = prompt::prompt(&format!("reply (a number or words{skip}): "))
            .map_err(|err| CliError::new("io", err))?;
        let Some(typed) = typed else { return Ok(None) };
        if typed.is_empty() {
            if place.is_some() {
                return Ok(Some((String::new(), None)));
            }
            continue;
        }
        match resolve(&typed, &question.options) {
            Ok(resolved) => return Ok(Some(resolved)),
            Err(err) => eprintln!("{}", err.message),
        }
    }
}

fn write_withdraw(here: &mut Here, args: &Withdraw) -> Result<(Output, Vec<String>), CliError> {
    let reference = args.issue.as_str();
    refuse_blank("reason", &args.reason)?;
    let (id, key, waiting) = issue(here, reference)?;
    let waiting = waiting.ok_or_else(|| not_waiting(reference))?;
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    let actor = &identity.actor;
    let question = open_question(here, id, reference)?;
    if question.actor != actor.name() || question.on_behalf_of.as_deref() != actor.on_behalf_of() {
        let asker = match &question.on_behalf_of {
            Some(human) => format!("{} for {human}", question.actor),
            None => question.actor.clone(),
        };
        return Err(CliError::new(
            "not_asker",
            format!(
                "{reference}'s question was asked by {}; only its asker withdraws it, and tm answer {reference} \"...\" replies",
                printable(&asker)
            ),
        ));
    }
    let reason = read("reason", &args.reason)?;
    let ops = here.store.withdraw(actor, id, question.id, &reason)?;
    let kept_for = kept_for(here, id)?;
    let shown = key.clone().unwrap_or_else(|| id.to_string());
    let mut text = format!("{shown} no longer waits on {waiting}: {reason}\n");
    if let Some(agent) = &kept_for {
        let _ = writeln!(text, "{agent} resumes it with tm start {shown}");
    }
    let data = json!({
        "issue": { "id": id, "key": key },
        "withdrawn_for": waiting,
        "reason": reason,
        "kept_for": kept_for,
    });
    let output = here.undoable(Output::new(text, data), ops[0].op_id, actor)?;
    Ok((output, identity.warnings))
}

fn kept_for(here: &Here, id: Uuid) -> Result<Option<String>, CliError> {
    Ok(here
        .store
        .claim_state(id)?
        .map(|state| state.claim().clone())
        .filter(|claim| claim.handed)
        .map(|claim| claim.agent))
}

fn open_question(here: &Here, id: Uuid, reference: &str) -> Result<CommentView, CliError> {
    here.store.open_question(id)?.ok_or_else(|| {
        CliError::new(
            "not_waiting",
            format!("{reference} has no open question; tm comment {reference} adds a comment"),
        )
    })
}

fn not_waiting(reference: &str) -> CliError {
    CliError::new(
        "not_waiting",
        format!("{reference} waits on no one; tm comment {reference} adds a comment"),
    )
}

/// Checked here as well as in the write, so the refusal names `--option`.
fn offered(options: &[String]) -> Result<Vec<String>, CliError> {
    question_options(options).map_err(|reason| {
        CliError::new("invalid_value", format!("--option: {reason}"))
            .with("field", "options")
            .with("reason", reason)
    })
}

/// `reply` as the option its words or number picks, with that number; any
/// other reply stands as written, but a number picks an option or is refused.
fn resolve(reply: &str, options: &[String]) -> Result<(String, Option<usize>), CliError> {
    let trimmed = reply.trim();
    if let Some(at) = options
        .iter()
        .position(|option| same_reply(option, trimmed))
    {
        return Ok((options[at].clone(), Some(at + 1)));
    }
    if options.is_empty() || !is_number(trimmed) {
        return Ok((reply.to_owned(), None));
    }
    match trimmed.parse::<usize>() {
        Ok(number) if (1..=options.len()).contains(&number) => {
            Ok((options[number - 1].clone(), Some(number)))
        }
        _ => Err(invalid(
            "reply",
            &format!(
                "picks no option: choose 1 to {}, or reply in words",
                options.len()
            ),
        )),
    }
}

fn invalid(field: &str, reason: &str) -> CliError {
    CliError::new("invalid_value", format!("the {field} {reason}"))
        .with("field", field)
        .with("reason", reason)
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
