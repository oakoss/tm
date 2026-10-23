//! `tm undo`: one command's writes reverted, named by the batch it printed.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::op::COMMENTS_FIELD;
use tm_core::{Actor, link};
use tm_store::{Restore, Target};
use uuid::Uuid;

use crate::error::CliError;
use crate::here::{self, Here};
use crate::identity;
use crate::output::{Output, printable, short};
use crate::show;

/// Fewer characters would too often match more than one batch.
const MIN_PART: usize = 8;

pub(crate) fn run(reference: Option<&str>, dry_run: bool) -> Result<Output, CliError> {
    here::write(|here| undo(here, reference, dry_run))
}

fn undo(
    here: &mut Here,
    reference: Option<&str>,
    dry_run: bool,
) -> Result<(Output, Vec<String>), CliError> {
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    let (batch, next) = match reference {
        Some(reference) => (batch(here, reference)?, None),
        None => last(here, &identity.actor)?,
    };
    let undone = if dry_run {
        here.store
            .undo_plan(&identity.actor, batch)
            .map(|plan| (plan, None))
    } else {
        here.store
            .undo(&identity.actor, batch)
            .map(|(plan, ops)| (plan, ops.first().map(|op| op.op_id)))
    };
    let (plan, written) = undone.map_err(|err| {
        let err = CliError::from(err);
        match next {
            Some(next) if matches!(err.code, "undo_conflict" | "cannot_undo") => {
                err.fix(format!("tm undo {}", short(next)))
            }
            _ => err,
        }
    })?;

    // A Link is filed under one end, but both ends' `show` change.
    let touched: BTreeSet<Uuid> = plan
        .restores
        .iter()
        .flat_map(|restore| [issue_of(restore), link_target(restore)])
        .flatten()
        .chain(plan.released.iter().copied())
        .collect();
    let mut issues = Vec::new();
    let mut keys = HashMap::new();
    for issue in &touched {
        let shown = show::render(here, *issue)?;
        keys.insert(*issue, shown.data["key"].clone());
        issues.push(shown.data);
    }
    let name = |issue: Uuid| {
        keys.get(&issue)
            .and_then(Value::as_str)
            .map_or_else(|| issue.to_string(), str::to_owned)
    };
    let restored: Vec<Value> = plan
        .restores
        .iter()
        .map(|restore| {
            let mut entry = json!(restore);
            if let Some(issue) = issue_of(restore) {
                entry["key"] = keys.get(&issue).cloned().unwrap_or(Value::Null);
            }
            entry
        })
        .collect();

    let mut text = String::new();
    let _ = writeln!(
        text,
        "{} {batch}:",
        if dry_run {
            "undoing would revert"
        } else {
            "undid"
        }
    );
    for restore in &plan.restores {
        let _ = writeln!(text, "  {}", describe(restore, &name));
    }
    for issue in &plan.released {
        let _ = writeln!(
            text,
            "  {}'s Claim stays released; its Agent takes it again with tm claim",
            name(*issue)
        );
    }
    let released: Vec<Value> = plan
        .released
        .iter()
        .map(|issue| json!({ "id": issue, "key": keys.get(issue) }))
        .collect();
    let data = json!({
        "undone": batch,
        "dry_run": dry_run,
        "restored": restored,
        "released": released,
        "issues": issues,
        "batch": null,
    });
    let output = Output::new(text, data);
    let output = match written {
        Some(written) => here.undoable(output, written, &identity.actor)?,
        None => output,
    };
    Ok((output, identity.warnings))
}

