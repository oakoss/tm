//! Reads Issues back out of the projection, applying the Scheme on the way.

use std::collections::{BTreeMap, HashMap};

use rusqlite::{Connection, Row};
use serde::Serialize;
use serde_json::{Value, json};
use tm_core::ProjectId;
use tm_core::scheme::{Category, FieldType, PRIORITY_RANKS, Scheme, WAITING_ON};
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

impl IssueView {
    /// Whom the Issue's open question waits on, set by `tm ask`.
    #[must_use]
    pub fn waiting_on(&self) -> Option<&str> {
        self.fields.get(WAITING_ON).and_then(Value::as_str)
    }
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
        // A membership reads as none unless it names a live list in the
        // Issue's Project and has a rank, which a delete or a merge can leave
        // it without (ADR 0016).
        "SELECT i.id, i.project, i.type, i.title, i.description, i.status, i.priority,
                i.assignee, i.reporter, i.parent_id, i.archived, i.created_ms, i.updated_ms,
                l.id, iif(l.id IS NULL, NULL, i.shortlist_rank)
         FROM issues i
         LEFT JOIN shortlists l
           ON l.id = i.shortlist AND l.deleted = 0 AND l.project = i.project
              AND i.shortlist_rank IS NOT NULL
         WHERE i.id IN {IDS}"
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
    if let Some(list) = row.get::<_, Option<Vec<u8>>>(13)? {
        fields.insert("shortlist".into(), json!(uuid_from(&list)?.to_string()));
        fields.insert("shortlist_rank".into(), json!(row.get::<_, String>(14)?));
    }
    Ok(IssueView {
        id,
        key: None,
        project: crate::apply::project_from(&project)?,
        category,
        fields,
        sets: BTreeMap::new(),
    })
}

/// A Link as one of its Issues sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkView {
    #[serde(rename = "type")]
    pub link_type: String,
    /// `outward` from the Link's source, `inward` from its target; a
    /// symmetric Link reads `outward` from both ends.
    pub direction: Direction,
    /// The name read from this end, such as `is blocked by`; a type this
    /// build does not know reads as its id.
    pub name: String,
    pub other: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Outward,
    Inward,
}

/// A comment, in log order: clock, then Replica, then operation (ADR 0006).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommentView {
    /// The id of the operation that added it.
    pub id: Uuid,
    pub actor: String,
    pub on_behalf_of: Option<String>,
    pub body: String,
    pub utc_ms: i64,
    pub retracted: bool,
    /// What `tm ask`, `tm answer` and `tm withdraw` wrote; `None` for a plain
    /// comment.
    pub role: Option<CommentRole>,
    /// The replies a question offers.
    pub options: Vec<String>,
}

/// The part a comment plays in a question, stored as its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentRole {
    Question,
    Answer,
    Withdrawn,
}

impl CommentRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::Answer => "answer",
            Self::Withdrawn => "withdrawn",
        }
    }

    fn parse(name: &str) -> Result<Self, StoreError> {
        match name {
            "question" => Ok(Self::Question),
            "answer" => Ok(Self::Answer),
            "withdrawn" => Ok(Self::Withdrawn),
            other => Err(StoreError::Corrupt(format!(
                "a comment's role {other:?} is none tm writes"
            ))),
        }
    }
}

const LINKS: &str = "SELECT type, dst, 0 FROM links WHERE src = ?1
     UNION ALL
     SELECT type, src, 1 FROM links WHERE dst = ?1
     ORDER BY 1, 3, 2";

const COMMENTS: &str = "SELECT c.op_id, c.actor, o.on_behalf_of, c.body, c.utc_ms, c.retracted,
     c.role, c.options FROM comments c LEFT JOIN ops o ON o.op_id = c.op_id
     WHERE c.issue_id = ?1 ORDER BY c.hlc, o.replica_id, c.op_id";

const OPEN_QUESTION: &str = "SELECT c.op_id, c.actor, o.on_behalf_of, c.body, c.utc_ms,
     c.retracted, c.role, c.options FROM comments c LEFT JOIN ops o ON o.op_id = c.op_id
     WHERE c.issue_id = ?1 AND c.role = 'question' AND c.retracted = 0
     ORDER BY c.hlc DESC, o.replica_id DESC, c.op_id DESC LIMIT 1";

