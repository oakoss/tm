//! `tm list`: the Issues a filter matches.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::ProjectId;
use tm_core::filter::{Context, FilterError, parse};
use tm_store::{IssueView, StoreError};
use uuid::Uuid;

use crate::error::CliError;
use crate::here::{self, Here};
use crate::identity;
use crate::output::Output;

/// With no filter, the open work: triage, todo and active.
const OPEN: &str = "-category:done,cancelled";

pub(crate) fn run(filter: &[String], limit: Option<usize>) -> Result<Output, CliError> {
    let here = here::open()?;
    match list(&here, filter, limit) {
        Ok(mut output) => {
            output.warnings = here.warnings;
            Ok(output)
        }
        Err(err) => Err(err.warned(here.warnings)),
    }
}

fn list(here: &Here, filter: &[String], limit: Option<usize>) -> Result<Output, CliError> {
    let filter = filter.join(" ");
    // A filter that only sorts still lists the open work. The default goes
    // last so error spans still point into what was typed.
    let filter = match parse(&filter) {
        Ok(query) if query.terms.is_empty() => format!("{filter} {OPEN}"),
        _ => filter,
    };
    let (total, views, projects) = here.store.read(|store| {
        let ids = find(here, &filter)?;
        let shown = &ids[..limit.map_or(ids.len(), |limit| limit.min(ids.len()))];
        let views = store.issues(shown)?;
        let mut projects: HashMap<ProjectId, Option<String>> = HashMap::new();
        for view in &views {
            if let Entry::Vacant(entry) = projects.entry(view.project) {
                entry.insert(store.project_key(view.project)?);
            }
        }
        Ok::<_, CliError>((ids.len(), views, projects))
    })?;

    let mut items = Vec::with_capacity(views.len());
    let mut text = String::new();
    for view in &views {
        let project = &projects[&view.project];
        let _ = writeln!(text, "{}", line(view));
        items.push(json!({
            "id": view.id,
            "key": view.key,
            "project": project,
            "category": view.category,
            "fields": view.fields,
            "sets": view.sets,
        }));
    }
    if total == 0 {
        text.push_str("no Issues match\n");
    } else if views.len() < total {
        let _ = writeln!(text, "({} of {total} shown)", views.len());
    }
    Ok(Output::new(
        text,
        json!({ "issues": items, "total": total }),
    ))
}

/// The matching ids. `@me` is the human, also when an Agent runs the
/// command; who that is is worked out only when the filter asks, since it
/// can mean running git.
fn find(here: &Here, filter: &str) -> Result<Vec<Uuid>, CliError> {
    match here.store.find(filter, Context { me: None }) {
        Err(StoreError::Filter(FilterError::NoIdentity { .. })) => {}
        other => return Ok(other?),
    }
    match identity::resolve(identity::env, || identity::git_email(&here.found.dir)) {
        Ok(identity) => {
            let me = identity
                .actor
                .on_behalf_of()
                .unwrap_or_else(|| identity.actor.name());
            Ok(here.store.find(filter, Context { me: Some(me) })?)
        }
        Err(reason) => Err(CliError::new(
            "no_identity",
            format!("@me needs to know who you are: {reason}"),
        )),
    }
}

/// `KEY  status  priority  title`, the Key falling back to the id, on one
/// line whatever the title holds.
fn line(view: &IssueView) -> String {
    let field = |name: &str| view.fields.get(name).and_then(Value::as_str).unwrap_or("-");
    let title: String = field("title")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!(
        "{}  {}  {}  {title}",
        view.key.clone().unwrap_or_else(|| view.id.to_string()),
        field("status"),
        field("priority"),
    )
}
