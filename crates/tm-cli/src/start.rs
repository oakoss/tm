//! `tm start`: claim an Issue and begin it in one batch, so one `tm undo`
//! reverts both.

use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::scheme::Category;
use tm_core::{Actor, Change, Op, Scheme};
use tm_store::{Claim, Draft, IssueView};
use uuid::Uuid;

use crate::claim::{self, Lasts};
use crate::edit::{self, Changed};
use crate::error::CliError;
use crate::fields::set;
use crate::follow::{self, Starts};
use crate::here::{self, Here};
use crate::identity;
use crate::output::{Output, printable};
use crate::show;

/// What starting an Issue changes on it, beside its Claim.
#[derive(Default)]
pub(crate) struct Begun {
    /// The Issue as the write read it.
    issue: Option<IssueView>,
    changes: Vec<Change>,
    warnings: Vec<String>,
    /// Its parents the write started too.
    starts: Starts,
}

pub(crate) struct Started {
    pub(crate) id: Uuid,
    pub(crate) claim: Option<Claim>,
    /// A Claim this session already held, kept rather than taken again.
    pub(crate) held: Option<Claim>,
    pub(crate) ops: Vec<Op>,
    pub(crate) begun: Begun,
}

pub(crate) fn run(reference: &str, lasts: &Lasts) -> Result<Output, CliError> {
    here::write(|here| write(here, reference, lasts))
}

fn write(
    here: &mut Here,
    reference: &str,
    lasts: &Lasts,
) -> Result<(Output, Vec<String>), CliError> {
    let id = here.issue(reference)?;
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    let actor = identity.actor.clone();
    let scheme = here.store.scheme().clone();
    let drafts = |lineage: &[IssueView]| begin(&scheme, &actor, lineage);
    // A human starts the work without a Claim, which only an Agent holds.
    let (claim, held, ops, begun) = if actor.is_agent() {
        let ttl_ms = lasts.ttl_ms(here);
        let started = here.store.take_with(&actor, id, ttl_ms, drafts)?;
        let (taken, kept) = if started.kept {
            (None, Some(started.claim))
        } else {
            (Some(started.claim), None)
        };
        (taken, kept, started.ops, started.made)
    } else {
        let (ops, begun) = here.store.append_with_lineage(Vec::new(), id, drafts)?;
        (None, None, ops, begun)
    };
    let started = Started {
        id,
        claim,
        held,
        ops,
        begun,
    };
    let output = report(here, &actor, reference, started)?;
    Ok((output, identity.warnings))
}

/// What a start wrote, as `tm start` and `tm next` print it: the Claim, the
/// changes, a branch name and the Issue.
pub(crate) fn report(
    here: &mut Here,
    actor: &Actor,
    reference: &str,
    started: Started,
) -> Result<Output, CliError> {
    let Started {
        id,
        claim,
        held,
        ops,
        mut begun,
    } = started;
    if !actor.is_agent()
        && let Some(state) = here.store.claim_state(id)?
        && let Some(other) = state.held()
    {
        begun.warnings.push(format!(
            "{} is claimed by {}, who may be working on it",
            begun
                .issue
                .as_ref()
                .and_then(|issue| issue.key.clone())
                .unwrap_or_else(|| id.to_string()),
            claim::holder(other)
        ));
    }
    let issue = begun.issue.ok_or_else(|| {
        CliError::new("not_found", format!("no Issue is {reference}"))
            .with("kind", "issue")
            .with("reference", reference)
    })?;
    let name = issue.key.clone().unwrap_or_else(|| id.to_string());
    let mut text = String::new();
    if let Some(held) = &held {
        let _ = writeln!(text, "{name} is already claimed by {}", claim::holder(held));
    }
    let release = claim.as_ref().map(|claim| {
        let (claimed, release) = claim::taken_lines(&name, claim);
        text.push_str(&claimed);
        release
    });
    for change in &begun.changes {
        if let Change::SetField { field, value, .. } = change {
            let before = issue.fields.get(field).cloned().unwrap_or(Value::Null);
            let line = edit::summary(&Changed {
                field: field.clone(),
                before,
                after: value.clone(),
            });
            let _ = writeln!(text, "{name}  {line}");
        }
    }
    if claim.is_none() && held.is_none() && begun.changes.is_empty() {
        let _ = writeln!(text, "{name}: nothing changed");
    }
    text.extend(release);
    let branch = branch(&issue);
    if let Some(branch) = &branch {
        let _ = writeln!(text, "a branch for it: git switch -c {branch}");
    }
    let mut shown = show::render(here, id)?;
    shown.text = text;
    shown.data["branch"] = json!(branch);
    shown.warnings.extend(begun.warnings);
    let shown = follow::report(&here.store, shown, begun.starts, id)?;
    match ops.first() {
        Some(op) => here.undoable(shown, op.op_id, actor),
        None => Ok(shown),
    }
}

/// The drafts that begin `lineage`'s first Issue: moving it out of triage or
/// todo into its first active Status, and its parents with it, and making the
/// human `actor` works for its Assignee when it has none.
pub(crate) fn begin(scheme: &Scheme, actor: &Actor, lineage: &[IssueView]) -> (Vec<Draft>, Begun) {
    let Some((issue, parents)) = lineage.split_first() else {
        return (Vec::new(), Begun::default());
    };
    let mut begun = Begun::default();
    let name = issue.key.clone().unwrap_or_else(|| issue.id.to_string());
    let mut moved = false;
    if matches!(issue.category, Some(Category::Triage | Category::Todo)) {
        let issue_type = issue
            .fields
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match follow::first(scheme, issue_type, Category::Active) {
            Some(to) => {
                begun.changes.push(set("status", json!(to)));
                moved = true;
            }
            None => begun.warnings.push(format!(
                "{name} was not moved: Issue type {} has no active Status",
                printable(issue_type)
            )),
        }
    } else if follow::finished(issue) {
        begun.warnings.push(format!(
            "{name} was not moved: it is finished; tm move reopens it"
        ));
    }
    if issue.fields.get("assignee").is_none_or(Value::is_null) {
        let human = actor.on_behalf_of().unwrap_or(actor.name());
        begun.changes.push(set("assignee", json!(human)));
    }
    let mut drafts: Vec<Draft> = begun
        .changes
        .iter()
        .map(|change| Draft {
            actor: actor.clone(),
            project: issue.project,
            subject: issue.id,
            change: change.clone(),
        })
        .collect();
    if moved {
        let (more, starts) = follow::starts(scheme, parents, actor);
        drafts.extend(more);
        begun.starts = starts;
    }
    begun.issue = Some(issue.clone());
    (drafts, begun)
}

/// A branch name for `issue`: its Key and the first words of its title, such
/// as `tm-64-claim-and-start-work`. Only a suggestion; tm creates no branch.
fn branch(issue: &IssueView) -> Option<String> {
    let key = issue.key.as_deref()?.to_ascii_lowercase();
    let title = issue
        .fields
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let words: Vec<String> = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(5)
        .map(str::to_owned)
        .collect();
    Some(
        std::iter::once(key)
            .chain(words)
            .collect::<Vec<_>>()
            .join("-"),
    )
}
