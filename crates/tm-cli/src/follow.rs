//! Parents follow their children: the store starts them as
//! [`tm_store::Lineage::start_parents`] says, and a child finishing or reopening
//! prints the parent's matching move without making it, since a parent can
//! carry work of its own. An import does not follow: it records what
//! happened elsewhere.

use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::scheme::{Category, MoveGap, Scheme};
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
                let shown = printable(&from);
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
            ParentOutcome::NoActiveMove(from) => {
                format!(
                    "its Workflow allows no active Status from {}",
                    printable(&from)
                )
            }
            ParentOutcome::Lacking { to, fields } => {
                let fix = ["tm", "edit", name.as_str()]
                    .map(crate::fields::quoted)
                    .into_iter()
                    .chain(
                        fields
                            .iter()
                            .map(|field| crate::fields::placeholder(store.scheme(), field)),
                    );
                format!(
                    "{} requires {}; {}",
                    printable(&to),
                    fields.join(", "),
                    fix.collect::<Vec<_>>().join(" ")
                )
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
    let from = Some(text(&parent, "status")).filter(|status| !status.is_empty());
    let to = match first(store.scheme(), text(&parent, "type"), from, category) {
        Ok(to) => to,
        Err(no) => {
            warnings.push(format!("tm cannot suggest {verb} {key}: {}", no.told()));
            return Ok(None);
        }
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

/// Why an Issue has no Status of a category to move to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NoMove {
    pub(crate) reason: String,
    /// The moves its Workflow allows instead, when it has a Status of the
    /// category but none it may reach; `None` when it has none at all.
    pub(crate) allowed: Option<Vec<String>>,
}

impl NoMove {
    /// The reason, then where the Issue can move instead when it has moves.
    pub(crate) fn told(&self) -> String {
        match self.allowed.as_deref() {
            Some([]) => format!("{}; it can move nowhere", self.reason),
            Some(allowed) => format!("{}; it can move to {}", self.reason, allowed.join(", ")),
            None => self.reason.clone(),
        }
    }
}

/// The first Status of `category` an Issue of `issue_type` in `from` may move
/// to, or why there is none.
pub(crate) fn first(
    scheme: &Scheme,
    issue_type: &str,
    from: Option<&str>,
    category: Category,
) -> Result<String, NoMove> {
    let Some(workflow) = scheme.workflow(issue_type) else {
        return Err(NoMove {
            reason: format!(
                "Issue type {} is not in the Scheme; tm scheme show lists the types",
                printable(issue_type)
            ),
            allowed: None,
        });
    };
    let name = category.id();
    match workflow.first_move(from, category) {
        Ok(status) => Ok(status.id.clone()),
        Err(MoveGap::Unreachable(moves)) => Err(NoMove {
            reason: format!(
                "its Workflow allows no {name} Status from {}",
                printable(from.unwrap_or_default())
            ),
            allowed: Some(moves.into_iter().map(|status| status.id.clone()).collect()),
        }),
        Err(MoveGap::NoStatus) => Err(NoMove {
            reason: format!("Issue type {} has no {name} Status", printable(issue_type)),
            allowed: None,
        }),
    }
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

#[cfg(test)]
mod tests {
    use tm_core::scheme::Transitions;

    use super::*;

    #[test]
    fn the_first_move_of_a_category_says_why_there_is_none() {
        let mut scheme = Scheme::built_in();
        scheme.workflows[0].transitions =
            Transitions::Listed([("todo".to_owned(), vec!["cancelled".to_owned()])].into());
        let first = |from, category| first(&scheme, "task", Some(from), category);
        assert_eq!(
            first("todo", Category::Cancelled),
            Ok("cancelled".to_owned())
        );
        assert_eq!(
            first("todo", Category::Done),
            Err(NoMove {
                reason: "its Workflow allows no done Status from todo".to_owned(),
                allowed: Some(vec!["cancelled".to_owned()]),
            })
        );
        assert_eq!(first("in_review", Category::Done), Ok("done".to_owned()));
        assert_eq!(
            super::first(&scheme, "gone", Some("todo"), Category::Done),
            Err(NoMove {
                reason: "Issue type gone is not in the Scheme; tm scheme show lists the types"
                    .to_owned(),
                allowed: None,
            })
        );
    }
}
