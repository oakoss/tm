//! `tm move`: an Issue to another Status of its Workflow, with any `--set`
//! fields written in the same batch.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::op::ClaimAction;
use tm_core::scheme::{Category, Workflow};
use tm_core::{Change, Op};
use tm_store::{Draft, IssueView, StoreError};

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
        _ => Ok((field, value)),
    }
}

pub(crate) fn run(
    reference: &str,
    target: &str,
    set: &[(String, String)],
) -> Result<Output, CliError> {
    here::write(|here| write(here, reference, target, set))
}

fn write(
    here: &mut Here,
    reference: &str,
    target: &str,
    fields: &[(String, String)],
) -> Result<(Output, Vec<String>), CliError> {
    let mut given = BTreeSet::new();
    for (field, _) in fields {
        if !given.insert(field) {
            return Err(
                CliError::new("invalid_value", format!("{field} is given more than once"))
                    .with("field", field.as_str()),
            );
        }
    }
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

    let mut changes = vec![set("status", json!(status))];
    for (field, value) in fields {
        changes.push(set(field, typed(&scheme, field, value)));
    }
    // A move into done or cancelled releases the Claim; a review status keeps
    // it (ADR 0009).
    let category = workflow.status(&status).map(|status| status.category);
    let finished = matches!(category, Some(Category::Done | Category::Cancelled));
    let draft = |change| Draft {
        actor: identity.actor.clone(),
        project: view.project,
        subject: id,
        change,
    };
    let drafts = || changes.iter().cloned().map(draft).collect::<Vec<_>>();
    let (ops, starts) = if category == Some(Category::Active) {
        here.store.append_with_lineage(drafts(), id, |lineage| {
            let parents = lineage.iter().filter(|issue| issue.id != id);
            follow::starts(&scheme, parents, &identity.actor)
        })?
    } else if !finished {
        (here.store.append_all(drafts())?, Starts::default())
    } else {
        let mut tries = 0;
        loop {
            let mut drafts = drafts();
            // The release names the Claim read here, so the write refuses one
            // that was claimed or released since and the move reads it again.
            // A lapsed Claim is released too, so finished work holds none.
            let held = here.store.claim_state(id)?.map(|state| state.claim().id);
            if let Some(claim) = held {
                drafts.push(draft(Change::Claim {
                    action: ClaimAction::Release { claim },
                }));
            }
            match here.store.append_if_claim(drafts, id, held) {
                Err(StoreError::ClaimChanged(_)) if tries < MOVE_TRIES => tries += 1,
                written => break (written?, Starts::default()),
            }
        }
    };
    let mut shown = show::render(here, id)?;
    shown.text = lines(&view, &changes, &ops);
    let shown = follow::report(&here.store, shown, starts, id)?;
    let output = here.undoable(shown, ops[0].op_id, &identity.actor)?;
    Ok((output, identity.warnings))
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
