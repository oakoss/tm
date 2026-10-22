//! `tm show`: one Issue, its fields and its Claim.

use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::scheme::FieldType;
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
    let (view, project, parent, (claim, now_ms), links, comments) = store.read(|store| {
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
        Ok::<_, CliError>((view, project, parent, (claim, now_ms), links, comments))
    })?;
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
