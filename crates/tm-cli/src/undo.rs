//! `tm undo`: one command's writes reverted, named by the batch it printed.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::link;
use tm_core::op::COMMENTS_FIELD;
use tm_store::Restore;
use uuid::Uuid;

use crate::error::CliError;
use crate::here::{self, Here};
use crate::identity;
use crate::output::{Output, printable};
use crate::show;

/// Fewer characters would too often match more than one batch.
const MIN_PART: usize = 8;

pub(crate) fn run(reference: &str, dry_run: bool) -> Result<Output, CliError> {
    let mut here = here::open()?;
    let warnings = std::mem::take(&mut here.warnings);
    match undo(&mut here, reference, dry_run) {
        Ok((mut output, identity_warnings)) => {
            output.warnings = warnings;
            output.warnings.extend(identity_warnings);
            Ok(output)
        }
        Err(err) => Err(err.warned(warnings)),
    }
}

fn undo(
    here: &mut Here,
    reference: &str,
    dry_run: bool,
) -> Result<(Output, Vec<String>), CliError> {
    let batch = batch(here, reference)?;
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    let (plan, written) = if dry_run {
        (here.store.undo_plan(&identity.actor, batch)?, None)
    } else {
        let (plan, ops) = here.store.undo(&identity.actor, batch)?;
        (plan, ops.first().map(|op| op.op_id))
    };

    // A Link is filed under one end, but both ends' `show` change.
    let touched: BTreeSet<Uuid> = plan
        .restores
        .iter()
        .flat_map(|restore| [Some(restore.issue), link_target(restore)])
        .flatten()
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
            entry["key"] = keys.get(&restore.issue).cloned().unwrap_or(Value::Null);
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
    if let Some(written) = written {
        let _ = writeln!(text, "undo this with: tm undo {written}");
    }
    let data = json!({
        "undone": batch,
        "dry_run": dry_run,
        "restored": restored,
        "issues": issues,
        "batch": written,
    });
    Ok((Output::new(text, data), identity.warnings))
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
    let issue = name(restore.issue);
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
