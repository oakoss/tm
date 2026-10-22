//! `tm show`: one Issue, its fields and its Claim.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::Scheme;
use tm_core::scheme::{Category, FieldType};
use tm_store::{ClaimState, CommentView, IssueView, LinkView, Store};
use uuid::Uuid;

use crate::claim::{self, claim_json};
use crate::error::CliError;
use crate::follow;
use crate::here::{self, Here};
use crate::output::{Output, local_time, printable};

pub(crate) fn run(reference: &str) -> Result<Output, CliError> {
    let here = here::read()?;
    match here.issue(reference).and_then(|id| render(&here, id)) {
        Ok(mut output) => {
            output.warnings = here.warnings;
            Ok(output)
        }
        Err(err) => Err(err.warned(here.warnings)),
    }
}

/// An Issue as `show` prints it; `create` prints the Issue it made the same way.
pub(crate) fn render(here: &Here, id: Uuid) -> Result<Output, CliError> {
    let store = &here.store;
    let (view, project, parent, (claim, now_ms), links, below) = store.read(|store| {
        let view = store.issue(id)?.ok_or_else(|| {
            CliError::new("not_found", format!("Issue {id} does not exist"))
                .with("kind", "issue")
                .with("id", id.to_string())
        })?;
        let project = store.project_key(view.project)?;
        let parent = match view.fields.get("parent").and_then(Value::as_str) {
            Some(parent) => Some(parent_ref(store, parent)?),
            None => None,
        };
        let now_ms = store.utc_ms()?;
        let claim = store.claim_state_at(id, now_ms)?;
        let links = store.links(id)?;
        let others: Vec<Uuid> = links.iter().map(|link| link.other).collect();
        let others: HashMap<Uuid, IssueView> = store
            .issues(&others)?
            .into_iter()
            .map(|other| (other.id, other))
            .collect();
        let links: Vec<(LinkView, Option<IssueView>)> = links
            .into_iter()
            .map(|link| {
                let other = others.get(&link.other).cloned();
                (link, other)
            })
            .collect();
        let comments = store.comments(id)?;
        let children = count_children(store.scheme(), &store.children(id)?);
        Ok::<_, CliError>((
            view,
            project,
            parent,
            (claim, now_ms),
            links,
            (comments, children),
        ))
    })?;
    let (comments, children) = below;
    let data = json!({
        "id": view.id,
        "key": view.key,
        "project": project,
        "category": view.category,
        "fields": view.fields,
        "sets": view.sets,
        "parent": parent,
        "claim": claim.as_ref().and_then(ClaimState::held).map(|held| claim_json(held, now_ms)),
        "lapsed_claim": claim
            .as_ref()
            .and_then(ClaimState::lapsed)
            .map(|lapsed| claim_json(lapsed, now_ms)),
        "links": links.iter().map(|(link, other)| json!({
            "type": link.link_type,
            "direction": link.direction,
            "name": link.name,
            "other": {
                "id": link.other,
                "key": other.as_ref().and_then(|other| other.key.clone()),
                "status": other.as_ref().and_then(|other| other.fields.get("status")),
                "category": other.as_ref().and_then(|other| other.category),
                "archived": other.as_ref().map(follow::archived),
            },
        })).collect::<Vec<_>>(),
        "comments": comments.iter().map(comment_json).collect::<Vec<_>>(),
        "children": children.json(),
    });

    let name = view.key.clone().unwrap_or_else(|| view.id.to_string());
    let title = view
        .fields
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut text = format!("{name}  {}\n", printable(title));
    for (field, value) in &view.fields {
        if field != "title" {
            write_field(&mut text, store, field, value, parent.as_ref());
        }
    }
    for (field, members) in &view.sets {
        let shown: Vec<String> = members.values().map(|member| printable(member)).collect();
        let _ = writeln!(text, "{field}: {}", shown.join(", "));
    }
    write_claim(&mut text, claim.as_ref(), &view, now_ms);
    for (link, other) in &links {
        let _ = writeln!(text, "{}", link_line(link, other.as_ref()));
    }
    if let Some(line) = children.line() {
        let _ = writeln!(text, "{line}");
    }
    for comment in &comments {
        write_comment(&mut text, comment);
    }
    Ok(Output::new(text, data))
}

