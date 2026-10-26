//! `tm move`: an Issue to another Status of its Workflow, with any `--set`
//! fields written in the same batch.

use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::op::ClaimAction;
use tm_core::scheme::{Category, Workflow};
use tm_core::{Actor, Change, Op, ProjectId};
use tm_store::{ClaimState, Draft, IssueView, Named, StoreError};
use uuid::Uuid;

use crate::edit::{self, Changed};
use crate::error::CliError;
use crate::fields::{set, split_pair, typed};
use crate::follow::{self, Starts};
use crate::here::{self, Here};
use crate::identity;
use crate::output::Output;
use crate::show;

/// Retries after another writer claims or releases the Issue mid-move.
const MOVE_TRIES: usize = 2;

/// `--set FIELD=VALUE` for a move: the Status is the move's target.
pub(crate) fn field_value(pair: &str) -> Result<(String, String), String> {
    let (field, value) = split_pair(pair)?;
    match field.as_str() {
        "status" => Err("give the Status as the move's target, not --set".into()),
        "labels" => Err("labels holds several values; tm move cannot set it".into()),
        "parent" => Err("tm move cannot set parent".into()),
        _ => crate::fields::not_membership(&field).map(|()| (field, value)),
    }
}

#[derive(clap::Args)]
pub(crate) struct Move {
    /// The Issue's Key or full id.
    issue: String,
    /// The Status, by id or label, such as `in_progress` or "In Review".
    status: String,
    /// Set a field in the same write, such as assignee=jace; FIELD=- reads
    /// the value from stdin.
    #[arg(long, value_name = "FIELD=VALUE", value_parser = field_value)]
    set: Vec<(String, String)>,
    /// Finish an Issue another Agent session holds a live Claim on, ending
    /// that Claim; only once that session has stopped work on it.
    #[arg(long)]
    force: bool,
}

pub(crate) fn run(args: &Move) -> Result<Output, CliError> {
    crate::fields::given_once(args.set.iter().map(|(field, _)| field))?;
    here::write(|here| write(here, &args.issue, &args.status, &args.set, args.force))
}

