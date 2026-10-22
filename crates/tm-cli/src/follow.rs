//! Parents follow their children: a child entering an active Status starts
//! each parent still in triage or todo in the same batch, and a child
//! finishing or reopening prints the parent's matching move without making
//! it, since a parent can carry work of its own. Only `tm move` and
//! `tm create` follow; an import records what happened elsewhere.

use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::Actor;
use tm_core::scheme::{Category, Scheme};
use tm_store::{Draft, IssueView, Store, StoreError};
use uuid::Uuid;

use crate::fields::set;
use crate::output::{Output, printable};

/// The parents a write starts.
#[derive(Default)]
pub(crate) struct Starts {
    /// Each started parent's text line and JSON entry.
    started: Vec<(String, Value)>,
    warnings: Vec<String>,
}

/// The moves starting each of `candidates` still in triage or todo, into the
/// first active Status of its own Workflow.
pub(crate) fn starts<'a>(
    scheme: &Scheme,
    candidates: impl IntoIterator<Item = &'a IssueView>,
    actor: &Actor,
) -> (Vec<Draft>, Starts) {
    let mut drafts = Vec::new();
    let mut starts = Starts::default();
    for parent in candidates {
        if !matches!(parent.category, Some(Category::Triage | Category::Todo)) {
            continue;
        }
        let name = name(parent);
        if archived(parent) {
            starts
                .warnings
                .push(format!("{name} was not started: it is archived"));
            continue;
        }
        let issue_type = text(parent, "type");
        let Some(to) = first(scheme, issue_type, Category::Active) else {
            starts.warnings.push(format!(
                "{name} was not started: Issue type {issue_type} has no active Status"
            ));
            continue;
        };
        let from = parent
            .fields
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_owned);
        drafts.push(Draft {
            actor: actor.clone(),
            project: parent.project,
            subject: parent.id,
            change: set("status", json!(to)),
        });
        starts.started.push((
            format!(
                "also started {name} ({} → {to})",
                printable(from.as_deref().unwrap_or("none"))
            ),
            json!({ "id": parent.id, "key": parent.key, "from": from, "to": to }),
        ));
    }
    (drafts, starts)
}

/// `output` with the parents `starts` moved, and the move `child`'s parent
/// may want now: closing it when `child` finished its last open child, or
/// reopening it when `child` is open under a finished parent.
pub(crate) fn report(
    store: &Store,
    mut output: Output,
    starts: Starts,
    child: Uuid,
) -> Result<Output, StoreError> {
    let (lines, started): (Vec<String>, Vec<Value>) = starts.started.into_iter().unzip();
    output.warnings.extend(starts.warnings);
    for line in &lines {
        let _ = writeln!(output.text, "{line}");
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
fn next(
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
    let key = name(&parent);
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
            name(&child)
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
        .statuses
        .iter()
        .find(|status| status.category == category)
        .map(|status| status.id.clone())
}

pub(crate) fn finished(issue: &IssueView) -> bool {
    matches!(issue.category, Some(Category::Done | Category::Cancelled))
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

fn name(issue: &IssueView) -> String {
    issue.key.clone().unwrap_or_else(|| issue.id.to_string())
}
