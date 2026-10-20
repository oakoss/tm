//! `tm create`: a new Issue in the Project the working directory maps to.

use serde_json::{Value, json};
use tm_core::Change;
use tm_core::scheme::{FieldType, Scheme};
use tm_store::{Draft, StoreError};
use uuid::Uuid;

use crate::error::CliError;
use crate::here::{self, Here};
use crate::identity;
use crate::output::Output;
use crate::show;

pub(crate) struct Create {
    pub(crate) issue_type: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) set: Vec<(String, String)>,
    pub(crate) labels: Vec<String>,
    pub(crate) parent: Option<String>,
}

/// `--set FIELD=VALUE`. A field with its own flag is refused, so `--type`
/// alone decides the Status.
pub(crate) fn field_value(pair: &str) -> Result<(String, String), String> {
    let (field, value) = split_pair(pair)?;
    match field.as_str() {
        "type" | "title" | "parent" | "labels" => {
            let flag = if field == "labels" { "label" } else { &field };
            Err(format!("give {field} with --{flag}, not --set"))
        }
        _ => Ok((field, value)),
    }
}

/// `FIELD=VALUE`, split at the first `=`.
pub(crate) fn split_pair(pair: &str) -> Result<(String, String), String> {
    pair.split_once('=')
        .map(|(field, value)| (field.to_owned(), value.to_owned()))
        .ok_or_else(|| format!("expected FIELD=VALUE, such as priority=high, not {pair:?}"))
}

pub(crate) fn run(create: &Create) -> Result<Output, CliError> {
    here::write(|here| write(here, create))
}

fn write(here: &mut Here, create: &Create) -> Result<(Output, Vec<String>), CliError> {
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    let project = here.project()?;
    let parent = match &create.parent {
        Some(reference) => Some(here.issue(reference)?),
        None => None,
    };
    let scheme = here.store.scheme().clone();
    let issue = Uuid::now_v7();
    let changes = changes(&scheme, create, parent);
    let drafts = changes
        .into_iter()
        .map(|change| Draft {
            actor: identity.actor.clone(),
            project,
            subject: issue,
            change,
        })
        .collect();
    let ops = match here.store.append_all(drafts) {
        Ok(ops) => ops,
        Err(StoreError::MissingFields { fields, .. }) => {
            return Err(missing(&scheme, create, fields));
        }
        Err(err) => return Err(err.into()),
    };
    let shown = show::render(here, issue)?;
    let output = here.undoable(shown, ops[0].op_id)?;
    Ok((output, identity.warnings))
}

/// The create, then each field given; the Status comes from the type's
/// Workflow, so it is set only when the type is known.
fn changes(scheme: &Scheme, create: &Create, parent: Option<Uuid>) -> Vec<Change> {
    let set = |field: &str, value: Value| Change::SetField {
        field: field.to_owned(),
        label: String::new(),
        value,
        value_label: None,
    };
    let mut changes = vec![Change::Create];
    if let Some(issue_type) = &create.issue_type {
        changes.push(set("type", json!(issue_type)));
        if let Some(workflow) = scheme.workflow(issue_type) {
            changes.push(set("status", json!(workflow.start)));
        }
    }
    if let Some(title) = &create.title {
        changes.push(set("title", json!(title)));
    }
    if let Some(parent) = parent {
        changes.push(set("parent", json!(parent.to_string())));
    }
    for (field, value) in &create.set {
        changes.push(set(field, typed(scheme, field, value)));
    }
    for label in &create.labels {
        changes.push(Change::Add {
            field: "labels".into(),
            label: String::new(),
            elem: label.clone(),
            elem_label: None,
        });
    }
    changes
}

/// A `--set` value as the field's type stores it: a number or boolean where
/// the Scheme says so, else the text. Text that does not parse is passed on
/// for the store to refuse with the field's rules.
pub(crate) fn typed(scheme: &Scheme, field: &str, value: &str) -> Value {
    match scheme.field(field).map(|field| &field.field_type) {
        Some(FieldType::Number) => value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or_else(|| json!(value), Value::Number),
        Some(FieldType::Boolean) => value
            .parse::<bool>()
            .map_or_else(|_| json!(value), Value::Bool),
        _ => json!(value),
    }
}

/// `missing_fields` with each field's allowed values. Without a type the
/// Status cannot be given, since it follows from the type, so it is not named.
fn missing(scheme: &Scheme, create: &Create, fields: Vec<String>) -> CliError {
    let no_type = fields.iter().any(|name| name == "type");
    let fields: Vec<String> = fields
        .into_iter()
        .filter(|name| !(no_type && name == "status"))
        .collect();
    let described = described(scheme, &fields);
    let mut fix = vec!["tm".to_owned(), "create".to_owned()];
    // One word per flag, so a value starting with `-` is not read as a flag.
    let mut flag = |name: &str, value: &str| fix.push(format!("{name}={}", quoted(value)));
    if let Some(issue_type) = &create.issue_type {
        flag("--type", issue_type);
    }
    if let Some(title) = &create.title {
        flag("--title", title);
    }
    for (field, value) in &create.set {
        flag("--set", &format!("{field}={value}"));
    }
    for label in &create.labels {
        flag("--label", label);
    }
    if let Some(parent) = &create.parent {
        flag("--parent", parent);
    }
    for name in &fields {
        match name.as_str() {
            "type" => fix.push("--type <type>".into()),
            "title" => fix.push("--title <title>".into()),
            other => fix.push(format!("--set {other}=<value>")),
        }
    }
    CliError::new(
        "missing_fields",
        format!("a new Issue needs {}", fields.join(", ")),
    )
    .fix(fix.join(" "))
    .with("fields", described)
}

/// Each field as `missing_fields` names it, with the values the Scheme allows
/// where it lists them.
fn described(scheme: &Scheme, fields: &[String]) -> Vec<Value> {
    fields
        .iter()
        .map(|name| {
            let allowed: Vec<&str> = match scheme.field(name).map(|field| &field.field_type) {
                Some(FieldType::IssueType) => scheme
                    .types
                    .iter()
                    .map(|issue_type| issue_type.id.as_str())
                    .collect(),
                Some(FieldType::Priority) => scheme
                    .priorities
                    .iter()
                    .map(|level| level.id.as_str())
                    .collect(),
                Some(FieldType::SingleSelect | FieldType::MultiSelect) => scheme
                    .field(name)
                    .map(|field| {
                        field
                            .options
                            .iter()
                            .map(|choice| choice.id.as_str())
                            .collect()
                    })
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            if allowed.is_empty() {
                json!({ "name": name })
            } else {
                json!({ "name": name, "allowed": allowed })
            }
        })
        .collect()
}

/// `value` as one POSIX shell word.
fn quoted(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.,:/=@+%".contains(&b));
    if plain {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use tm_core::scheme::Field;

    use super::*;

    #[test]
    fn a_set_value_takes_the_type_its_field_stores() {
        let mut scheme = Scheme::built_in();
        scheme.fields.push(Field {
            name: "points".into(),
            label: "Points".into(),
            field_type: FieldType::Number,
            builtin: false,
            derived: false,
            required: false,
            options: Vec::new(),
        });
        assert_eq!(typed(&scheme, "points", "3"), json!(3.0));
        assert_eq!(typed(&scheme, "points", "three"), json!("three"));
        assert_eq!(typed(&scheme, "archived", "true"), json!(true));
        assert_eq!(typed(&scheme, "title", "true"), json!("true"));
        assert_eq!(typed(&scheme, "nope", "1"), json!("1"));
    }
}
