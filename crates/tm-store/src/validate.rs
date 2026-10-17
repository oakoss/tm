//! Write-time checks against the Scheme. They run only when this Replica
//! writes; replay never consults the Scheme, so a Scheme edit or a merge never
//! makes a stored operation unreadable.

use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::Value;
use tm_core::Change;
use tm_core::scheme::{Field, FieldType, Scheme, priority_rank};
use uuid::Uuid;

use crate::StoreError;

/// Checks `change` against the Scheme and the Issue's current state, and fills
/// in the display labels the operation records.
pub(crate) fn check(
    tx: &Transaction<'_>,
    scheme: &Scheme,
    issue_id: Uuid,
    change: Change,
) -> Result<Change, StoreError> {
    if !matches!(change, Change::Create) && !exists(tx, issue_id)? {
        return Err(StoreError::UnknownIssue(issue_id));
    }
    match change {
        Change::Create | Change::Comment { .. } => Ok(change),
        Change::SetField { field, value, .. } => {
            // A stored value can be cleared even when the Scheme lacks its field.
            if value.is_null() && scheme.field(&field).is_none() && has_value(tx, issue_id, &field)?
            {
                return Ok(Change::SetField {
                    label: field.clone(),
                    field,
                    value,
                    value_label: None,
                });
            }
            let def = settable(scheme, &field)?;
            if def.field_type.is_multi() {
                return Err(invalid(def, "holds several values; add or remove them", []));
            }
            let value_label = check_value(tx, scheme, issue_id, def, &value)?;
            Ok(Change::SetField {
                label: def.label.clone(),
                field,
                value,
                value_label,
            })
        }
        Change::Add { field, elem, .. } => {
            let (def, elem, elem_label) = member(scheme, &field, &elem)?;
            Ok(Change::Add {
                field,
                label: def.label.clone(),
                elem,
                elem_label,
            })
        }
        Change::Remove { field, elem, .. } => {
            // A held element can be removed even when the Scheme does not allow it.
            let def = scheme.field(&field);
            let stored = match def.map(|def| def.field_type) {
                Some(FieldType::Labels) => elem.trim().to_lowercase(),
                _ => elem.clone(),
            };
            if let Some(present) = stored_member(tx, issue_id, &field, &stored)? {
                return Ok(Change::Remove {
                    label: def.map_or_else(|| field.clone(), |def| def.label.clone()),
                    field,
                    elem: stored,
                    elem_label: present.display,
                });
            }
            let (def, elem, elem_label) = member(scheme, &field, &elem)?;
            Ok(Change::Remove {
                field,
                label: def.label.clone(),
                elem,
                elem_label,
            })
        }
    }
}

/// A set element an Issue holds, with the display form it was added with.
struct Present {
    display: Option<String>,
}

fn stored_member(
    tx: &Transaction<'_>,
    issue_id: Uuid,
    field: &str,
    elem: &str,
) -> Result<Option<Present>, StoreError> {
    Ok(tx
        .query_row(
            "SELECT display FROM set_members WHERE issue_id = ?1 AND field = ?2 AND elem = ?3",
            params![issue_id.as_bytes(), field, elem],
            |row| {
                Ok(Present {
                    display: row.get(0)?,
                })
            },
        )
        .optional()?)
}

