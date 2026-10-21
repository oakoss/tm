//! The beads adapter: `bd export` lines as `issue` records (ADR 0013). A type
//! or Status with no exact counterpart keeps the original as a label, and
//! whatever else tm cannot hold is counted.

use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;

use serde::Serialize;
use serde_json::{Map, Value};
use tm_core::{Actor, link};

use crate::export::unreadable;
use crate::import::{Parsed, repeated_comment};
use crate::record::{CommentRecord, IssueRecord, LinkRecord};
use crate::{RecordsImported, Store, StoreError};

const SOURCE: &str = "beads:";

/// Fields the conversion reads, or derives and need not keep.
const READ: [&str; 20] = [
    "_type",
    "id",
    "title",
    "description",
    "design",
    "acceptance_criteria",
    "notes",
    "status",
    "priority",
    "issue_type",
    "assignee",
    "owner",
    "created_by",
    "labels",
    "comments",
    "close_reason",
    "created_at",
    "updated_at",
    "closed_at",
    "dependencies",
];

/// Derived counts, rebuilt from what is kept.
const DERIVED: [&str; 3] = ["dependency_count", "dependent_count", "comment_count"];

/// What a beads import did, and what it could not keep.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BeadsImported {
    pub imported: RecordsImported,
    /// Each thing tm has no place for, such as `started_at` or `dependency
    /// waits-for`, with how many times the file holds it.
    pub dropped: Dropped,
}

type Dropped = BTreeMap<String, usize>;

/// A `bd export` read and checked for Project `project`, nothing written yet.
#[derive(Debug)]
pub struct BeadsExport {
    issues: Vec<(usize, IssueRecord)>,
    dropped: Dropped,
}

/// Reads `bd export`'s Issues as records for Project `project`.
///
/// # Errors
///
/// [`StoreError::InvalidExport`] for a line that is not a beads Issue or
/// holds a time that is not RFC 3339.
pub fn read_beads(input: &mut dyn BufRead, project: &str) -> Result<BeadsExport, StoreError> {
    let (issues, dropped) = records(input, project)?;
    // The write would refuse these too; refusing them here needs no store.
    for (line, issue) in &issues {
        let times = [&issue.created, &issue.updated]
            .into_iter()
            .chain(issue.comments.iter().map(|comment| &comment.at));
        for at in times.flatten() {
            at.parse::<jiff::Timestamp>()
                .map_err(|err| StoreError::InvalidExport {
                    line: *line,
                    reason: format!("{at:?} is not an RFC 3339 time: {err}"),
                })?;
        }
    }
    Ok(BeadsExport { issues, dropped })
}

impl Store {
    /// Writes `bd export`'s Issues into Project `project` as records, so a
    /// re-import compares three ways like any other.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvalidExport`] for a line that is not a beads Issue,
    /// before anything is written; otherwise as [`Store::import_records`].
    pub fn import_beads(
        &mut self,
        input: &mut dyn BufRead,
        project: &str,
        runner: &Actor,
    ) -> Result<BeadsImported, StoreError> {
        self.write_beads(read_beads(input, project)?, runner)
    }

    /// Writes an export [`read_beads`] checked.
    ///
    /// # Errors
    ///
    /// As [`Store::import_records`].
    pub fn write_beads(
        &mut self,
        export: BeadsExport,
        runner: &Actor,
    ) -> Result<BeadsImported, StoreError> {
        let imported = self.write_records(&Parsed::issues(export.issues), runner)?;
        Ok(BeadsImported {
            imported,
            dropped: export.dropped,
        })
    }
}

/// One beads dependency, read as "`issue` depends on `on`".
struct Edge {
    line: usize,
    issue: String,
    on: String,
    kind: String,
}

/// Every record the file gives, by line. Edges are placed once every Issue is
/// read, since beads files a `blocks` edge on the blocked Issue and tm on the
/// blocker.
fn records(
    input: &mut dyn BufRead,
    project: &str,
) -> Result<(Vec<(usize, IssueRecord)>, Dropped), StoreError> {
    let mut dropped = BTreeMap::new();
    let mut issues = Vec::new();
    let mut edges = Vec::new();
    let mut lines = HashMap::new();
    for (index, text) in input.lines().enumerate() {
        let line = index + 1;
        let text = text.map_err(StoreError::Io)?;
        if text.trim().is_empty() {
            continue;
        }
        let invalid = |reason: String| StoreError::InvalidExport { line, reason };
        let value: Value = serde_json::from_str(&text).map_err(|err| invalid(unreadable(&err)))?;
        let Value::Object(bead) = value else {
            return Err(invalid("the line is not a JSON object".into()));
        };
        match bead.get("_type").and_then(Value::as_str) {
            None | Some("issue") => {}
            Some(other) => {
                count(&mut dropped, format!("{other} line"));
                continue;
            }
        }
        // A deleted Issue, which bd import skips too.
        if bead.get("status").and_then(Value::as_str) == Some("tombstone") {
            count(&mut dropped, "tombstone Issue".into());
            continue;
        }
        let record = record(&bead, project, &mut dropped).map_err(invalid)?;
        if let Some(first) = lines.insert(record.source.clone(), line) {
            return Err(invalid(format!("{} is also line {first}", record.source)));
        }
        if let Some(id) = repeated_comment(&record) {
            return Err(invalid(format!("comment id {id} appears twice")));
        }
        edges.extend(dependencies(&bead, line, &mut dropped).map_err(invalid)?);
        issues.push((line, record));
    }
    issues.sort_by_cached_key(|(_, record)| creation_order(record));
    place(&mut issues, edges, &mut dropped)?;
    Ok((issues, dropped))
}

