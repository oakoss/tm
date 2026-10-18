//! Reads Issues back out of the projection, applying the Scheme on the way.

use std::collections::{BTreeMap, HashMap};

use rusqlite::{Connection, Row};
use serde::Serialize;
use serde_json::{Value, json};
use tm_core::ProjectId;
use tm_core::scheme::{Category, FieldType, PRIORITY_RANKS, Scheme};
use uuid::Uuid;

use crate::StoreError;
use crate::apply::uuid_from;

/// An Issue's current values and the Status category the Scheme gives it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IssueView {
    pub id: Uuid,
    /// `None` until the Issue's create carries a Key this Replica can record:
    /// one from another Replica not yet given one, or one whose Key collided.
    pub key: Option<String>,
    pub project: ProjectId,
    /// A Status the Scheme does not define reads as Triage, so the Issue comes
    /// back for attention.
    pub category: Option<Category>,
    /// Absent while unset, except priority, archived, created and updated.
    pub fields: BTreeMap<String, Value>,
    /// Each element mapped to its display form.
    pub sets: BTreeMap<String, BTreeMap<String, String>>,
}

/// The ids, bound as one JSON array of hex strings, read back as blobs.
const IDS: &str = "(SELECT unhex(value) FROM json_each(?1))";

/// The Issues `ids` names, in that order, one query per table whatever their
/// number; an id with no Issue is left out.
pub(crate) fn issues(
    conn: &Connection,
    scheme: &Scheme,
    ids: &[Uuid],
) -> Result<Vec<IssueView>, StoreError> {
    let param = Value::from(
        ids.iter()
            .map(|id| id.simple().to_string())
            .collect::<Vec<_>>(),
    )
    .to_string();
    let mut views: HashMap<Uuid, IssueView> = HashMap::with_capacity(ids.len());
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT id, project, type, title, description, status, priority, assignee, reporter,
                parent_id, archived, created_ms, updated_ms
         FROM issues WHERE id IN {IDS}"
    ))?;
    let mut rows = stmt.query([&param])?;
    while let Some(row) = rows.next()? {
        let view = built_in(scheme, row)?;
        views.insert(view.id, view);
    }

    let mut stmt = conn.prepare_cached(&format!(
        // Without fresh statistics the planner scans keys instead.
        "SELECT issue_id, key FROM keys INDEXED BY keys_by_issue
         WHERE provisional = 0 AND issue_id IN {IDS}"
    ))?;
    let mut rows = stmt.query([&param])?;
    while let Some(row) = rows.next()? {
        if let Some(view) = views.get_mut(&uuid_from(&row.get::<_, Vec<u8>>(0)?)?) {
            view.key = Some(row.get(1)?);
        }
    }

    let mut stmt = conn.prepare_cached(&format!(
        "SELECT issue_id, field, txt, num FROM field_values WHERE issue_id IN {IDS}"
    ))?;
    let mut rows = stmt.query([&param])?;
    while let Some(row) = rows.next()? {
        if let Some(view) = views.get_mut(&uuid_from(&row.get::<_, Vec<u8>>(0)?)?) {
            let (name, value) = scheme_value(scheme, row)?;
            view.fields.insert(name, value);
        }
    }

    let mut stmt = conn.prepare_cached(&format!(
        "SELECT issue_id, field, elem, display FROM set_members WHERE issue_id IN {IDS}"
    ))?;
    let mut rows = stmt.query([&param])?;
    while let Some(row) = rows.next()? {
        if let Some(view) = views.get_mut(&uuid_from(&row.get::<_, Vec<u8>>(0)?)?) {
            let elem: String = row.get(2)?;
            let display = row
                .get::<_, Option<String>>(3)?
                .unwrap_or_else(|| elem.clone());
            view.sets
                .entry(row.get(1)?)
                .or_default()
                .insert(elem, display);
        }
    }

    let mut out: Vec<IssueView> = Vec::with_capacity(ids.len());
    let mut placed: HashMap<Uuid, usize> = HashMap::new();
    for id in ids {
        if let Some(view) = views.remove(id) {
            placed.insert(*id, out.len());
            out.push(view);
        } else if let Some(&at) = placed.get(id) {
            out.push(out[at].clone());
        }
    }
    Ok(out)
}

/// An Issue's own columns, from a row of the `issues` query.
fn built_in(scheme: &Scheme, row: &Row<'_>) -> Result<IssueView, StoreError> {
    let id = uuid_from(&row.get::<_, Vec<u8>>(0)?)?;
    let project: String = row.get(1)?;
    let issue_type: Option<String> = row.get(2)?;
    let status: Option<String> = row.get(5)?;
    let priority: i64 = row.get(6)?;
    let level = PRIORITY_RANKS
        .iter()
        .find(|(_, rank)| *rank == priority)
        .map(|(level, _)| *level)
        .ok_or_else(|| StoreError::Corrupt(format!("priority rank {priority} is not a level")))?;
    let category = status.as_deref().map(|status| {
        issue_type
            .as_deref()
            .and_then(|issue_type| scheme.category(issue_type, status))
            .unwrap_or(Category::Triage)
    });
    let mut fields = BTreeMap::new();
    let texts = [
        ("type", issue_type),
        ("status", status),
        ("title", row.get(3)?),
        ("description", row.get(4)?),
        ("assignee", row.get(7)?),
        ("reporter", row.get(8)?),
    ];
    for (name, value) in texts {
        if let Some(value) = value {
            fields.insert(name.to_owned(), Value::String(value));
        }
    }
    fields.insert("priority".into(), json!(level));
    if let Some(parent) = row.get::<_, Option<Vec<u8>>>(9)? {
        fields.insert("parent".into(), json!(uuid_from(&parent)?.to_string()));
    }
    fields.insert("archived".into(), json!(row.get::<_, bool>(10)?));
    fields.insert("created".into(), json!(row.get::<_, i64>(11)?));
    fields.insert("updated".into(), json!(row.get::<_, i64>(12)?));
    Ok(IssueView {
        id,
        key: None,
        project: crate::apply::project_from(&project)?,
        category,
        fields,
        sets: BTreeMap::new(),
    })
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

/// A Scheme field value from a `field_values` row, typed by the Scheme; a
/// field it does not define reads as whichever column holds a value.
fn scheme_value(scheme: &Scheme, row: &Row<'_>) -> Result<(String, Value), StoreError> {
    let name: String = row.get(1)?;
    let txt: Option<String> = row.get(2)?;
    let num: Option<f64> = row.get(3)?;
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
    Ok((name, value))
}
