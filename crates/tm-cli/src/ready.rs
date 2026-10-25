//! `tm ready`: the work an Agent can start now, and what each would unblock,
//! after the session's Short-lists (ADR 0016).

use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::ProjectId;
use tm_store::{Member, Shortlist};
use uuid::Uuid;

use crate::error::CliError;
use crate::here::{self, Here};
use crate::identity;
use crate::list::{Item, Listed, line, project_keys, shortlists_of};
use crate::next;
use crate::output::Output;

pub(crate) fn run(limit: Option<usize>, json: bool) -> Result<Output, CliError> {
    let mut here = here::read()?;
    let lists = session_lists(&mut here);
    let read = here.store.read(|store| {
        let mut ready = store.ready()?;
        let total = ready.len();
        ready.truncate(limit.unwrap_or(total));
        let ids: Vec<Uuid> = ready.iter().map(|ready| ready.issue).collect();
        let views = store.issues(&ids)?;
        let unblocked: Vec<Uuid> = ready.iter().flat_map(|r| r.unblocks.clone()).collect();
        let keys: HashMap<Uuid, Option<String>> = store
            .issues(&unblocked)?
            .into_iter()
            .map(|view| (view.id, view.key))
            .collect();
        let projects = project_keys(store, &views)?;
        let on = shortlists_of(store, &views)?;
        Ok::<_, CliError>((total, ready, views, keys, projects, on))
    });
    let (total, ready, views, keys, projects, on) = match read {
        Ok(read) => read,
        Err(err) => return Err(err.warned(here.warnings)),
    };
    let mut text = String::new();
    let shown = next::lists_section(&mut text, &lists, next::Room::WHOLE);
    let shortlists = (!lists.is_empty()).then_some(shown);

    // Only the form printed is built, as for `tm list`.
    if json {
        let issues = ready
            .iter()
            .zip(&views)
            .map(|(entry, view)| {
                let unblocks = entry
                    .unblocks
                    .iter()
                    .map(|id| json!({ "id": id, "key": keys.get(id).cloned().flatten() }))
                    .collect();
                let mut item = Item::of(view, &projects);
                item.unblocks = Some(unblocks);
                item
            })
            .collect();
        let listed = Listed {
            issues,
            shortlists,
            total,
        };
        return match listed.output() {
            Ok(mut output) => {
                output.warnings = here.warnings;
                Ok(output)
            }
            Err(err) => Err(err.warned(here.warnings)),
        };
    }
    if shortlists.is_some() {
        let _ = writeln!(text, "Ready ({total}):");
    }
    for (entry, view) in ready.iter().zip(&views) {
        let _ = write!(text, "{}", line(view));
        if !entry.unblocks.is_empty() {
            let named: Vec<String> = entry
                .unblocks
                .iter()
                .map(|id| {
                    keys.get(id)
                        .cloned()
                        .flatten()
                        .unwrap_or_else(|| id.to_string())
                })
                .collect();
            let _ = write!(text, "  (unblocks {})", named.join(", "));
        }
        if let Some(list) = on.get(&view.id) {
            let _ = write!(text, "  [on {list}]");
        }
        text.push('\n');
    }
    if total == 0 {
        text.push_str("nothing is ready\n");
    } else if views.len() < total {
        let _ = writeln!(text, "({} of {total} shown)", views.len());
    }
    let mut output = Output::new(text, Value::Null);
    output.warnings = here.warnings;
    Ok(output)
}

/// The session's lists, which `tm ready` leads with. Who the session is, which
/// may run git, is read only when its Project has a list or `TM_SHORTLIST`
/// names one; a session that cannot say, a binding that names no list, or a
/// list it cannot read is a warning, so the queue still prints.
fn session_lists(here: &mut Here) -> Vec<(Shortlist, Vec<Member>)> {
    // The queue spans Projects, so a directory mapped to none has no lists
    // to lead with, and no warning unless TM_SHORTLIST asked for one.
    let project = match here.project() {
        Ok(project) => project,
        Err(_) if std::env::var_os("TM_SHORTLIST").is_none() => return Vec::new(),
        Err(err) => {
            let why = next::unread(&err);
            here.warnings.push(format!("no Short-lists: {why}"));
            return Vec::new();
        }
    };
    let read = match here.store.shortlists(project) {
        Ok(lists) if lists.is_empty() && std::env::var_os("TM_SHORTLIST").is_none() => {
            return Vec::new();
        }
        Ok(_) => read(here, project),
        Err(err) => Err(format!("no Short-lists: {err}")),
    };
    read.unwrap_or_else(|warning| {
        here.warnings.push(warning);
        Vec::new()
    })
}

fn read(here: &mut Here, project: ProjectId) -> Result<Vec<(Shortlist, Vec<Member>)>, String> {
    match identity::resolve(identity::env, || identity::git_email(&here.found.dir)) {
        Ok(identity) => {
            here.warnings.extend(identity.warnings);
            next::session_lists(here, project, &identity.actor).map_err(|err| next::unread(&err))
        }
        Err(message) => Err(format!("no identity, so no Short-lists: {message}")),
    }
}