fn count(dropped: &mut Dropped, what: String) {
    *dropped.entry(what).or_default() += 1;
}

fn text(bead: &Map<String, Value>, field: &str) -> Result<Option<String>, String> {
    match bead.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.is_empty() => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("{field} is not text")),
    }
}

/// Adds a label unless the bead already holds it in any spelling tm reads as
/// the same.
fn add_label(labels: &mut Vec<String>, label: String) {
    let key = |label: &str| label.trim().to_lowercase();
    if !labels.iter().any(|held| key(held) == key(&label)) {
        labels.push(label);
    }
}

/// beads lists newest first; creating oldest first gives Keys in creation
/// order, with an id's numeric parts breaking a tie.
fn creation_order(record: &IssueRecord) -> (Option<i64>, Vec<(u64, String)>) {
    let created = record
        .created
        .as_deref()
        .and_then(|at| at.parse::<jiff::Timestamp>().ok())
        .map(jiff::Timestamp::as_millisecond);
    let parts = record
        .source
        .split('.')
        .map(|part| {
            part.parse()
                .map_or_else(|_| (0, part.to_owned()), |number| (number, String::new()))
        })
        .collect();
    (created, parts)
}

fn present(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(text) => !text.is_empty() && text != "{}",
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

fn record(
    bead: &Map<String, Value>,
    project: &str,
    dropped: &mut Dropped,
) -> Result<IssueRecord, String> {
    let id = text(bead, "id")?.ok_or("the Issue has no id")?;
    let source = format!("{SOURCE}{id}");
    let mut labels = match bead.get("labels") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(labels)) => labels
            .iter()
            .map(|label| label.as_str().map(str::to_owned))
            .collect::<Option<_>>()
            .ok_or("labels are not all text")?,
        Some(_) => return Err("labels is not a list".into()),
    };
    // Absent, each is beads' default.
    let kind = text(bead, "issue_type")?.unwrap_or_else(|| "task".into());
    let issue_type = tm_type(&kind).to_owned();
    if issue_type != kind {
        add_label(&mut labels, format!("beads-type:{kind}"));
    }
    let status = text(bead, "status")?.unwrap_or_else(|| "open".into());
    if !matches!(status.as_str(), "open" | "in_progress" | "closed") {
        add_label(&mut labels, format!("beads-status:{status}"));
    }
    let priority = match bead.get("priority") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let level = value.as_u64().and_then(tm_priority);
            if level.is_none() {
                count(dropped, "priority".into());
            }
            level.map(str::to_owned)
        }
    };
    let reporter = text(bead, "owner")?.or(text(bead, "created_by")?);
    let mut comments = comments(bead, dropped)?;
    if let Some(reason) = text(bead, "close_reason")? {
        match &reporter {
            Some(actor) => comments.push(CommentRecord {
                id: "close_reason".into(),
                actor: actor.clone(),
                on_behalf_of: None,
                body: reason,
                at: text(bead, "closed_at")?,
            }),
            None => count(dropped, "close_reason".into()),
        }
    }
    for (field, value) in bead {
        if !READ.contains(&field.as_str()) && !DERIVED.contains(&field.as_str()) && present(value) {
            count(dropped, field.clone());
        }
    }
    Ok(IssueRecord {
        source,
        project: project.to_owned(),
        key: None,
        issue_type: Some(issue_type),
        title: text(bead, "title")?,
        description: description(bead)?,
        status: Some(tm_status(&status).to_owned()),
        priority,
        assignee: text(bead, "assignee")?,
        reporter,
        updated_by: None,
        labels,
        archived: false,
        parent: None,
        links: Vec::new(),
        comments,
        fields: BTreeMap::new(),
        created: text(bead, "created_at")?,
        updated: text(bead, "updated_at")?,
    })
}

/// beads' types as the default Scheme's.
fn tm_type(kind: &str) -> &str {
    match kind {
        "epic" | "story" | "task" | "bug" | "spike" => kind,
        "feature" => "story",
        "decision" => "design",
        _ => "task",
    }
}

/// beads' Statuses as the default Workflow's; blocking is derived from
/// `blocks` Links.
fn tm_status(status: &str) -> &'static str {
    match status {
        "in_progress" | "hooked" => "in_progress",
        "closed" => "done",
        _ => "todo",
    }
}

