//! `tm edit`: an Issue's fields, labels and parent, changed in one batch.

use std::collections::btree_map::Entry;
use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::Change;
use tm_core::scheme::Scheme;
use tm_store::{Draft, IssueView, Named, StoreError};
use uuid::Uuid;

use crate::error::CliError;
use crate::fields::{self, quoted, set, split_pair, typed};
use crate::here::{self, Here};
use crate::identity;
use crate::output::{Output, printable};

#[derive(clap::Args)]
#[command(group(
    clap::ArgGroup::new("changes")
        .required(true)
        .multiple(true)
        .args(["set", "unset", "labels", "remove_labels", "parent", "no_parent"])
))]
pub(crate) struct Edit {
    /// The Issue's Key or full id.
    issue: String,
    /// Set a field, such as priority=high; FIELD=- reads the value from
    /// stdin. Repeat for more.
    #[arg(long, value_name = "FIELD=VALUE", value_parser = field_value)]
    set: Vec<(String, String)>,
    /// Clear a field; repeat for more.
    #[arg(long, value_name = "FIELD", value_parser = unset_field)]
    unset: Vec<String>,
    /// Add a label; repeat for more.
    #[arg(long = "label", value_name = "LABEL")]
    labels: Vec<String>,
    /// Remove a label; repeat for more.
    #[arg(long = "remove-label", value_name = "LABEL")]
    remove_labels: Vec<String>,
    /// The new parent Issue's Key or id.
    #[arg(long, conflicts_with = "no_parent")]
    parent: Option<String>,
    /// Remove the parent.
    #[arg(long)]
    no_parent: bool,
    /// Write a field another Agent session's live Claim protects; only once
    /// that session has stopped work on it.
    #[arg(long)]
    force: bool,
    /// Refuse the write if a field it sets changed after this version, the
    /// one tm show printed when you read the Issue.
    #[arg(long, value_name = "VERSION")]
    if_version: Option<u64>,
}

/// `--set FIELD=VALUE`. Labels and parent have their own flags; a Status is
/// refused when the command runs, with the move that changes it.
pub(crate) fn field_value(pair: &str) -> Result<(String, String), String> {
    let (field, value) = split_pair(pair)?;
    own_flag(&field).map(|()| (field, value))
}

/// `--unset FIELD`.
pub(crate) fn unset_field(field: &str) -> Result<String, String> {
    own_flag(field).map(|()| field.to_owned())
}

pub(crate) fn own_flag(field: &str) -> Result<(), String> {
    match field {
        "labels" => Err("change labels with --label and --remove-label".into()),
        "parent" => Err("change the parent with --parent or --no-parent".into()),
        _ => fields::not_owned(field),
    }
}

/// A field's value before and after the edit, as `--json` reports it.
pub(crate) struct Changed {
    pub(crate) field: String,
    pub(crate) before: Value,
    pub(crate) after: Value,
}

pub(crate) fn run(edit: &Edit) -> Result<Output, CliError> {
    here::write(|here| write(here, &edit.issue, edit))
}

