//! `tm move`: an Issue to another Status of its Workflow, with any `--set`
//! fields written in the same batch.

use serde_json::json;
use tm_core::Change;
use tm_core::op::ClaimAction;
use tm_core::scheme::{Category, Workflow};
use tm_store::{Draft, StoreError};

use crate::error::CliError;
use crate::fields::{set, split_pair, typed};
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
    let finished = workflow
        .status(&status)
        .is_some_and(|status| matches!(status.category, Category::Done | Category::Cancelled));
    let draft = |change| Draft {
        actor: identity.actor.clone(),
        project: view.project,
        subject: id,
        change,
    };
    let mut tries = 0;
    let ops = loop {
        let mut drafts: Vec<Draft> = changes.iter().cloned().map(draft).collect();
        if !finished {
            break here.store.append_all(drafts)?;
        }
        // The release names the Claim read here, so the write refuses one that
        // was claimed or released since and the move reads it again.
        let held = here.store.claim(id)?.map(|claim| claim.id);
        if let Some(claim) = held {
            drafts.push(draft(Change::Claim {
                action: ClaimAction::Release { claim },
            }));
        }
        match here.store.append_if_claim(drafts, id, held) {
            Err(StoreError::ClaimChanged(_)) if tries < MOVE_TRIES => tries += 1,
            written => break written?,
        }
    };
    let shown = show::render(here, id)?;
    let output = here.undoable(shown, ops[0].op_id)?;
    Ok((output, identity.warnings))
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