fn has_value(tx: &Transaction<'_>, issue_id: Uuid, field: &str) -> Result<bool, StoreError> {
    Ok(tx
        .query_row(
            "SELECT 1 FROM field_values WHERE issue_id = ?1 AND field = ?2",
            params![issue_id.as_bytes(), field],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn has_member(tx: &Transaction<'_>, issue_id: Uuid, field: &str) -> Result<bool, StoreError> {
    Ok(tx
        .query_row(
            "SELECT 1 FROM set_members WHERE issue_id = ?1 AND field = ?2",
            params![issue_id.as_bytes(), field],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Whether the Issue's Status belongs to its type's Workflow; true while either
/// is unset or the Issue does not exist yet.
pub(crate) fn status_fits_type(
    tx: &Transaction<'_>,
    scheme: &Scheme,
    issue_id: Uuid,
) -> Result<bool, StoreError> {
    let (Some(issue_type), Some(status)) = (
        current(tx, issue_id, "type")?,
        current(tx, issue_id, "status")?,
    ) else {
        return Ok(true);
    };
    Ok(scheme.category(&issue_type, &status).is_some())
}

/// The required fields `issue_id` lacks; none for an Issue that does not exist yet.
pub(crate) fn missing_fields(
    tx: &Transaction<'_>,
    scheme: &Scheme,
    issue_id: Uuid,
) -> Result<Vec<String>, StoreError> {
    let columns = ["type", "title", "description", "status", "assignee"];
    let row: Option<Vec<Option<String>>> = tx
        .query_row(
            "SELECT type, title, description, status, assignee, parent_id IS NOT NULL
             FROM issues WHERE id = ?1",
            [issue_id.as_bytes()],
            |row| {
                let mut values: Vec<Option<String>> = (0..columns.len())
                    .map(|i| row.get(i))
                    .collect::<Result<_, _>>()?;
                let has_parent: bool = row.get(columns.len())?;
                values.push(has_parent.then(String::new));
                Ok(values)
            },
        )
        .optional()?;
    let Some(values) = row else {
        return Ok(Vec::new());
    };
    let mut missing = Vec::new();
    for field in scheme.fields.iter().filter(|field| field.required) {
        let name = field.name.as_str();
        let present = if let Some(i) = columns.iter().position(|column| *column == name) {
            values[i]
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
        } else if name == "parent" {
            values[columns.len()].is_some()
        } else if field.field_type.is_multi() {
            has_member(tx, issue_id, name)?
        } else if field.builtin {
            // Priority and archived always hold a value; computed fields are not set.
            true
        } else {
            has_value(tx, issue_id, name)?
        };
        if !present {
            missing.push(field.name.clone());
        }
    }
    Ok(missing)
}

fn settable<'a>(scheme: &'a Scheme, name: &str) -> Result<&'a Field, StoreError> {
    let def = scheme.field(name).ok_or_else(|| StoreError::UnknownField {
        field: name.to_owned(),
        valid: scheme
            .fields
            .iter()
            .filter(|field| !field.derived)
            .map(|field| field.name.clone())
            .collect(),
    })?;
    if def.derived {
        return Err(invalid(def, "is computed and cannot be set", []));
    }
    Ok(def)
}

/// Checks a set element and returns its stored form and display label. Labels
/// compare without case, so they are stored lowercased and keep the spelling
/// they were written with as their display form.
fn member<'a>(
    scheme: &'a Scheme,
    field: &str,
    elem: &str,
) -> Result<(&'a Field, String, Option<String>), StoreError> {
    let def = settable(scheme, field)?;
    match def.field_type {
        FieldType::Labels => {
            let written = elem.trim();
            if written.is_empty() {
                return Err(invalid(def, "cannot hold an empty label", []));
            }
            if written.chars().any(char::is_control) {
                return Err(invalid(def, "cannot hold control characters", []));
            }
            let stored = written.to_lowercase();
            let display = (stored != written).then(|| written.to_owned());
            Ok((def, stored, display))
        }
        FieldType::MultiSelect => {
            let choice = def
                .option(elem)
                .ok_or_else(|| invalid(def, "has no such option", option_ids(def)))?;
            Ok((def, elem.to_owned(), Some(choice.label.clone())))
        }
        _ => Err(invalid(def, "holds one value; set it", [])),
    }
}

fn check_value(
    tx: &Transaction<'_>,
    scheme: &Scheme,
    issue_id: Uuid,
    def: &Field,
    value: &Value,
) -> Result<Option<String>, StoreError> {
    let text = || value.as_str().filter(|text| !text.trim().is_empty());
    match (def.field_type, value) {
        (_, Value::Null)
        | (FieldType::Number, Value::Number(_))
        | (FieldType::Boolean, Value::Bool(_)) => Ok(None),
        (FieldType::IssueType, Value::String(id)) => {
            let issue_type = scheme.issue_type(id).ok_or_else(|| {
                invalid(
                    def,
                    "is not a type",
                    scheme.types.iter().map(|t| t.id.clone()),
                )
            })?;
            let workflow = scheme
                .workflow(id)
                .ok_or_else(|| invalid(def, "has no Workflow in the Scheme", []))?;
            // Refused only when a valid Status would become invalid, so an Issue
            // whose type and Status are both unknown can be repaired by its type.
            if let Some(status) = current(tx, issue_id, "status")? {
                let valid_now = current(tx, issue_id, "type")?
                    .and_then(|current_type| scheme.workflow(&current_type))
                    .is_some_and(|current| current.status(&status).is_some());
                if valid_now && workflow.status(&status).is_none() {
                    return Err(invalid(
                        def,
                        "would leave the current Status outside the type's Workflow",
                        [],
                    ));
                }
            }
            Ok(Some(issue_type.label.clone()))
        }
        (FieldType::Status, Value::String(id)) => {
            let issue_type = current(tx, issue_id, "type")?
                .ok_or_else(|| invalid(def, "needs the Issue's type to be set first", []))?;
            let workflow = scheme.workflow(&issue_type).ok_or_else(|| {
                invalid(
                    def,
                    "needs a type the Scheme defines; set the type first",
                    [],
                )
            })?;
            let from = current(tx, issue_id, "status")?;
            if !workflow.allows(from.as_deref(), id) {
                return Err(invalid(
                    def,
                    "is not allowed from the current Status",
                    workflow.statuses.iter().map(|status| status.id.clone()),
                ));
            }
            Ok(workflow.status(id).map(|status| status.label.clone()))
        }
        (FieldType::Priority, Value::String(id)) => scheme
            .priority(id)
            .filter(|_| priority_rank(id).is_some())
            .map(|level| Some(level.label.clone()))
            .ok_or_else(|| {
                invalid(
                    def,
                    "is not a priority level",
                    scheme.priorities.iter().map(|level| level.id.clone()),
                )
            }),
        (FieldType::SingleSelect, Value::String(id)) => def
            .option(id)
            .map(|choice| Some(choice.label.clone()))
            .ok_or_else(|| invalid(def, "has no such option", option_ids(def))),
        (FieldType::Text | FieldType::RichText | FieldType::Person, Value::String(_)) => text()
            .map(|_| None)
            .ok_or_else(|| invalid(def, "cannot be blank", [])),
        (FieldType::IssueReference, Value::String(id)) => {
            let target = Uuid::parse_str(id).map_err(|_| invalid(def, "is not an Issue id", []))?;
            if target == issue_id {
                return Err(invalid(def, "cannot refer to the Issue itself", []));
            }
            if exists(tx, target)? {
                Ok(None)
            } else {
                Err(StoreError::UnknownIssue(target))
            }
        }
        (FieldType::Date, Value::String(date)) => date
            .parse::<jiff::civil::Date>()
            .map(|_| None)
            .map_err(|_| invalid(def, "is not a date (YYYY-MM-DD)", [])),
        (FieldType::Datetime, Value::String(time)) => time
            .parse::<jiff::Timestamp>()
            .map(|_| None)
            .map_err(|_| invalid(def, "is not an RFC 3339 timestamp", [])),
        _ => Err(invalid(def, "has the wrong value type", [])),
    }
}

fn current(
    tx: &Transaction<'_>,
    issue_id: Uuid,
    column: &str,
) -> Result<Option<String>, StoreError> {
    Ok(tx
        .query_row(
            &format!("SELECT {column} FROM issues WHERE id = ?1"),
            [issue_id.as_bytes()],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

fn exists(tx: &Transaction<'_>, issue_id: Uuid) -> Result<bool, StoreError> {
    Ok(tx
        .query_row(
            "SELECT 1 FROM issues WHERE id = ?1",
            [issue_id.as_bytes()],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn option_ids(def: &Field) -> Vec<String> {
    def.options.iter().map(|choice| choice.id.clone()).collect()
}

fn invalid(def: &Field, reason: &str, valid: impl IntoIterator<Item = String>) -> StoreError {
    StoreError::Invalid {
        field: def.name.clone(),
        reason: reason.to_owned(),
        valid: valid.into_iter().collect(),
    }
}