fn write(
    here: &mut Here,
    reference: &str,
    target: &str,
    fields: &[(String, String)],
    force: bool,
) -> Result<(Output, Vec<String>), CliError> {
    let id = here.issue(reference)?;
    let scheme = here.store.scheme().clone();
    let view = here.store.issue(id)?.ok_or_else(|| {
        CliError::new("not_found", format!("no Issue is {reference}"))
            .with("kind", "issue")
            .with("reference", reference)
    })?;
    let issue_type = view
        .fields
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_owned();
    let workflow = scheme.workflow(&issue_type).ok_or_else(|| {
        CliError::new(
            "invalid_value",
            format!("{reference} has no type the Scheme knows, so it has no Workflow to move in"),
        )
        .with("field", "type")
        .with(
            "allowed",
            scheme
                .types
                .iter()
                .map(|t| t.id.clone())
                .collect::<Vec<_>>(),
        )
    })?;
    let status = status(workflow, &issue_type, target)?;
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    crate::fields::known(&scheme, fields.iter().map(|(field, _)| field))?;
    // A move into done or cancelled releases the Claim; a review status keeps
    // it (ADR 0009).
    let category = workflow.status(&status).map(|status| status.category);
    let finished = matches!(category, Some(Category::Done | Category::Cancelled));
    if force {
        here.store.force_past_claims();
    }
    let forced = |err| CliError::past_claim(err, "tm move --force");
    // A refusal would lose piped text, so check before reading it; the write
    // checks again.
    if !crate::fields::piped(fields).is_empty() {
        if finished {
            refuse_finish(here, &identity.actor, id, !force).map_err(held_elsewhere)?;
        } else {
            let mut written = crate::fields::written(&scheme, &view, fields, &[]);
            let moves = !view
                .fields
                .get("status")
                .is_some_and(|before| edit::same(before, &json!(status)));
            if moves {
                written.push("status");
            }
            here.store
                .check_protected(&identity.actor, id, &written)
                .map_err(forced)?;
        }
    }
    // Read once all else is checked, so a mistake keeps the piped text; the
    // append below may retry, but this runs once.
    let fields = crate::fields::read_stdin(fields, "leave the field out")?;

    let mut changes = vec![set("status", json!(status))];
    for (field, value) in &fields {
        changes.push(set(field, typed(&scheme, field, value)));
    }
    changes.retain(|change| match change {
        Change::SetField { field, value, .. } => !view
            .fields
            .get(field)
            .is_some_and(|before| edit::same(before, value)),
        _ => true,
    });
    // A finished Issue a Claim still holds is let go by moving it again.
    if changes.is_empty() && !(finished && here.store.claim_state(id)?.is_some()) {
        return Ok((unchanged(here, &view)?, identity.warnings));
    }
    let draft = |change| Draft {
        actor: identity.actor.clone(),
        project: view.project,
        subject: id,
        change,
    };
    let drafts = || changes.iter().cloned().map(draft).collect::<Vec<_>>();
    let (ops, starts) = if category == Some(Category::Active) {
        here.store
            .append_with_lineage(drafts(), id, |lineage| {
                let parents = lineage.issues().get(1..).unwrap_or_default();
                follow::starts(&scheme, parents, lineage, &identity.actor)
            })
            .map_err(forced)?
    } else if !finished {
        let ops = here.store.append_all(drafts()).map_err(forced)?;
        (ops, Starts::default())
    } else {
        let ops = finish(here, &identity.actor, (view.project, id), drafts, !force)
            .map_err(held_elsewhere)?;
        // Another session can release the Claim this move came to release.
        if ops.is_empty() {
            return Ok((unchanged(here, &view)?, identity.warnings));
        }
        (ops, Starts::default())
    };
    let mut shown = show::render(here, id)?;
    shown.text = lines(&view, &changes, &ops);
    let shown = follow::report(&here.store, shown, starts, id)?;
    let output = here.undoable(shown, ops[0].op_id, &identity.actor)?;
    Ok((output, identity.warnings))
}

fn unchanged(here: &Here, view: &IssueView) -> Result<Output, CliError> {
    let name = view.key.clone().unwrap_or_else(|| view.id.to_string());
    let mut output = show::render(here, view.id)?;
    output.text = format!("{name}: nothing changed\n");
    output.data["batch"] = Value::Null;
    Ok(output)
}

/// A finish refused for another session's Claim says what overrides it, in
/// the message and never as a `fix`, so it is not run by rote.
fn held_elsewhere(err: StoreError) -> CliError {
    let refused = matches!(err, StoreError::AlreadyClaimed { .. });
    let mut err = CliError::from(err);
    if refused {
        err.message
            .push_str("; finishing it would end that Claim, which tm move --force does");
    }
    err
}

/// Writes `drafts` for a move of `issue` into done or cancelled, releasing
/// its Claim in the same batch. With `guarded`, an Agent's move is refused
/// with [`StoreError::AlreadyClaimed`] while another session holds a live
/// Claim (ADR 0009).
pub(crate) fn finish(
    here: &mut Here,
    actor: &Actor,
    (project, issue): (ProjectId, Uuid),
    drafts: impl Fn() -> Vec<Draft>,
    guarded: bool,
) -> Result<Vec<Op>, StoreError> {
    let mut tries = 0;
    loop {
        let mut drafts = drafts();
        let state = refuse_finish(here, actor, issue, guarded)?;
        // The release names the Claim read here, so the write refuses one
        // that was taken, renewed or released since, and the move reads it
        // and judges it again. A lapsed Claim is released too, so finished
        // work holds none.
        let held = state.map(|state| state.claim().clone());
        if let Some(claim) = &held {
            drafts.push(Draft {
                actor: actor.clone(),
                project,
                subject: issue,
                change: Change::Claim {
                    action: ClaimAction::Release { claim: claim.id },
                },
            });
        }
        match here.store.append_if_claim(drafts, issue, held.as_ref()) {
            Err(StoreError::ClaimChanged(_)) if tries < MOVE_TRIES => tries += 1,
            written => break written,
        }
    }
}

