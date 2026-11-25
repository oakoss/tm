//! `tm create`: a new Issue in the Project the working directory maps to.

use tm_core::scheme::Scheme;
use tm_store::{NewIssue, StoreError};
use uuid::Uuid;

use crate::error::CliError;
use crate::fields::{self, quoted, split_pair, typed};
use crate::follow;
use crate::here::{self, Here};
use crate::mention;
use crate::output::Output;
use crate::show;

pub(crate) struct Create {
    pub(crate) issue_type: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) set: Vec<(String, String)>,
    pub(crate) labels: Vec<String>,
    pub(crate) add: Vec<(String, String)>,
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
        _ => crate::fields::not_owned(&field).map(|()| (field, value)),
    }
}

pub(crate) fn run(create: Create) -> Result<Output, CliError> {
    here::write(|here| write(here, create))
}

fn write(here: &mut Here, mut create: Create) -> Result<(Output, Vec<String>), CliError> {
    let identity = here.writer()?;
    let project = here.project()?;
    fields::given_once(create.set.iter().map(|(field, _)| field))?;
    let parent = match &create.parent {
        Some(reference) => Some(here.issue(reference)?),
        None => None,
    };
    let scheme = here.store.scheme().clone();
    fields::known(&scheme, create.set.iter().map(|(field, _)| field))?;
    fields::labels(&create.labels)?;
    fields::several(&scheme, &create.add, &[], |_, _| false)?;
    if let Some(issue_type) = &create.issue_type {
        if scheme.issue_type(issue_type).is_none() {
            return Err(StoreError::Invalid {
                field: "type".into(),
                reason: "is not a type".into(),
                valid: scheme.type_ids(),
            }
            .into());
        }
        if let Some(parent) = parent {
            here.store.check_new_parent_type(issue_type, parent)?;
        }
    }
    // Read once all else is checked, so a mistake keeps the piped text.
    // Stdin is spent once read, so from here on `set` holds what it gave.
    create.set = fields::read_resolved(here, &create.set, "leave the field out")?;
    for (field, value) in &mut create.set {
        if field == "description" {
            *value = mention::resolve(here, value)?;
        }
    }
    let create = &create;
    let new = new_issue(&scheme, create, parent);
    let mut warnings = identity.warnings.clone();
    warnings.extend(fields::entity_warnings(&new.changes(&scheme)));
    let (ops, created) = match here.store.create(&identity.actor, project, &new) {
        Ok((written, created)) => (here.passing(written), created),
        Err(StoreError::MissingFields { fields, target, .. }) => {
            return Err(missing(&scheme, create, fields, target));
        }
        Err(err) => return Err(err.into()),
    };
    let issue = created.issue;
    let shown = show::render(here, issue)?;
    let shown = follow::report(&here.store, shown, created.starts, issue)?;
    let output = here.undoable(shown, ops[0].op_id, &identity.actor)?;
    Ok((output, warnings))
}

fn new_issue(scheme: &Scheme, create: &Create, parent: Option<Uuid>) -> NewIssue {
    NewIssue {
        issue_type: create.issue_type.clone(),
        title: create.title.clone(),
        parent,
        fields: create
            .set
            .iter()
            .map(|(field, value)| (field.clone(), typed(scheme, field, value)))
            .collect(),
        labels: create.labels.clone(),
        elements: create.add.clone(),
    }
}

/// `missing_fields` with each field's allowed values, and the starting
/// Status when it requires them. Without a type the Status cannot be given,
/// since it follows from the type, so it is not named.
fn missing(
    scheme: &Scheme,
    create: &Create,
    fields: Vec<String>,
    target: Option<String>,
) -> CliError {
    let no_type = fields.iter().any(|name| name == "type");
    let fields: Vec<String> = fields
        .into_iter()
        .filter(|name| !(no_type && name == "status"))
        .collect();
    let described = fields::described(scheme, &fields);
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
    for (field, value) in &create.add {
        flag("--add", &format!("{field}={value}"));
    }
    if let Some(parent) = &create.parent {
        flag("--parent", parent);
    }
    fix.extend(fields.iter().map(|name| fields::placeholder(scheme, name)));
    let err = CliError::new(
        "missing_fields",
        format!("a new Issue needs {}", fields.join(", ")),
    )
    .fix(fix.join(" "))
    .with("fields", described);
    match target {
        Some(target) => err.with("target", target),
        None => err,
    }
}
