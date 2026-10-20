//! The `issue` record: an Issue's current state, as `tm export` writes it and
//! adapters write it for `tm import --records` (ADR 0013). Its JSON Schema is
//! `docs/specs/issue-record.schema.json`.

use std::collections::BTreeMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tm_core::Scheme;
use uuid::Uuid;

use crate::StoreError;
use crate::apply::uuid_from;
use crate::read::{self, IssueView};

/// The source a tm export gives its own Issues.
pub(crate) const TM_SOURCE: &str = "tm:";

/// One Issue. References to other Issues are another record's `source`, a
/// Key, or an Issue id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueRecord {
    /// `<system>:<id>`, such as `beads:tm-u8g.10`; a re-import finds the
    /// Issue by it.
    pub source: String,
    /// The key of the Project the Issue belongs to.
    pub project: String,
    /// The Issue's Key where it came from; shown, never assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub issue_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Who created it; the import's runner when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporter: Option<String>,
    /// Who last changed it; a re-import writes as them, else as its runner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Outward Links only, as each is filed under its source (ADR 0009).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkRecord>,
    /// Shown comments; a retracted one is not carried.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<CommentRecord>,
    /// Scheme fields by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, Value>,
    /// RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkRecord {
    #[serde(rename = "type")]
    pub link_type: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentRecord {
    /// Stable at the source, so a re-import knows the comment again.
    pub id: String,
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_behalf_of: Option<String>,
    pub body: String,
    /// RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// The source a tm export gives Issue `id`.
#[must_use]
pub fn tm_source(id: Uuid) -> String {
    format!("{TM_SOURCE}{id}")
}

/// An instant as RFC 3339 in UTC.
pub(crate) fn rfc3339(ms: i64) -> Result<String, StoreError> {
    jiff::Timestamp::from_millisecond(ms)
        .map(|at| at.to_string())
        .map_err(|err| StoreError::Corrupt(format!("time {ms} ms: {err}")))
}

/// Every Issue's record, ordered by id, read inside the caller's read
/// transaction.
pub(crate) fn issue_records(
    conn: &Connection,
    scheme: &Scheme,
) -> Result<Vec<IssueRecord>, StoreError> {
    let ids = {
        let mut stmt = conn.prepare("SELECT id FROM issues ORDER BY id")?;
        let rows = stmt.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
        rows.map(|row| uuid_from(&row?))
            .collect::<Result<Vec<_>, _>>()?
    };
    let keys = project_keys(conn)?;
    read::issues(conn, scheme, &ids)?
        .into_iter()
        .map(|view| issue_record(conn, &keys, view))
        .collect()
}

fn project_keys(conn: &Connection) -> Result<BTreeMap<String, String>, StoreError> {
    let mut stmt = conn.prepare("SELECT id, key FROM projects")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn issue_record(
    conn: &Connection,
    keys: &BTreeMap<String, String>,
    view: IssueView,
) -> Result<IssueRecord, StoreError> {
    let IssueView {
        id,
        key,
        project,
        mut fields,
        mut sets,
        ..
    } = view;
    let project = keys
        .get(&project.to_string())
        .cloned()
        .ok_or_else(|| StoreError::Corrupt(format!("Issue {id} has no Project {project}")))?;
    let mut text = |name: &str| match fields.remove(name) {
        Some(Value::String(text)) => Some(text),
        _ => None,
    };
    let (issue_type, title, description, status, priority, assignee, reporter) = (
        text("type"),
        text("title"),
        text("description"),
        text("status"),
        text("priority"),
        text("assignee"),
        text("reporter"),
    );
    let parent = text("parent")
        .map(|parent| Uuid::parse_str(&parent).map(tm_source))
        .transpose()
        .map_err(|err| StoreError::Corrupt(format!("Issue {id}'s parent: {err}")))?;
    let time = |value: Option<Value>| value.and_then(|v| v.as_i64()).map(rfc3339).transpose();
    let created = time(fields.remove("created"))?;
    let updated = time(fields.remove("updated"))?;
    let archived = fields.remove("archived") == Some(Value::Bool(true));
    // What is left are the Scheme fields.
    let labels = sets
        .remove("labels")
        .map(|set| set.into_values().collect())
        .unwrap_or_default();
    // A Scheme set field is carried as the list of its elements.
    for (name, set) in sets {
        fields.insert(name, Value::from(set.into_keys().collect::<Vec<_>>()));
    }
    let links = {
        let mut stmt =
            conn.prepare_cached("SELECT type, dst FROM links WHERE src = ?1 ORDER BY type, dst")?;
        let rows = stmt.query_map([id.as_bytes()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        rows.map(|row| {
            let (link_type, dst) = row?;
            Ok(LinkRecord {
                link_type,
                target: tm_source(uuid_from(&dst)?),
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?
    };
    let comments = read::comments(conn, id)?
        .into_iter()
        .filter(|comment| !comment.retracted)
        .map(|comment| {
            Ok(CommentRecord {
                id: comment.id.to_string(),
                actor: comment.actor,
                on_behalf_of: comment.on_behalf_of,
                body: comment.body,
                at: Some(rfc3339(comment.utc_ms)?),
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    Ok(IssueRecord {
        source: tm_source(id),
        project,
        key,
        issue_type,
        title,
        description,
        status,
        priority,
        assignee,
        reporter,
        updated_by: None,
        labels,
        archived,
        parent,
        links,
        comments,
        fields,
        created,
        updated,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::json;

    use super::*;

    const SCHEMA: &str = include_str!("../../../docs/specs/issue-record.schema.json");

    fn keys(value: &Value) -> BTreeSet<String> {
        value.as_object().unwrap().keys().cloned().collect()
    }

    fn names(value: &Value) -> BTreeSet<String> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap().to_owned())
            .collect()
    }

    /// The published schema and the record type say the same thing: every
    /// field a full record writes, and only the required ones on a minimal.
    /// The full records are struct literals, so a new field must be set here.
    #[test]
    fn the_published_schema_matches_the_record() {
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let text = |s: &str| Some(s.to_owned());
        let comment = CommentRecord {
            id: "1".into(),
            actor: "a".into(),
            on_behalf_of: text("b"),
            body: "c".into(),
            at: text("d"),
        };
        let link = LinkRecord {
            link_type: "blocks".into(),
            target: "x:2".into(),
        };
        let full = IssueRecord {
            source: "x:1".into(),
            project: "CPL".into(),
            key: text("CPL-1"),
            issue_type: text("task"),
            title: text("t"),
            description: text("d"),
            status: text("todo"),
            priority: text("high"),
            assignee: text("a"),
            reporter: text("r"),
            updated_by: text("u"),
            labels: vec!["l".into()],
            archived: true,
            parent: text("x:0"),
            links: vec![link.clone()],
            comments: vec![comment.clone()],
            fields: BTreeMap::from([("f".to_owned(), json!(1))]),
            created: text("c"),
            updated: text("u"),
        };
        let (comment, link) = (
            serde_json::to_value(comment).unwrap(),
            serde_json::to_value(link).unwrap(),
        );
        let mut written = keys(&serde_json::to_value(&full).unwrap());
        written.insert("_type".into());
        assert_eq!(written, keys(&schema["properties"]));

        let minimal: IssueRecord =
            serde_json::from_value(json!({"source": "x:1", "project": "CPL"})).unwrap();
        let mut required = keys(&serde_json::to_value(&minimal).unwrap());
        required.insert("_type".into());
        assert_eq!(required, names(&schema["required"]));

        let items = |name: &str| schema["properties"][name]["items"].clone();
        assert_eq!(keys(&comment), keys(&items("comments")["properties"]));
        assert_eq!(keys(&link), keys(&items("links")["properties"]));
        assert_eq!(names(&items("links")["required"]), keys(&link));
        let bare: CommentRecord =
            serde_json::from_value(json!({"id": "1", "actor": "a", "body": "c"})).unwrap();
        assert_eq!(
            names(&items("comments")["required"]),
            keys(&serde_json::to_value(bare).unwrap())
        );

        let levels: BTreeSet<String> = tm_core::scheme::PRIORITY_RANKS
            .iter()
            .map(|(level, _)| (*level).to_owned())
            .collect();
        assert_eq!(names(&schema["properties"]["priority"]["enum"]), levels);
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let err = serde_json::from_value::<IssueRecord>(
            json!({"source": "x:1", "project": "CPL", "asignee": "a"}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("asignee"), "{err}");
    }
}
