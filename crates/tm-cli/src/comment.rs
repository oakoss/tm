//! `tm comment`: a comment on an Issue, its body given or read from stdin, or
//! a correction of one.

use serde_json::json;
use tm_core::{Actor, Change};
use tm_store::{CommentView, Draft};
use uuid::Uuid;

use crate::error::CliError;
use crate::fields;
use crate::here::{self, Here};
use crate::identity;
use crate::output::{Output, short};

/// Fewer hex digits would too often match more than one comment.
const MIN_PART: usize = 8;

pub(crate) fn run(reference: &str, body: &str, edit: Option<&str>) -> Result<Output, CliError> {
    here::write(|here| write(here, reference, body, edit))
}

fn write(
    here: &mut Here,
    reference: &str,
    body: &str,
    edit: Option<&str>,
) -> Result<(Output, Vec<String>), CliError> {
    // Comments are append-only, so a blank one from a failed `$(cmd)` would
    // stay for good.
    if body.trim().is_empty() {
        return Err(CliError::new("invalid_value", "a comment cannot be blank")
            .with("field", "body")
            .with("reason", "is blank"));
    }
    let id = here.issue(reference)?;
    let project = here
        .store
        .issue(id)?
        .map(|view| view.project)
        .ok_or_else(|| CliError::new("not_found", format!("no Issue is {reference}")))?;
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    let edited = edit
        .map(|which| edited_comment(here, (id, reference), which, &identity.actor))
        .transpose()?;
    // Read once all else is checked, so a mistake keeps the piped text.
    let body = if body == "-" {
        let read = fields::read_stdin_text("body")?;
        if read.trim().is_empty() {
            return Err(fields::refused("body", "is blank"));
        }
        read
    } else {
        body.to_owned()
    };
    let change = match edited {
        Some(comment) => Change::EditComment {
            comment,
            body: body.clone(),
        },
        None => Change::Comment { body: body.clone() },
    };
    let op = here.store.append(Draft {
        actor: identity.actor.clone(),
        project,
        subject: id,
        change,
    })?;
    let key = here.store.issue(id)?.and_then(|view| view.key);
    let shown = key.clone().unwrap_or_else(|| id.to_string());
    let (text, comment) = match edited {
        Some(comment) => (
            format!("edited comment {} on {shown}\n", short(comment)),
            json!({ "id": comment, "body": body, "edited_utc_ms": op.utc_ms }),
        ),
        None => (
            format!("commented on {shown}\n"),
            json!({
                "id": op.op_id,
                "actor": op.actor,
                "on_behalf_of": op.on_behalf_of,
                "body": body,
                "utc_ms": op.utc_ms,
            }),
        ),
    };
    let data = json!({ "issue": { "id": id, "key": key }, "comment": comment });
    let output = Output::new(text, data);
    Ok((
        here.undoable(output, op.op_id, &identity.actor)?,
        identity.warnings,
    ))
}

/// The comment `which` names on the Issue: `last`, a person's own latest
/// plain comment, or a full id or a unique start or end of one of at least
/// [`MIN_PART`] hex digits. An Agent must name its comment, since other
/// sessions may write under its name.
fn edited_comment(
    here: &Here,
    (issue, reference): (Uuid, &str),
    which: &str,
    actor: &Actor,
) -> Result<Uuid, CliError> {
    let comments = here.store.comments(issue)?;
    if which == "last" {
        if actor.is_agent() {
            return Err(CliError::new(
                "comment_required",
                format!(
                    "{} is an Agent, and other sessions may write as it; name the comment by its id",
                    actor.name()
                ),
            )
            .fix(format!("tm comment {reference} --edit '<comment>' '<text>'")));
        }
        return comments
            .iter()
            .rev()
            .find(|comment| {
                comment.actor.to_lowercase() == actor.name().to_lowercase()
                    && comment.on_behalf_of.is_none()
                    && comment.role.is_none()
                    && !comment.retracted
            })
            .map(|comment| comment.id)
            .ok_or_else(|| {
                CliError::new(
                    "not_found",
                    format!("{} has no comment on {reference} to edit", actor.name()),
                )
                .with("kind", "comment")
            });
    }
    let digits = which.replace('-', "").to_ascii_lowercase();
    if digits.len() < MIN_PART || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(CliError::new(
            "invalid_value",
            format!(
                "{which:?} names no comment; give last, or at least {MIN_PART} hex digits of its id"
            ),
        )
        .with("field", "comment"));
    }
    let matching: Vec<&CommentView> = comments
        .iter()
        .filter(|comment| {
            let hex = comment.id.simple().to_string();
            hex.starts_with(&digits) || hex.ends_with(&digits)
        })
        .collect();
    match matching.as_slice() {
        [] => Err(CliError::new(
            "not_found",
            format!("{reference} has no comment whose id starts or ends with {which}"),
        )
        .with("kind", "comment")),
        // The store lets undo rewrite a hidden comment; a person edits only
        // what they can see.
        [only] if only.retracted => Err(CliError::new(
            "invalid_value",
            format!(
                "comment {} is retracted; undoing its retraction shows it again",
                short(only.id)
            ),
        )
        .with("field", "comment")),
        [only] => Ok(only.id),
        several => Err(CliError::new(
            "invalid_value",
            format!("{which} matches more than one comment on {reference}; give more of it"),
        )
        .with("field", "comment")
        .with(
            "matches",
            several
                .iter()
                .map(|comment| comment.id.to_string())
                .collect::<Vec<_>>(),
        )),
    }
}
