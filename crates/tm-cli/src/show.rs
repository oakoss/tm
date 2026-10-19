//! `tm show`: one Issue, its fields and its Claim.

use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_store::{CommentView, LinkView, Store};
use uuid::Uuid;

use crate::error::CliError;
use crate::here::{self, Here};
use crate::output::{Output, printable};

pub(crate) fn run(reference: &str) -> Result<Output, CliError> {
    let here = here::open()?;
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
    let (view, project, parent, claim, links, comments) = store.read(|store| {
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
        let claim = store.claim(id)?;
        let links = store.links(id)?;
        let others: Vec<Uuid> = links.iter().map(|link| link.other).collect();
        let keys: HashMap<Uuid, Option<String>> = store
            .issues(&others)?
            .into_iter()
            .map(|other| (other.id, other.key))
            .collect();
        let links: Vec<(LinkView, Option<String>)> = links
            .into_iter()
            .map(|link| {
                let key = keys.get(&link.other).cloned().flatten();
                (link, key)
            })
            .collect();
        let comments = store.comments(id)?;
        Ok::<_, CliError>((view, project, parent, claim, links, comments))
    })?;
    let data = json!({
        "id": view.id,
        "key": view.key,
        "project": project,
        "category": view.category,
        "fields": view.fields,
        "sets": view.sets,
        "parent": parent,
        "claim": claim.as_ref().map(|claim| json!({
            "id": claim.id,
            "agent": claim.agent,
            "on_behalf_of": claim.on_behalf_of,
            "expires_ms": claim.expires_ms,
        })),
        "links": links.iter().map(|(link, key)| json!({
            "type": link.link_type,
            "direction": link.direction,
            "name": link.name,
            "other": { "id": link.other, "key": key },
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
        if field == "title" {
            continue;
        }
        let shown = match (field.as_str(), &parent) {
            ("parent", Some(parent)) => parent["key"]
                .as_str()
                .map_or_else(|| plain(&parent["id"]), str::to_owned),
            _ => plain(value),
        };
        let _ = writeln!(text, "{field}: {}", printable(&shown));
    }
    for (field, members) in &view.sets {
        let shown: Vec<String> = members.values().map(|member| printable(member)).collect();
        let _ = writeln!(text, "{field}: {}", shown.join(", "));
    }
    if let Some(claim) = &claim {
        let _ = writeln!(
            text,
            "claimed by {} for {} (Claim {})",
            claim.agent, claim.on_behalf_of, claim.id
        );
    }
    for (link, key) in &links {
        let other = key.clone().unwrap_or_else(|| link.other.to_string());
        let _ = writeln!(text, "{} {other}", link.name);
    }
    for comment in &comments {
        write_comment(&mut text, comment);
    }
    Ok(Output::new(text, data))
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