fn write_claim(text: &mut String, state: Option<&ClaimState>, view: &IssueView, now_ms: i64) {
    match state {
        None => {}
        Some(ClaimState::Held(claim)) => {
            let _ = writeln!(
                text,
                "claimed by {}, {} (Claim {})",
                claim::holder(claim),
                claim::expiry(claim, now_ms),
                claim.id
            );
        }
        Some(ClaimState::Lapsed(claim)) => {
            let takeable = if claim::takeable(view) {
                "; any Agent may take it"
            } else {
                ""
            };
            let _ = writeln!(
                text,
                "Claim by {} {} (Claim {}){takeable}",
                claim::holder(claim),
                claim::expiry(claim, now_ms),
                claim.id
            );
        }
    }
}

/// A Link by the other Issue's Key, marked when the ready queue no longer
/// counts the other Issue, so it does not read as open.
fn link_line(link: &LinkView, other: Option<&IssueView>) -> String {
    let name = other
        .and_then(|other| other.key.clone())
        .unwrap_or_else(|| link.other.to_string());
    let mut marks = Vec::new();
    if let Some(status) = other
        .filter(|other| follow::finished(other))
        .and_then(|other| other.fields.get("status").and_then(Value::as_str))
    {
        marks.push(printable(status));
    }
    if other.is_some_and(follow::archived) {
        marks.push("archived".to_owned());
    }
    if marks.is_empty() {
        format!("{} {name}", link.name)
    } else {
        format!("{} {name} ({})", link.name, marks.join(", "))
    }
}

/// One field: a parent by Key, a time stored in milliseconds in local time,
/// and text of several lines indented under the field's name.
fn write_field(
    text: &mut String,
    store: &Store,
    field: &str,
    value: &Value,
    parent: Option<&Value>,
) {
    let instant = store
        .scheme()
        .field(field)
        .filter(|def| def.field_type == FieldType::Datetime)
        .and(value.as_i64());
    let shown = match (field, parent, instant) {
        ("parent", Some(parent), _) => parent["key"]
            .as_str()
            .map_or_else(|| plain(&parent["id"]), str::to_owned),
        (_, _, Some(utc_ms)) => local_time(utc_ms),
        _ => plain(value),
    };
    if shown.contains('\n') {
        let _ = writeln!(text, "{field}:");
        for line in shown.lines() {
            let _ = writeln!(text, "  {}", printable(line));
        }
    } else {
        let _ = writeln!(text, "{field}: {}", printable(&shown));
    }
}

/// A retracted comment keeps its place and author, not its words.
fn comment_json(comment: &CommentView) -> Value {
    json!({
        "id": comment.id,
        "actor": comment.actor,
        "on_behalf_of": comment.on_behalf_of,
        "body": (!comment.retracted).then_some(&comment.body),
        "utc_ms": comment.utc_ms,
        "retracted": comment.retracted,
    })
}

fn write_comment(text: &mut String, comment: &CommentView) {
    let by = match &comment.on_behalf_of {
        Some(human) => format!("{} for {human}", comment.actor),
        None => comment.actor.clone(),
    };
    let by = format!("{by}, {}", local_time(comment.utc_ms));
    if comment.retracted {
        let _ = writeln!(text, "\n{by}: (retracted)");
        return;
    }
    let _ = writeln!(text, "\n{by}:");
    for line in comment.body.lines() {
        let _ = writeln!(text, "  {}", printable(line));
    }
}

/// An Issue's children by Status, and the archived ones counted apart, since
/// a list leaves them out.
struct Children {
    /// Finished work first and cancelled last, so progress reads left to right.
    by_status: Vec<(Option<String>, usize)>,
    archived: usize,
}

impl Children {
    fn json(&self) -> Value {
        let by_status: Vec<Value> = self
            .by_status
            .iter()
            .map(|(status, count)| json!({ "status": status, "count": count }))
            .collect();
        json!({ "by_status": by_status, "archived": self.archived })
    }

