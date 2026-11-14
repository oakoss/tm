//! `tm undo`: one command's writes reverted, named by the batch it printed.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::op::{COMMENT_BODY_FIELD, COMMENTS_FIELD};
use tm_core::{Actor, link};
use tm_store::{Restore, Target, WriteOptions};
use uuid::Uuid;

use crate::error::CliError;
use crate::here::{self, Here};
use crate::output::{Output, id_by_part, printable, short, undo_hint};
use crate::show;

pub(crate) fn run(reference: Option<&str>, dry_run: bool, force: bool) -> Result<Output, CliError> {
    here::write(|here| {
        undo(here, reference, dry_run, force).map(|(undone, warnings)| (undone.output, warnings))
    })
}

/// What `tm undo` printed, and each line it printed after the batch it names.
pub(crate) struct Undone {
    pub(crate) output: Output,
    pub(crate) lines: Vec<String>,
}

pub(crate) fn undo(
    here: &mut Here,
    reference: Option<&str>,
    dry_run: bool,
    force: bool,
) -> Result<(Undone, Vec<String>), CliError> {
    let identity = here.writer()?;
    let (batch, next) = match reference {
        Some(reference) => (batch(here, reference)?, None),
        None => last(here, &identity.actor)?,
    };
    let options = WriteOptions {
        force,
        ..WriteOptions::default()
    };
    let undone = here
        .store
        .undo_with(&identity.actor, batch, &options, !dry_run)
        .map(|(plan, ops)| (plan, ops.first().filter(|_| !dry_run).map(|op| op.op_id)));
    let (plan, written) = undone.map_err(|err| {
        let err = CliError::past_claim(err, "tm undo --force");
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
    let lists = list_names(here, &plan.restores)?;
    let visible: Vec<&Restore> = plan
        .restores
        .iter()
        .filter(|restore| shown(restore, &plan.restores))
        .collect();
    let restored: Vec<Value> = visible.iter().map(|r| restore_json(r, &keys)).collect();

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
    let reverted: Vec<String> = visible
        .iter()
        .map(|restore| describe(restore, &name, &lists))
        .chain(plan.released.iter().map(|issue| {
            format!(
                "{}'s Claim stays released; its Agent takes it again with tm claim",
                name(*issue)
            )
        }))
        .collect();
    for line in &reverted {
        let _ = writeln!(text, "  {line}");
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
    let mut lines = reverted;
    let output = match written {
        Some(written) => {
            if here.store.can_undo(written)? {
                lines.push(undo_hint(written));
            }
            here.undoable(output, written, &identity.actor)?
        }
        None => output,
    };
    Ok((Undone { output, lines }, identity.warnings))
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
        .fix("tm undo '<batch>'"));
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
        Target::Issue { issue } | Target::Notification { issue, .. } => Some(issue),
        Target::Shortlist { .. } | Target::Memory { .. } => None,
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

const SHORTLIST: &str = "shortlist";
const RANK: &str = "shortlist_rank";

/// The name of each Short-list a restore puts an Issue on or takes it off.
fn list_names(here: &Here, restores: &[Restore]) -> Result<HashMap<Uuid, String>, CliError> {
    let mut lists = HashMap::new();
    for restore in restores.iter().filter(|r| r.field == SHORTLIST) {
        for id in [&restore.from, &restore.to].into_iter().filter_map(list_id) {
            let name = here.store.shortlist(id)?.and_then(|list| list.name);
            let name = name.map_or_else(|| id.to_string(), |name| printable(&name));
            lists.insert(id, name);
        }
    }
    Ok(lists)
}

/// A restore with its Issue's Key; a rank's values are left out.
fn restore_json(restore: &Restore, keys: &HashMap<Uuid, Value>) -> Value {
    let mut entry = json!(restore);
    if restore.field == RANK {
        entry["from"] = Value::Null;
        entry["to"] = Value::Null;
    }
    if let Some(issue) = issue_of(restore) {
        entry["key"] = keys.get(&issue).cloned().unwrap_or(Value::Null);
    }
    entry
}

fn list_id(value: &Value) -> Option<Uuid> {
    value.as_str().and_then(|id| Uuid::parse_str(id).ok())
}

/// Whether a restore is shown: a rank restored with its Issue's list is
/// the list line's to tell, and a rank is never shown (ADR 0016).
fn shown(restore: &Restore, all: &[Restore]) -> bool {
    restore.field != RANK
        || !all
            .iter()
            .any(|other| other.field == SHORTLIST && other.target == restore.target)
}

/// One restore as a person reads it: Keys for Issues, lists by name, and a
/// set element added or removed rather than true or false.
fn describe(
    restore: &Restore,
    name: &impl Fn(Uuid) -> String,
    lists: &HashMap<Uuid, String>,
) -> String {
    let issue = match &restore.target {
        Target::Issue { issue } => name(*issue),
        Target::Shortlist {
            name: Some(list), ..
        } => format!("Short-list {}", printable(list)),
        Target::Shortlist { shortlist, .. } => format!("Short-list {shortlist}"),
        Target::Memory { key: Some(key), .. } => format!("memory {}", printable(key)),
        Target::Memory { memory, .. } => format!("memory {memory}"),
        Target::Notification {
            notification,
            issue,
        } => {
            let read = if restore.to == json!(true) {
                "read"
            } else {
                "unread"
            };
            return format!("{} Notification {notification}: {read}", name(*issue));
        }
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
        (field, Some(elem)) if field == COMMENT_BODY_FIELD => {
            format!("{issue} comment {elem}: text restored")
        }
        (field, Some(elem)) => {
            let change = if present { "added" } else { "removed" };
            format!("{issue} {field} {}: {change}", printable(elem))
        }
        (SHORTLIST, None) => {
            let list = |value: &Value| {
                list_id(value).map_or_else(
                    || "(none)".to_owned(),
                    |id| lists.get(&id).cloned().unwrap_or_else(|| id.to_string()),
                )
            };
            let (from, to) = (list(&restore.from), list(&restore.to));
            format!("{issue} Short-list: {from} -> {to}")
        }
        (RANK, None) => format!("{issue} Short-list order: restored"),
        (field, None) => format!(
            "{issue} {field}: {} -> {}",
            printable(&restore.from.to_string()),
            printable(&restore.to.to_string())
        ),
    }
}

/// The batch a full id, or a unique start or end of one, names.
fn batch(here: &Here, reference: &str) -> Result<Uuid, CliError> {
    if let Ok(id) = Uuid::parse_str(reference) {
        return Ok(id);
    }
    let within = ("batch", "written on this Replica");
    id_by_part(reference, within, |digits| {
        Ok(here.store.batches_matching(digits)?)
    })
}