/// beads' 0 (critical) to 4 (backlog).
fn tm_priority(level: u64) -> Option<&'static str> {
    ["urgent", "high", "medium", "low", "none"]
        .get(usize::try_from(level).ok()?)
        .copied()
}

/// The description, then beads' design, acceptance criteria and notes, each
/// under a heading, in a fixed form so a re-import compares equal.
fn description(bead: &Map<String, Value>) -> Result<Option<String>, String> {
    let mut parts = Vec::new();
    if let Some(description) = text(bead, "description")? {
        parts.push(description);
    }
    for (field, heading) in [
        ("design", "Design"),
        ("acceptance_criteria", "Acceptance criteria"),
        ("notes", "Notes"),
    ] {
        if let Some(body) = text(bead, field)? {
            parts.push(format!("## {heading}\n\n{body}"));
        }
    }
    Ok((!parts.is_empty()).then(|| parts.join("\n\n")))
}

fn comments(
    bead: &Map<String, Value>,
    dropped: &mut Dropped,
) -> Result<Vec<CommentRecord>, String> {
    let comments = match bead.get("comments") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(comments)) => comments,
        Some(_) => return Err("comments is not a list".into()),
    };
    comments
        .iter()
        .map(|comment| {
            let comment = comment.as_object().ok_or("a comment is not an object")?;
            for (field, value) in comment {
                let read = ["id", "issue_id", "author", "text", "created_at"];
                if !read.contains(&field.as_str()) && present(value) {
                    count(dropped, format!("comment {field}"));
                }
            }
            let id = match comment.get("id") {
                Some(Value::String(id)) => id.clone(),
                Some(Value::Number(id)) => id.to_string(),
                _ => return Err("a comment has no id".into()),
            };
            Ok(CommentRecord {
                actor: text(comment, "author")?
                    .ok_or_else(|| format!("comment {id} has no author"))?,
                on_behalf_of: None,
                body: text(comment, "text")?.unwrap_or_default(),
                at: text(comment, "created_at")?,
                id,
            })
        })
        .collect()
}

fn dependencies(
    bead: &Map<String, Value>,
    line: usize,
    dropped: &mut Dropped,
) -> Result<Vec<Edge>, String> {
    let edges = match bead.get("dependencies") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(edges)) => edges,
        Some(_) => return Err("dependencies is not a list".into()),
    };
    let issue = text(bead, "id")?.unwrap_or_default();
    edges
        .iter()
        .map(|edge| {
            let edge = edge.as_object().ok_or("a dependency is not an object")?;
            for (field, value) in edge {
                let read = ["issue_id", "depends_on_id", "type"].contains(&field.as_str());
                if !read && present(value) {
                    count(dropped, format!("dependency {field}"));
                }
            }
            Ok(Edge {
                line,
                issue: text(edge, "issue_id")?.unwrap_or_else(|| issue.clone()),
                on: text(edge, "depends_on_id")?.ok_or("a dependency names no Issue")?,
                kind: text(edge, "type")?.ok_or("a dependency has no type")?,
            })
        })
        .collect()
}

/// Files each edge on the record tm keeps it on: a parent on the child, a
/// Link on its outward end.
fn place(
    issues: &mut [(usize, IssueRecord)],
    edges: Vec<Edge>,
    dropped: &mut Dropped,
) -> Result<(), StoreError> {
    let index: HashMap<String, usize> = issues
        .iter()
        .enumerate()
        .map(|(at, (_, record))| (record.source.clone(), at))
        .collect();
    for edge in edges {
        // The Link type, and whether it reads outward from the depended-on
        // Issue; `None` for a parent.
        let mapped = match edge.kind.as_str() {
            "parent-child" => None,
            "blocks" => Some((link::BLOCKS, true)),
            "caused-by" => Some(("causes", true)),
            "related" | "relates-to" => Some(("relates_to", false)),
            "duplicates" => Some(("duplicates", false)),
            "supersedes" => Some(("supersedes", false)),
            "discovered-from" => Some(("derived_from", false)),
            other => {
                count(dropped, format!("dependency {other}"));
                continue;
            }
        };
        let (issue, on) = (
            format!("{SOURCE}{}", edge.issue),
            format!("{SOURCE}{}", edge.on),
        );
        let (Some(&issue_at), Some(&on_at)) = (index.get(&issue), index.get(&on)) else {
            count(dropped, "dependency on an Issue not imported".into());
            continue;
        };
        if issue_at == on_at {
            return Err(StoreError::InvalidExport {
                line: edge.line,
                reason: format!("{issue} depends on itself"),
            });
        }
        let Some((link_type, from_on)) = mapped else {
            let child = &mut issues[issue_at].1;
            if child.parent.is_some() {
                count(dropped, "second parent".into());
            } else {
                child.parent = Some(on);
            }
            continue;
        };
        let (from, to) = if from_on {
            (on_at, issue)
        } else {
            (issue_at, on)
        };
        let links = &mut issues[from].1.links;
        let link = LinkRecord {
            link_type: link_type.to_owned(),
            target: to,
        };
        if !links.contains(&link) {
            links.push(link);
        }
    }
    Ok(())
}
