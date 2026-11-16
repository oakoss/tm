//! `tm move`: an Issue to another Status of its Workflow, with any `--set`
//! fields written in the same batch.

use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::op::ClaimAction;
use tm_core::scheme::{Category, Workflow};
use tm_core::{Actor, Change, Op};
use tm_store::{Draft, IssueView, StoreError, WriteOptions};

use crate::edit::{self, Changed};
use crate::error::CliError;
use crate::fields::{set, split_pair, typed};
use crate::follow;
use crate::here::{self, Here};
use crate::output::Output;
use crate::show;

/// `--set FIELD=VALUE` for a move: the Status is the move's target.
pub(crate) fn field_value(pair: &str) -> Result<(String, String), String> {
    let (field, value) = split_pair(pair)?;
    match field.as_str() {
        "status" => Err("give the Status as the move's target, not --set".into()),
        "labels" => Err("labels holds several values; tm move cannot set it".into()),
        "parent" => Err("tm move cannot set parent".into()),
        _ => crate::fields::not_owned(&field).map(|()| (field, value)),
    }
}

#[derive(clap::Args)]
pub(crate) struct Move {
    /// The Issue's Key or full id.
    issue: String,
    /// The Status, by id or label, such as `in_progress`, `done` or "In
    /// Review"; `tm show ISSUE` lists those it can move to.
    status: String,
    /// Set a field in the same write, such as assignee=jace; FIELD=- reads
    /// the value from stdin.
    #[arg(long, value_name = "FIELD=VALUE", value_parser = field_value)]
    set: Vec<(String, String)>,
    /// Finish an Issue another Agent session holds a live Claim on, ending
    /// that Claim; only once that session has stopped work on it.
    #[arg(long)]
    force: bool,
    /// Refuse the write if a field it sets changed after this version, the
    /// one tm show printed when you read the Issue.
    #[arg(long, value_name = "VERSION")]
    if_version: Option<u64>,
}

impl Move {
    /// `tm move ISSUE STATUS --if-version VERSION`, as `tm tui` makes it.
    pub(crate) fn to(issue: &str, status: &str, if_version: Option<u64>) -> Self {
        Self {
            issue: issue.to_owned(),
            status: status.to_owned(),
            set: Vec::new(),
            force: false,
            if_version,
        }
    }
}

/// The version a write is based on: `given` by `--if-version`, else the one
/// `read` with the Issue. One too large for the log is ahead of any Issue,
/// and refused as such.
pub(crate) fn based_on(given: Option<u64>, read: i64) -> i64 {
    given.map_or(read, |given| i64::try_from(given).unwrap_or(i64::MAX))
}

pub(crate) fn run(args: &Move) -> Result<Output, CliError> {
    crate::fields::given_once(args.set.iter().map(|(field, _)| field))?;
    here::write(|here| write(here, args))
}

pub(crate) fn write(here: &mut Here, args: &Move) -> Result<(Output, Vec<String>), CliError> {
    let (reference, target) = (args.issue.as_str(), args.status.as_str());
    let (fields, force) = (args.set.as_slice(), args.force);
    let id = here.issue(reference)?;
    let scheme = here.store.scheme().clone();
    let (view, read) = here.store.issue_with_version(id)?.ok_or_else(|| {
        CliError::new("not_found", format!("no Issue is {reference}"))
            .with("kind", "issue")
            .with("reference", reference)
    })?;
    let based_on = based_on(args.if_version, read);
    let options = WriteOptions {
        force,
        based_on: Some(based_on),
        ..WriteOptions::default()
    };
    let issue_type = view.issue_type().unwrap_or_default().to_owned();
    let workflow = scheme.workflow(&issue_type).ok_or_else(|| {
        CliError::new(
            "invalid_value",
            format!("{reference} has no type the Scheme knows, so it has no Workflow to move in"),
        )
        .with("field", "type")
        .with("allowed", scheme.type_ids())
    })?;
    let status = status(workflow, &issue_type, target)?;
    let identity = here.writer()?;
    crate::fields::known(&scheme, fields.iter().map(|(field, _)| field))?;
    // A move into done or cancelled releases the Claim; a review status keeps
    // it (ADR 0009).
    let category = workflow.status(&status).map(|status| status.category);
    let finished = category.is_some_and(Category::is_closed);
    let forced = |err| CliError::past_claim(err, "tm move --force");
    if !crate::fields::piped(fields).is_empty() {
        let to = (status.as_str(), finished);
        refuse_early(here, &identity.actor, &view, to, args, based_on)?;
    }
    // Read once all else is checked, so a mistake keeps the piped text.
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
    // A finished Issue a Claim or a question still holds is let go by moving
    // it again.
    let lets_go = || -> Result<bool, StoreError> {
        let asked = view.fields.contains_key(tm_core::scheme::WAITING_ON);
        Ok(asked || here.store.claim_state(id)?.is_some())
    };
    if changes.is_empty() && !(finished && lets_go()?) {
        // A version not read from the Issue is a mistake even here.
        here.store.check_version(id, based_on, &[])?;
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
        let (written, starts) = here
            .store
            .write_starting_parents(&identity.actor, drafts(), id, &options)
            .map_err(forced)?;
        (here.passing(written), starts)
    } else if !finished {
        let written = here.store.write(drafts(), &options).map_err(forced)?;
        (here.passing(written), Vec::new())
    } else {
        let issue = (view.project, id);
        let written = here
            .store
            .finish(&identity.actor, issue, drafts(), (!force, &options))
            .map_err(held_elsewhere)?;
        let ops = here.passing(written);
        // Another session can release the Claim this move came to release.
        if ops.is_empty() {
            return Ok((unchanged(here, &view)?, identity.warnings));
        }
        (ops, Vec::new())
    };
    let mut shown = show::render(here, id)?;
    shown.text = lines(&view, &ops);
    let shown = follow::report(&here.store, shown, starts, id)?;
    let output = here.undoable(shown, ops[0].op_id, &identity.actor)?;
    Ok((output, identity.warnings))
}