fn write(here: &mut Here, reference: &str, edit: &Edit) -> Result<(Output, Vec<String>), CliError> {
    fields::given_once(edit.set.iter().map(|(field, _)| field).chain(&edit.unset))?;
    refuse_status(reference, edit)?;
    let id = here.issue(reference)?;
    let (view, read) = here.store.issue_with_version(id)?.ok_or_else(|| {
        CliError::new("not_found", format!("no Issue is {reference}"))
            .with("kind", "issue")
            .with("reference", reference)
    })?;
    let based_on = crate::moves::based_on(edit.if_version, read);
    here.store.base_writes_on(id, based_on);
    let parent = match &edit.parent {
        Some(parent) => Some(here.issue(parent)?),
        None => None,
    };
    // Only a parent that changes is written, so only it is checked: a cycle a
    // merge left stays editable.
    let current = view.fields.get("parent").and_then(Value::as_str);
    let reparents = match parent {
        Some(parent) => current != Some(parent.to_string().as_str()),
        None => edit.no_parent && current.is_some(),
    };
    if let Some(parent) = parent.filter(|_| reparents) {
        here.store.check_parent(id, parent)?;
    }
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    check_flags(here.store.scheme(), &view, edit)?;
    if edit.force {
        here.store.force_past_claims();
    }
    let forced = |err| CliError::past_claim(err, "tm edit --force");
    // A refusal would lose piped text, so check before reading it; the write
    // checks again.
    if !fields::piped(&edit.set).is_empty() {
        let mut written = fields::written(here.store.scheme(), &view, &edit.set, &edit.unset);
        if reparents {
            written.push("parent");
        }
        here.store
            .check_protected(&identity.actor, id, &written)
            .map_err(forced)?;
        here.store.check_version(id, based_on, &written)?;
    }
    // Read once all else is checked, so a mistake keeps the piped text.
    let values = fields::read_stdin(&edit.set, "clear a field with --unset")?;
    let (changes, report) = changes(here, &view, edit, &values, parent)?;
    let name = view.key.clone().unwrap_or_else(|| view.id.to_string());
    let data = |report: &[Changed]| {
        json!({
            "id": view.id,
            "key": view.key,
            "changed": report
                .iter()
                .map(|c| json!({ "field": c.field, "before": c.before, "after": c.after }))
                .collect::<Vec<_>>(),
        })
    };
    if changes.is_empty() {
        // Writing nothing loses nothing, but a version not read from the
        // Issue is still a mistake to report.
        here.store.check_version(id, based_on, &[])?;
        let mut output = Output::new(format!("{name}: nothing changed\n"), data(&[]));
        output.data["batch"] = Value::Null;
        return Ok((output, Vec::new()));
    }

    let drafts = changes
        .into_iter()
        .map(|change| Draft {
            actor: identity.actor.clone(),
            project: view.project,
            subject: id,
            change,
        })
        .collect();
    let ops = here.store.append_all(drafts).map_err(forced)?;
    let mut text = String::new();
    for change in &report {
        let _ = writeln!(text, "{name}  {}", summary(change));
    }
    let output = here.undoable(
        Output::new(text, data(&report)),
        ops[0].op_id,
        &identity.actor,
    )?;
    Ok((output, identity.warnings))
}

/// The Status changes only by `tm move`, which also releases a Claim when
/// work finishes (ADR 0009).
fn refuse_status(reference: &str, edit: &Edit) -> Result<(), CliError> {
    let target = edit
        .set
        .iter()
        .find(|(field, _)| field == "status")
        .map(|(_, value)| value.as_str())
        .or_else(|| {
            edit.unset
                .iter()
                .any(|field| field == "status")
                .then_some("STATUS")
        });
    match target {
        Some(target) => Err(CliError::new(
            "use_move",
            "the Status changes with tm move, which also releases a Claim when the work is done",
        )
        .fix(format!("tm move {} {}", quoted(reference), quoted(target)))),
        None => Ok(()),
    }
}

/// Refuses what the flags ask for that no value could make right: a field
/// the Scheme lacks or computes, one that cannot be cleared, a blank label
/// to add, or a label both added and removed.
fn check_flags(scheme: &Scheme, view: &IssueView, edit: &Edit) -> Result<(), CliError> {
    fields::known(scheme, edit.set.iter().map(|(field, _)| field))?;
    fields::labels(&edit.labels)?;
    for name in &edit.unset {
        unsettable(scheme, view, name)?;
    }
    if let Some(both) = edit.labels.iter().find(|label| {
        edit.remove_labels
            .iter()
            .any(|removed| key(removed) == key(label))
    }) {
        return Err(CliError::new(
            "invalid_value",
            format!("{} is both added and removed", printable(both.trim())),
        )
        .with("field", "labels"));
    }
    Ok(())
}

/// A label keyed as the store keys labels: trimmed and lowercased.
fn key(label: &str) -> String {
    label.trim().to_lowercase()
}

/// The changes the flags ask for, leaving out any that would write a value
/// the Issue already has, and what each changes.
fn changes(
    here: &Here,
    view: &IssueView,
    edit: &Edit,
    values: &[(String, String)],
    parent: Option<Uuid>,
) -> Result<(Vec<Change>, Vec<Changed>), CliError> {
    let scheme = here.store.scheme();
    let mut changes = Vec::new();
    let mut report = Vec::new();
    let mut field = |name: &str, after: Value, before: Option<&Value>| {
        let before = before.cloned().unwrap_or(Value::Null);
        if !same(&before, &after) {
            changes.push(set(name, after.clone()));
            report.push(Changed {
                field: name.to_owned(),
                before,
                after,
            });
        }
    };
    for (name, value) in values {
        field(name, typed(scheme, name, value), view.fields.get(name));
    }
    for name in &edit.unset {
        field(name, Value::Null, view.fields.get(name));
    }

    let before = view.sets.get("labels").cloned().unwrap_or_default();
    let mut after = before.clone();
    for label in &edit.labels {
        if let Entry::Vacant(entry) = after.entry(key(label)) {
            entry.insert(label.trim().to_owned());
            changes.push(fields::label(label, true));
        }
    }
    for label in &edit.remove_labels {
        if after.remove(&key(label)).is_some() {
            changes.push(fields::label(label, false));
        }
    }
    if after != before {
        report.push(Changed {
            field: "labels".into(),
            before: json!(before.values().collect::<Vec<_>>()),
            after: json!(after.values().collect::<Vec<_>>()),
        });
    }

    let current = view.fields.get("parent").and_then(Value::as_str);
    let wanted = match (parent, edit.no_parent) {
        (Some(parent), _) => Some(Some(parent.to_string())),
        (None, true) => Some(None),
        (None, false) => None,
    };
    if let Some(wanted) = wanted
        && wanted.as_deref() != current
    {
        let key = |id: Option<&str>| -> Result<Value, CliError> {
            let Some(id) = id else {
                return Ok(Value::Null);
            };
            let key = match Uuid::parse_str(id) {
                Ok(uuid) => here.store.issue(uuid)?.and_then(|view| view.key),
                Err(_) => None,
            };
            Ok(json!({ "id": id, "key": key }))
        };
        changes.push(set(
            "parent",
            wanted.as_deref().map_or(Value::Null, |id| json!(id)),
        ));
        report.push(Changed {
            field: "parent".into(),
            before: key(current)?,
            after: key(wanted.as_deref())?,
        });
    }
    Ok((changes, report))
}

