//! Reads Issues back out of the projection, applying the Scheme on the way.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{Value, json};
use tm_core::scheme::{Category, FieldType, PRIORITY_RANKS, Scheme};
use uuid::Uuid;

use crate::StoreError;
use crate::apply::uuid_from;

/// An Issue's current values and the Status category the Scheme gives it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IssueView {
    pub id: Uuid,
    pub project: String,
    /// A Status the Scheme does not define reads as Triage, so the Issue comes
    /// back for attention.
    pub category: Option<Category>,
    /// Absent while unset, except priority, archived, created and updated.
    pub fields: BTreeMap<String, Value>,
    /// Each element mapped to its display form.
    pub sets: BTreeMap<String, BTreeMap<String, String>>,
}

pub(crate) fn issue(
    conn: &Connection,
    scheme: &Scheme,
    issue_id: Uuid,
) -> Result<Option<IssueView>, StoreError> {
    let Some(mut view) = built_in(conn, scheme, issue_id)? else {
        return Ok(None);
    };
    scheme_values(conn, scheme, &mut view)?;
    let mut stmt =
        conn.prepare_cached("SELECT field, elem, display FROM set_members WHERE issue_id = ?1")?;
    let members = stmt.query_map([issue_id.as_bytes()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    for member in members {
        let (field, elem, display) = member?;
        let display = display.unwrap_or_else(|| elem.clone());
        view.sets.entry(field).or_default().insert(elem, display);
    }
    Ok(Some(view))
}

struct IssueRow {
    project: String,
    issue_type: Option<String>,
    status: Option<String>,
    priority: i64,
    parent: Option<Vec<u8>>,
    archived: bool,
    created_ms: i64,
    updated_ms: i64,
    /// Text columns read as they are: title, description, assignee, reporter.
    texts: [(&'static str, Option<String>); 4],
}

fn built_in(
    conn: &Connection,
    scheme: &Scheme,
    issue_id: Uuid,
) -> Result<Option<IssueView>, StoreError> {
    let row = conn
        .query_row(
            "SELECT project, type, title, description, status, priority, assignee, reporter,
                    parent_id, archived, created_ms, updated_ms
             FROM issues WHERE id = ?1",
            [issue_id.as_bytes()],
            |row| {
                Ok(IssueRow {
                    project: row.get(0)?,
                    issue_type: row.get(1)?,
                    status: row.get(4)?,
                    priority: row.get(5)?,
                    parent: row.get(8)?,
                    archived: row.get(9)?,
                    created_ms: row.get(10)?,
                    updated_ms: row.get(11)?,
                    texts: [
                        ("title", row.get(2)?),
                        ("description", row.get(3)?),
                        ("assignee", row.get(6)?),
                        ("reporter", row.get(7)?),
                    ],
                })
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let level = PRIORITY_RANKS
        .iter()
        .find(|(_, rank)| *rank == row.priority)
        .map(|(level, _)| *level)
        .ok_or_else(|| {
            StoreError::Corrupt(format!("priority rank {} is not a level", row.priority))
        })?;
    let category = row.status.as_deref().map(|status| {
        row.issue_type
            .as_deref()
            .and_then(|issue_type| scheme.category(issue_type, status))
            .unwrap_or(Category::Triage)
    });
    let mut fields = BTreeMap::new();
    let named = [("type", row.issue_type), ("status", row.status)];
    for (name, value) in named.into_iter().chain(row.texts) {
        if let Some(value) = value {
            fields.insert(name.to_owned(), Value::String(value));
        }
    }
    fields.insert("priority".into(), json!(level));
    if let Some(parent) = row.parent {
        fields.insert("parent".into(), json!(uuid_from(&parent)?.to_string()));
    }
    fields.insert("archived".into(), json!(row.archived));
    fields.insert("created".into(), json!(row.created_ms));
    fields.insert("updated".into(), json!(row.updated_ms));
    Ok(Some(IssueView {
        id: issue_id,
        project: row.project,
        category,
        fields,
        sets: BTreeMap::new(),
    }))
}

/// Whole numbers read back as the integers they were written as.
#[allow(clippy::cast_possible_truncation)]
fn number_json(number: f64) -> Value {
    if number.fract() == 0.0 && number.abs() <= 9_007_199_254_740_992.0 {
        json!(number as i64)
    } else {
        json!(number)
    }
}

/// Scheme field values, typed by the Scheme; a field it does not define reads
/// as whichever column holds a value.
fn scheme_values(
    conn: &Connection,
    scheme: &Scheme,
    view: &mut IssueView,
) -> Result<(), StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT field, txt, num FROM field_values WHERE issue_id = ?1")?;
    let rows = stmt.query_map([view.id.as_bytes()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<f64>>(2)?,
        ))
    })?;
    for row in rows {
        let (name, txt, num) = row?;
        let field_type = scheme.field(&name).map(|field| field.field_type);
        let value = match (field_type, txt, num) {
            (Some(FieldType::Boolean), _, Some(number)) => json!(number != 0.0),
            (_, Some(string), _) => json!(string),
            (_, None, Some(number)) => number_json(number),
            (_, None, None) => {
                return Err(StoreError::Corrupt(format!(
                    "field {name} has an empty row"
                )));
            }
        };
        view.fields.insert(name, value);
    }
    Ok(())
}