fn unchanged(here: &Here, view: &IssueView) -> Result<Output, CliError> {
    let name = view.shown_key();
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

/// Refuses a move to `status` as its write would, before piped text is read,
/// since a refusal after would lose it; the write checks again.
fn refuse_early(
    here: &Here,
    actor: &Actor,
    view: &IssueView,
    (status, finished): (&str, bool),
    args: &Move,
    based_on: i64,
) -> Result<(), CliError> {
    let mut written = crate::fields::written(here.store.scheme(), view, &args.set, &[]);
    let moves = !view
        .fields
        .get("status")
        .is_some_and(|before| edit::same(before, &json!(status)));
    if moves {
        written.push("status");
    }
    if finished {
        if !args.force {
            here.store
                .check_finish(actor, view.id)
                .map_err(held_elsewhere)?;
        }
    } else {
        here.store
            .check_protected(actor, view.id, &written, args.force)
            .map_err(|err| CliError::past_claim(err, "tm move --force"))?;
    }
    Ok(here.store.check_version(view.id, based_on, &written)?)
}

/// A line for each field the move set, as `tm edit` prints them, and one for
/// a released Claim; `--json` keeps the whole Issue.
fn lines(view: &IssueView, ops: &[Op]) -> String {
    let name = view.shown_key();
    let mut text = String::new();
    let written = ops
        .iter()
        .filter(|op| op.subject == view.id)
        .filter_map(|op| op.change().ok());
    for change in written {
        if let Change::SetField { field, value, .. } = change {
            let before = view.fields.get(&field).cloned().unwrap_or(Value::Null);
            if edit::same(&before, &value) {
                continue;
            }
            let line = edit::summary(&Changed {
                field,
                before,
                after: value,
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
    if let Some(status) = workflow.named(target) {
        return Ok(status.id.clone());
    }
    let category = Category::ALL
        .into_iter()
        .find(|category| category.id().eq_ignore_ascii_case(target));
    let allowed: Vec<String> = match category {
        Some(category) => workflow
            .of_category(category)
            .map(|status| status.id.clone())
            .collect(),
        None => workflow
            .statuses
            .iter()
            .map(|status| status.id.clone())
            .collect(),
    };
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
    fn a_finished_issue_still_waiting_on_a_question_is_let_go_by_moving_it_again() {
        let at = here::fixture();
        let mut here = here::open_at(&at).unwrap();
        let jace = Actor::human("jace");
        let project = here.project().unwrap();
        let (id, _) = here
            .store
            .create_issue(&jace, project, "task", "alpha")
            .unwrap();
        write(&mut here, &Move::to("TM-1", "done", None)).unwrap();
        let asked = Draft {
            actor: jace,
            project,
            subject: id,
            change: set("waiting_on", json!("jace")),
        };
        let written = here.store.append(asked).unwrap();
        here.passing(written);
        let (output, _) = write(&mut here, &Move::to("TM-1", "done", None)).unwrap();
        assert!(
            output.text.starts_with("TM-1  waiting_on:"),
            "{}",
            output.text
        );
        let view = here.store.issue(id).unwrap().unwrap();
        assert_eq!(view.fields.get("waiting_on"), None);
    }

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
