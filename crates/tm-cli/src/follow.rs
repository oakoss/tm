//! Parents follow their children: the store starts them as
//! [`tm_store::start_parents`] says, and a child finishing or reopening
//! prints the parent's matching move without making it, since a parent can
//! carry work of its own. An import does not follow: it records what
//! happened elsewhere.

use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::scheme::{Category, Scheme};
use tm_store::{IssueView, ParentOutcome, ParentStart, Store, StoreError};
use uuid::Uuid;

use crate::output::{Output, printable};

/// `output` with the parents `starts` moved or left, and the move `child`'s
/// parent may want now: closing it when `child` finished its last open child,
/// or reopening it when `child` is open under a finished parent.
pub(crate) fn report(
    store: &Store,
    mut output: Output,
    starts: Vec<ParentStart>,
    child: Uuid,
) -> Result<Output, StoreError> {
    let mut started = Vec::new();
    for start in starts {
        let name = tm_store::shown_key(start.key.as_deref(), start.id);
        let why = match start.outcome {
            ParentOutcome::Started { from, to } => {
                let shown = printable(from.as_deref().unwrap_or("none"));
                let _ = writeln!(output.text, "also started {name} ({shown} → {to})");
                started.push(json!({ "id": start.id, "key": start.key, "from": from, "to": to }));
                continue;
            }
            ParentOutcome::Archived => "it is archived".to_owned(),
            ParentOutcome::Held(holder) => {
                format!("{} holds it", crate::claim::holder(&holder))
            }
            ParentOutcome::NoActiveStatus(issue_type) => {
                format!("Issue type {issue_type} has no active Status")
            }
        };
        output
            .warnings
            .push(format!("{name} was not started: {why}"));
    }
    let mut commands = Vec::new();
    if let Some((message, command)) = next(store, child, &mut output.warnings)? {
        let _ = writeln!(output.text, "{message}");
        commands.push(command);
    }
    output.data["started"] = json!(started);
    output.data["next"] = json!(commands);
    Ok(output)
}

/// The suggested move and its command, or a warning in `warnings` when the
/// parent's Workflow has no Status to suggest.
pub(crate) fn next(
    store: &Store,
    child: Uuid,
    warnings: &mut Vec<String>,
) -> Result<Option<(String, String)>, StoreError> {
    let Some(child) = store.issue(child)? else {
        return Ok(None);
    };
    let Some(parent) = child
        .fields
        .get("parent")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
    else {
        return Ok(None);
    };
    let Some(parent) = store.issue(parent)? else {
        return Ok(None);
    };
    if archived(&parent) {
        return Ok(None);
    }
    let reopen = open(&child) && finished(&parent);
    let close = !open(&child) && open(&parent) && !store.children(parent.id)?.iter().any(open);
    if !(reopen || close) {
        return Ok(None);
    }
    let key = parent.shown_key();
    let (category, verb) = if reopen {
        (Category::Active, "reopening")
    } else {
        (Category::Done, "closing")
    };
    let Some(to) = first(store.scheme(), text(&parent, "type"), category) else {
        warnings.push(format!(
            "{key} has no {} Status, so tm cannot suggest {verb} it",
            category.id()
        ));
        return Ok(None);
    };
    let command = format!("tm move {key} {to}");
    let message = if reopen {
        format!(
            "{key} is {} but {} is open; reopen it with: {command}",
            printable(text(&parent, "status")),
            child.shown_key()
        )
    } else {
        format!("{key} has no open children left; close it with: {command}")
    };
    Ok(Some((message, command)))
}

/// The first Status of `category` in `issue_type`'s Workflow.
pub(crate) fn first(scheme: &Scheme, issue_type: &str, category: Category) -> Option<String> {
    scheme
        .workflow(issue_type)?
        .of_category(category)
        .next()
        .map(|status| status.id.clone())
}

pub(crate) fn finished(issue: &IssueView) -> bool {
    issue.category.is_some_and(Category::is_closed)
}

/// Open as the ready queue counts it: not finished and not archived.
fn open(issue: &IssueView) -> bool {
    !finished(issue) && !archived(issue)
}

pub(crate) fn archived(issue: &IssueView) -> bool {
    issue.fields.get("archived") == Some(&json!(true))
}

fn text<'a>(issue: &'a IssueView, field: &str) -> &'a str {
    issue
        .fields
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
}