/// Whether writing `after` over `before` would change nothing. A whole number
/// reads back as an integer, and `typed` gives a float.
pub(crate) fn same(before: &Value, after: &Value) -> bool {
    before == after || before.as_f64().is_some_and(|b| after.as_f64() == Some(b))
}

fn held(value: &Value) -> bool {
    !value.is_null() && value.as_str().is_none_or(|text| !text.trim().is_empty())
}

/// Whether `--unset` can clear `name`. Priority and archived always hold a
/// value, so clearing them would write their default; a field the Scheme
/// lacks clears only while the Issue still holds it.
fn unsettable(scheme: &Scheme, view: &IssueView, name: &str) -> Result<(), CliError> {
    if matches!(name, "priority" | "archived") {
        return Err(CliError::new(
            "invalid_value",
            format!("{name} always holds a value; set it instead, such as with --set {name}=..."),
        )
        .with("field", name));
    }
    match scheme.field(name) {
        Some(field) if field.derived => return Err(fields::computed(name)),
        // The store refuses only a required field the write would empty, and
        // counts blank text as empty.
        Some(field) if field.required && view.fields.get(name).is_some_and(held) => {
            return Err(StoreError::MissingFields {
                issue: Named {
                    id: view.id,
                    key: view.key.clone(),
                    source: None,
                },
                fields: vec![name.to_owned()],
            }
            .into());
        }
        _ => {}
    }
    if scheme.field(name).is_none() && !view.fields.contains_key(name) {
        return Err(StoreError::UnknownField {
            field: name.to_owned(),
            valid: scheme
                .fields
                .iter()
                .filter(|field| !field.derived)
                .map(|field| field.name.clone())
                .collect(),
        }
        .into());
    }
    Ok(())
}

/// A value as a change line shows it; a parent by Key.
pub(crate) fn shown(value: &Value) -> String {
    match value {
        Value::Null => "(none)".to_owned(),
        Value::Array(items) if items.is_empty() => "(none)".to_owned(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map_or_else(|| item.to_string(), str::to_owned)
            })
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(parent) => parent["key"]
            .as_str()
            .or_else(|| parent["id"].as_str())
            .unwrap_or_default()
            .to_owned(),
        other => other.to_string(),
    }
}

/// Whether a shown value is too long for one change line.
pub(crate) fn long(text: &str) -> bool {
    text.contains('\n') || text.chars().count() > 60
}

/// One line for a change: old to new, or for text of several lines or more
/// than a line's length, how many lines it went from and to.
pub(crate) fn summary(change: &Changed) -> String {
    let (before, after) = (shown(&change.before), shown(&change.after));
    if long(&before) || long(&after) {
        let lines = |value: &Value, text: &str| {
            if value.is_null() {
                0
            } else {
                text.lines().count()
            }
        };
        printable(&format!(
            "{}: changed ({} → {} lines)",
            change.field,
            lines(&change.before, &before),
            lines(&change.after, &after)
        ))
    } else {
        printable(&format!("{}: {before} → {after}", change.field))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_is_the_same_whether_read_as_an_integer_or_a_float() {
        assert!(same(&json!(3), &json!(3.0)));
        assert!(!same(&json!(3), &json!(3.5)));
        assert!(!same(&json!("3"), &json!(3.0)));
        assert!(same(&Value::Null, &Value::Null));
    }
}