pub(crate) fn links(conn: &Connection, issue: Uuid) -> Result<Vec<LinkView>, StoreError> {
    let mut stmt = conn.prepare_cached(LINKS)?;
    let mut rows = stmt.query([issue.as_bytes()])?;
    let mut links = Vec::new();
    while let Some(row) = rows.next()? {
        let link_type: String = row.get(0)?;
        let other = uuid_from(&row.get::<_, Vec<u8>>(1)?)?;
        let known = tm_core::link::link_type(&link_type);
        let inward = row.get::<_, bool>(2)? && !known.is_some_and(|def| def.symmetric);
        let name = match known {
            Some(def) if inward => def.inward,
            Some(def) => def.outward,
            None => &link_type,
        };
        links.push(LinkView {
            name: name.to_owned(),
            direction: if inward {
                Direction::Inward
            } else {
                Direction::Outward
            },
            other,
            link_type,
        });
    }
    Ok(links)
}

pub(crate) fn comments(conn: &Connection, issue: Uuid) -> Result<Vec<CommentView>, StoreError> {
    let mut stmt = conn.prepare_cached(COMMENTS)?;
    let mut rows = stmt.query([issue.as_bytes()])?;
    let mut comments = Vec::new();
    while let Some(row) = rows.next()? {
        comments.push(comment(row)?);
    }
    Ok(comments)
}

/// `issue`'s open question: its latest question not retracted, ordered as
/// [`comments`] orders them.
pub(crate) fn open_question(
    conn: &Connection,
    issue: Uuid,
) -> Result<Option<CommentView>, StoreError> {
    let mut stmt = conn.prepare_cached(OPEN_QUESTION)?;
    let mut rows = stmt.query([issue.as_bytes()])?;
    rows.next()?.map(comment).transpose()
}

/// The latest answer or withdrawal after the question asked in the batch
/// that handed `claim` on.
pub(crate) fn settled(conn: &Connection, claim: Uuid) -> Result<Option<Settled>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT c.op_id, c.actor, o.on_behalf_of, c.body, c.utc_ms, c.retracted, c.role,
           c.options, asked.options FROM op_batches h
         JOIN op_batches q ON q.batch = h.batch
         JOIN comments asked ON asked.op_id = q.op_id AND asked.role = 'question'
         JOIN comments c ON c.issue_id = asked.issue_id
           AND c.role IN ('answer', 'withdrawn') AND c.retracted = 0 AND c.hlc > asked.hlc
         LEFT JOIN ops o ON o.op_id = c.op_id
         WHERE h.op_id = ?1
         ORDER BY c.hlc DESC LIMIT 1",
    )?;
    let mut rows = stmt.query([claim.as_bytes()])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let comment = comment(row)?;
    let choice = tm_core::op::choice(&options(row, 8)?, &comment.body)
        .filter(|_| comment.role == Some(CommentRole::Answer));
    Ok(Some(Settled { comment, choice }))
}

/// How a question was settled: the answer or withdrawal, and the option an
/// answer picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settled {
    pub comment: CommentView,
    /// The 1-based number of the option the answer is, if any.
    pub choice: Option<usize>,
}

fn comment(row: &Row<'_>) -> Result<CommentView, StoreError> {
    Ok(CommentView {
        id: uuid_from(&row.get::<_, Vec<u8>>(0)?)?,
        actor: row.get(1)?,
        on_behalf_of: row.get(2)?,
        body: row.get(3)?,
        utc_ms: row.get(4)?,
        retracted: row.get(5)?,
        role: row
            .get::<_, Option<String>>(6)?
            .as_deref()
            .map(CommentRole::parse)
            .transpose()?,
        options: options(row, 7)?,
    })
}

fn options(row: &Row<'_>, column: usize) -> Result<Vec<String>, StoreError> {
    match row.get::<_, Option<String>>(column)? {
        Some(options) => serde_json::from_str(&options).map_err(|err| {
            StoreError::Corrupt(format!("a question's options are malformed: {err}"))
        }),
        None => Ok(Vec::new()),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    /// Without these, `show` scanned every Link and comment in the Workspace.
    #[test]
    fn an_issues_links_and_comments_are_found_by_index() {
        let dir = std::env::temp_dir().join(format!("tm-store-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("store.db")).unwrap();
        let plan = |sql: &str| -> Vec<String> {
            store
                .connection()
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map([Uuid::nil().as_bytes()], |row| row.get(3))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let (links, comments) = (plan(LINKS), plan(COMMENTS));
        drop(store);
        std::fs::remove_dir_all(&dir).unwrap();
        for (index, plan) in [
            ("links_by_src", &links),
            ("links_by_dst", &links),
            ("comments_by_issue", &comments),
        ] {
            assert!(
                plan.iter().any(|step| step.contains(index)),
                "{index}: {plan:?}"
            );
        }
        assert!(
            !links.iter().any(|step| step.starts_with("SCAN")),
            "{links:?}"
        );
    }
}