/// The newest batch a bare undo reverts, and the one before it, which a
/// refusal offers instead (ADR 0012). An Agent must name its batch, since
/// other sessions may write under its name.
fn last(here: &Here, actor: &Actor) -> Result<(Uuid, Option<Uuid>), CliError> {
    if actor.is_agent() {
        return Err(CliError::new(
            "batch_required",
            format!(
                "{} is an Agent, and other sessions may write as it; name the batch its write printed",
                actor.name()
            ),
        )
        .fix("tm undo <batch>"));
    }
    match here.store.undo_candidates(actor.name(), 2)?.as_slice() {
        [] => Err(CliError::new(
            "not_found",
            format!("{} has nothing on this Replica to undo", actor.name()),
        )
        .with("kind", "batch")),
        [newest, rest @ ..] => Ok((*newest, rest.first().copied())),
    }
}

const fn issue_of(restore: &Restore) -> Option<Uuid> {
    match restore.target {
        Target::Issue { issue } => Some(issue),
        Target::Shortlist { .. } => None,
    }
}

/// The other end of a restored Link, from its element `type/target`.
fn link_target(restore: &Restore) -> Option<Uuid> {
    if restore.field != link::FIELD {
        return None;
    }
    let (_, target) = restore.elem.as_deref()?.split_once('/')?;
    Uuid::parse_str(target).ok()
}

/// One restore as a person reads it: Keys for Issues, and a set element
/// added or removed rather than true or false.
fn describe(restore: &Restore, name: &impl Fn(Uuid) -> String) -> String {
    let issue = match &restore.target {
        Target::Issue { issue } => name(*issue),
        Target::Shortlist {
            name: Some(list), ..
        } => format!("Short-list {}", printable(list)),
        Target::Shortlist { shortlist, .. } => format!("Short-list {shortlist}"),
    };
    let present = restore.to == json!(true);
    match (restore.field.as_str(), restore.elem.as_deref()) {
        (field, Some(elem)) if field == link::FIELD => {
            let link_type = elem
                .split_once('/')
                .map_or(elem, |(link_type, _)| link_type);
            let shown = link::link_type(link_type).map_or(link_type, |def| def.outward);
            let other = link_target(restore).map_or_else(|| elem.to_owned(), name);
            let change = if present { "linked again" } else { "removed" };
            format!("{issue} {shown} {other}: {change}")
        }
        (field, Some(elem)) if field == COMMENTS_FIELD => {
            let change = if present { "retracted" } else { "shown again" };
            format!("{issue} comment {elem}: {change}")
        }
        (field, Some(elem)) => {
            let change = if present { "added" } else { "removed" };
            format!("{issue} {field} {}: {change}", printable(elem))
        }
        (field, None) => format!(
            "{issue} {field}: {} -> {}",
            printable(&restore.from.to_string()),
            printable(&restore.to.to_string())
        ),
    }
}

/// The batch a full id, or a unique start or end of one of at least
/// [`MIN_PART`] hex digits, names; one that matches several is refused.
fn batch(here: &Here, reference: &str) -> Result<Uuid, CliError> {
    if let Ok(id) = Uuid::parse_str(reference) {
        return Ok(id);
    }
    let digits: String = reference.chars().filter(|c| *c != '-').collect();
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(CliError::new(
            "invalid_value",
            format!("{reference:?} is not a batch id, which is hex digits"),
        )
        .with("field", "batch"));
    }
    if digits.len() < MIN_PART {
        return Err(CliError::new(
            "invalid_value",
            format!(
                "{reference:?} is too short to name a batch; give at least {MIN_PART} characters"
            ),
        )
        .with("field", "batch"));
    }
    match here.store.batches_matching(reference)?.as_slice() {
        [] => Err(CliError::new(
            "not_found",
            format!("no batch written on this Replica starts or ends with {reference}"),
        )
        .with("kind", "batch")
        .with("reference", reference)),
        [only] => Ok(*only),
        several => Err(CliError::new(
            "invalid_value",
            format!(
                "{reference} matches more than one batch; give more of it, or its last characters"
            ),
        )
        .with("field", "batch")
        .with(
            "matches",
            several.iter().map(Uuid::to_string).collect::<Vec<_>>(),
        )),
    }
}
