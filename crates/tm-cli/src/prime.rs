//! `tm prime`: the context an Agent session starts from (design.md
//! "Interface"): who it writes as, the Scheme, its Claims and the ready
//! queue, kept small enough for a `SessionStart` hook.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::IsTerminal as _;

use serde_json::{Value, json};
use tm_core::{Actor, Scheme};
use tm_store::Claim;
use uuid::Uuid;

use crate::error::CliError;
use crate::here;
use crate::identity;
use crate::list::{item, line_within, project_keys};
use crate::output::{Output, local_time, printable};

/// Ready Issues shown; the rest are counted, so 200 Issues stay near 1k
/// tokens.
pub(crate) const READY_SHOWN: usize = 15;

/// Claims shown in text; `--json` lists every one.
pub(crate) const CLAIMS_SHOWN: usize = 10;

/// A title's printed length in bytes, so long or wide titles cannot break the
/// budget; `tm show` gives the whole title.
pub(crate) const TITLE_BYTES: usize = 64;

/// A read: prime prints what it can, so a hook never fails a session start
/// over a missing identity or Project.
pub(crate) fn run(json: bool) -> Result<Output, CliError> {
    let here = here::open()?;
    let (actor, project, warnings) = context(&here);
    let read = here.store.read(|store| {
        let claims = match actor.as_ref().filter(|actor| actor.is_agent()) {
            Some(actor) => store.claims_held(actor.name(), holder(actor))?,
            None => Vec::new(),
        };
        let mut ready = store.ready()?;
        let total = ready.len();
        ready.truncate(READY_SHOWN);
        let ids: Vec<Uuid> = claims.iter().map(|claim| claim.issue).collect();
        let claimed = store.issues(&ids)?;
        let ids: Vec<Uuid> = ready.iter().map(|ready| ready.issue).collect();
        let ready = store.issues(&ids)?;
        let projects = project_keys(store, &[claimed.clone(), ready.clone()].concat())?;
        Ok::<_, CliError>((claims, claimed, ready, total, projects))
    });
    let (claims, claimed, ready, total, projects) =
        read.map_err(|err| err.warned(warnings.clone()))?;
    let scheme = here.store.scheme();

    let mut text = String::new();
    if !json {
        for warning in &warnings {
            let _ = writeln!(text, "warning: {}", printable(warning));
        }
    }
    let who = actor.as_ref().map_or_else(|| "nobody".to_owned(), named);
    let _ = writeln!(
        text,
        "tm: Project {}, writing as {}",
        project.as_deref().unwrap_or("-"),
        printable(&who)
    );
    let scheme = scheme_brief(&mut text, scheme);

    let by_id: HashMap<Uuid, _> = claimed.iter().map(|view| (view.id, view)).collect();
    let _ = writeln!(text, "Your Claims:");
    let mut claim_items = Vec::with_capacity(claims.len());
    for claim in &claims {
        let Some(view) = by_id.get(&claim.issue) else {
            continue;
        };
        if claim_items.len() < CLAIMS_SHOWN {
            let expiry = claim
                .expires_ms
                .map_or_else(String::new, |ms| format!(", expires {}", local_time(ms)));
            let _ = writeln!(
                text,
                "  {}  (Claim {}{expiry})",
                line_within(view, TITLE_BYTES),
                claim.id
            );
        }
        let mut entry = item(view, &projects);
        entry["claim"] = claim_json(claim);
        claim_items.push(entry);
    }
    if claim_items.is_empty() {
        let _ = writeln!(text, "  none");
    } else if claim_items.len() > CLAIMS_SHOWN {
        let _ = writeln!(
            text,
            "  and {} more: tm --json prime lists them all",
            claim_items.len() - CLAIMS_SHOWN
        );
    }

    let _ = writeln!(text, "Ready ({total}):");
    if total == 0 {
        let _ = writeln!(text, "  nothing is ready");
    }
    for view in &ready {
        let _ = writeln!(text, "  {}", line_within(view, TITLE_BYTES));
    }
    if ready.len() < total {
        let _ = writeln!(text, "  and {} more: tm ready", total - ready.len());
    }

    let data = json!({
        "project": project,
        "scheme": scheme,
        "actor": actor.as_ref().map(|actor| json!({
            "name": actor.name(),
            "agent": actor.is_agent(),
            "on_behalf_of": actor.on_behalf_of(),
        })),
        "claims": claim_items,
        "ready": {
            "issues": ready.iter().map(|view| item(view, &projects)).collect::<Vec<_>>(),
            "total": total,
        },
    });
    let mut output = Output::new(text, data);
    if json {
        output.warnings = warnings;
    }
    Ok(output)
}

/// Who the session writes as and its Project, each `None` with a warning
/// rather than an error.
fn context(here: &here::Here) -> (Option<Actor>, Option<String>, Vec<String>) {
    let mut warnings = here.warnings.clone();
    let actor = match identity::resolve(identity::env, || identity::git_email(&here.found.dir)) {
        Ok(identity) => {
            warnings.extend(identity.warnings);
            Some(identity.actor)
        }
        Err(message) => {
            warnings.push(format!("no identity: {message}"));
            None
        }
    };
    // In a hook, whose stdout is not a terminal, an unnamed Agent writes as
    // its human; prime is the one place to catch that.
    if let Some(actor) = actor.as_ref().filter(|actor| !actor.is_agent())
        && !std::io::stdout().is_terminal()
    {
        warnings.push(format!(
            "this session writes as {}; an Agent sets TM_AGENT to its name",
            actor.name()
        ));
    }
    let project = match here::mapped_project(&here.store, &here.found, &here.cwd) {
        Ok((_, key)) => Some(key.to_owned()),
        Err(err) => {
            warnings.push(err.message);
            None
        }
    };
    (actor, project, warnings)
}

/// The human a Claim taken by `actor` is held for.
fn holder(actor: &Actor) -> &str {
    actor.on_behalf_of().unwrap_or_else(|| actor.name())
}

fn named(actor: &Actor) -> String {
    match actor.on_behalf_of() {
        Some(human) => format!("{} for {human}", actor.name()),
        None => actor.name().to_owned(),
    }
}

fn claim_json(claim: &Claim) -> Value {
    json!({
        "id": claim.id,
        "agent": claim.agent,
        "on_behalf_of": claim.on_behalf_of,
        "expires_ms": claim.expires_ms,
    })
}

/// The Scheme in brief: types, each Workflow's Statuses with their
/// categories, and the fields a create must set; `tm scheme show` has the
/// rest.
fn scheme_brief(text: &mut String, scheme: &Scheme) -> Value {
    let types: Vec<&str> = scheme.types.iter().map(|kind| kind.id.as_str()).collect();
    let _ = writeln!(text, "Scheme {}: types {}", scheme.name, types.join(", "));
    let mut workflows = Vec::new();
    for workflow in &scheme.workflows {
        let statuses: Vec<String> = workflow
            .statuses
            .iter()
            .map(|status| format!("{} ({})", status.id, status.category.id()))
            .collect();
        let _ = writeln!(text, "  statuses: {}", statuses.join(", "));
        workflows.push(json!({
            "name": workflow.name,
            "statuses": workflow.statuses.iter().map(|status| json!({
                "id": status.id,
                "category": status.category.id(),
            })).collect::<Vec<_>>(),
        }));
    }
    let required: Vec<&str> = scheme
        .fields
        .iter()
        .filter(|field| field.required)
        .map(|field| field.name.as_str())
        .collect();
    let _ = writeln!(text, "  required: {}", required.join(", "));
    json!({
        "name": scheme.name,
        "types": types,
        "workflows": workflows,
        "required": required,
    })
}