/// `issue`'s Claim, unless `guarded` refuses an Agent's finish of it while
/// another session holds it.
fn refuse_finish(
    here: &Here,
    actor: &Actor,
    issue: Uuid,
    guarded: bool,
) -> Result<Option<ClaimState>, StoreError> {
    let state = here.store.claim_state(issue)?;
    if guarded
        && actor.is_agent()
        && let Some(ClaimState::Held(holder)) = &state
        && !holder.taken_by(actor)
    {
        let key = here.store.issue(issue)?.and_then(|view| view.key);
        return Err(StoreError::AlreadyClaimed {
            issue: Named {
                key,
                ..issue.into()
            },
            holder: Box::new(holder.clone()),
        });
    }
    Ok(state)
}

/// A line for each field the move set, as `tm edit` prints them, and one for
/// a released Claim; `--json` keeps the whole Issue.
fn lines(view: &IssueView, changes: &[Change], ops: &[Op]) -> String {
    let name = view.key.clone().unwrap_or_else(|| view.id.to_string());
    let mut text = String::new();
    for change in changes {
        if let Change::SetField { field, value, .. } = change {
            let before = view.fields.get(field).cloned().unwrap_or(Value::Null);
            if edit::same(&before, value) {
                continue;
            }
            let line = edit::summary(&Changed {
                field: field.clone(),
                before,
                after: value.clone(),
            });
            let _ = writeln!(text, "{name}  {line}");
        }
    }
    // A move to an active Status may also carry renewals of other Claims.
    if ops.iter().any(|op| {
        op.subject == view.id
            && matches!(
                op.change(),
                Ok(Change::Claim {
                    action: ClaimAction::Release { .. }
                })
            )
    }) {
        let _ = writeln!(text, "{name}  Claim released");
    }
    if text.is_empty() {
        let _ = writeln!(text, "{name}: nothing changed");
    }
    text
}

/// The Status `target` names in `workflow`, by id or label without case. A
/// category name is refused with that category's Statuses, never guessed.
fn status(workflow: &Workflow, issue_type: &str, target: &str) -> Result<String, CliError> {
    if let Some(status) = workflow.statuses.iter().find(|status| {
        status.id.eq_ignore_ascii_case(target) || status.label.eq_ignore_ascii_case(target)
    }) {
        return Ok(status.id.clone());
    }
    let category = Category::ALL
        .into_iter()
        .find(|category| category.id().eq_ignore_ascii_case(target));
    let allowed: Vec<String> = workflow
        .statuses
        .iter()
        .filter(|status| category.is_none_or(|category| status.category == category))
        .map(|status| status.id.clone())
        .collect();
    let message = match category {
        Some(_) => format!(
            "{target} is a Status category; for Issue type {issue_type} it is one of: {}",
            allowed.join(", ")
        ),
        None => format!(
            "{target:?} is not a Status of Issue type {issue_type}; one of: {}",
            allowed.join(", ")
        ),
    };
    Err(CliError::new("invalid_value", message)
        .with("field", "status")
        .with("allowed", allowed))
}

#[cfg(test)]
mod tests {
    use tm_core::Scheme;

    use super::*;

    #[test]
    fn a_target_is_a_status_by_id_or_label_and_a_category_is_never_guessed() {
        let scheme = Scheme::built_in();
        let workflow = scheme.workflow("task").unwrap();
        assert_eq!(
            status(workflow, "task", "in_progress").unwrap(),
            "in_progress"
        );
        assert_eq!(
            status(workflow, "task", "In Progress").unwrap(),
            "in_progress"
        );
        assert_eq!(status(workflow, "task", "DONE").unwrap(), "done");
        let err = status(workflow, "task", "active").unwrap_err();
        assert_eq!(err.code, "invalid_value");
        assert_eq!(err.details["allowed"], json!(["in_progress", "in_review"]));
        let err = status(workflow, "task", "shipped").unwrap_err();
        assert_eq!(
            err.details["allowed"].as_array().unwrap().len(),
            workflow.statuses.len()
        );
    }
}