    /// Such as `Children: 12 done, 1 in_review, 5 todo (2 archived)`.
    fn line(&self) -> Option<String> {
        let counts: Vec<String> = self
            .by_status
            .iter()
            .map(|(status, count)| match status {
                Some(status) => format!("{count} {}", printable(status)),
                None => format!("{count} with no status"),
            })
            .collect();
        match (counts.is_empty(), self.archived) {
            (true, 0) => None,
            (true, archived) => Some(format!("Children: {archived} archived")),
            (false, 0) => Some(format!("Children: {}", counts.join(", "))),
            (false, archived) => Some(format!(
                "Children: {} ({archived} archived)",
                counts.join(", ")
            )),
        }
    }
}

/// Where a Status sorts: by category, then later in its Workflow first.
type Place = (u8, Reverse<usize>);

fn count_children(scheme: &Scheme, children: &[IssueView]) -> Children {
    let mut by_status: Vec<(Place, Option<String>, usize)> = Vec::new();
    let mut archived = 0;
    for child in children {
        if follow::archived(child) {
            archived += 1;
            continue;
        }
        let status = child.fields.get("status").and_then(Value::as_str);
        // A Status the Scheme does not define reads as Triage.
        let rank = match child.category {
            Some(Category::Done) => 0,
            Some(Category::Active) => 1,
            Some(Category::Todo) => 2,
            Some(Category::Triage) | None => 3,
            Some(Category::Cancelled) => 4,
        };
        let place = child
            .fields
            .get("type")
            .and_then(Value::as_str)
            .and_then(|issue_type| scheme.workflow(issue_type))
            .and_then(|workflow| {
                workflow
                    .statuses
                    .iter()
                    .position(|s| Some(s.id.as_str()) == status)
            })
            .unwrap_or_default();
        let at = (rank, Reverse(place));
        // Children of other types can place one Status differently; the
        // earliest place wins, so the order does not follow the read order.
        let seen = by_status
            .iter_mut()
            .find(|(_, id, _)| id.as_deref() == status);
        match seen {
            Some((first, _, count)) => {
                *first = (*first).min(at);
                *count += 1;
            }
            None => by_status.push((at, status.map(str::to_owned), 1)),
        }
    }
    by_status.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    Children {
        by_status: by_status
            .into_iter()
            .map(|(_, status, count)| (status, count))
            .collect(),
        archived,
    }
}

/// A parent by id and, where it has one, Key.
fn parent_ref(store: &Store, parent: &str) -> Result<Value, CliError> {
    let key = match Uuid::parse_str(parent) {
        Ok(id) => store.issue(id)?.and_then(|view| view.key),
        Err(_) => None,
    };
    Ok(json!({ "id": parent, "key": key }))
}

/// A value as a person reads it: strings without quotes.
fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tm_core::ProjectId;

    use super::*;

    fn child(issue_type: &str, status: Option<&str>, category: Option<Category>) -> IssueView {
        let mut fields = BTreeMap::from([("type".to_owned(), json!(issue_type))]);
        if let Some(status) = status {
            fields.insert("status".into(), json!(status));
        }
        IssueView {
            id: Uuid::now_v7(),
            key: None,
            project: ProjectId::from_uuid(Uuid::nil()),
            category,
            fields,
            sets: BTreeMap::new(),
        }
    }

    #[test]
    fn children_order_the_same_whatever_order_they_are_read_in() {
        let scheme = Scheme::built_in();
        let mut children = vec![
            // An undefined type has no category.
            child("gone", Some("done"), None),
            child("task", Some("done"), Some(Category::Done)),
            child("task", Some("in_progress"), Some(Category::Active)),
            child("task", Some("blocked"), Some(Category::Triage)),
            child("task", Some("cancelled"), Some(Category::Cancelled)),
            child("task", None, None),
        ];
        let line = "Children: 2 done, 1 in_progress, 1 with no status, 1 blocked, 1 cancelled";
        let counted = count_children(&scheme, &children);
        assert_eq!(counted.line().as_deref(), Some(line));
        let unset = json!({ "status": null, "count": 1 });
        assert_eq!(counted.json()["by_status"][2], unset);
        children.reverse();
        let reversed = count_children(&scheme, &children);
        assert_eq!(reversed.line().as_deref(), Some(line));
    }
}
