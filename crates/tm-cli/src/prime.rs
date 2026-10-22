//! `tm prime`: the context an Agent session starts from (design.md
//! "Interface"): who it writes as, the Scheme, its Claims, the work in
//! progress and the ready queue, kept small enough for a `SessionStart` hook.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::IsTerminal as _;

use serde_json::{Value, json};
use tm_core::filter::Context;
use tm_core::{Actor, ProjectId, Scheme};
use tm_store::{AgentClaims, Claim, IssueView};
use uuid::Uuid;

use crate::claim;
use crate::error::CliError;
use crate::guide::HABITS;
use crate::here;
use crate::identity;
use crate::list::{item, line_within, project_keys};
use crate::output::{Output, printable};

/// Ready Issues shown; the rest are counted, so 200 Issues stay near 1k
/// tokens.
pub(crate) const READY_SHOWN: usize = 15;

/// Issues in an active Status shown, most recently updated first, so a new
/// session sees where the last one left off.
pub(crate) const ACTIVE_SHOWN: usize = 10;

/// Claims shown in text; `--json` lists every one.
pub(crate) const CLAIMS_SHOWN: usize = 10;

/// A title's printed length in bytes, so long or wide titles cannot break the
/// budget; `tm show` gives the whole title.
pub(crate) const TITLE_BYTES: usize = 64;

/// A read: prime prints what it can, so a hook never fails a session start
/// over a missing identity or Project.
pub(crate) fn run(json: bool) -> Result<Output, CliError> {
    let here = here::read()?;
    let (actor, project, warnings) = context(&here);
    let read = here.store.read(|store| {
        let claims = match actor.as_ref().filter(|actor| actor.is_agent()) {
            Some(actor) => store.claims_by(actor.name(), holder(actor))?,
            None => AgentClaims::default(),
        };
        let mut active = store.find("category:active", Context { me: None })?;
        let active_total = active.len();
        active.truncate(ACTIVE_SHOWN);
        let mut ready = store.ready()?;
        let total = ready.len();
        ready.truncate(READY_SHOWN);
        let ids: Vec<Uuid> = claims
            .held
            .iter()
            .chain(&claims.lapsed)
            .map(|claim| claim.issue)
            .collect();
        let claimed = store.issues(&ids)?;
        let active = store.issues(&active)?;
        let ids: Vec<Uuid> = ready.iter().map(|ready| ready.issue).collect();
        let ready = store.issues(&ids)?;
        let projects = project_keys(
            store,
            &[claimed.clone(), active.clone(), ready.clone()].concat(),
        )?;
        Ok::<_, CliError>((
            claims,
            claimed,
            (active, active_total),
            ready,
            total,
            projects,
        ))
    });
    let (claims, claimed, (active, active_total), ready, total, projects) =
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

    let session = actor.as_ref().and_then(Actor::session);
    let now_ms = claims.at_ms;
    let sessions = by_session(claims, session);
    let claim_items = claims_sections(&mut text, &sessions, &claimed, &projects, now_ms);
    section(
        &mut text,
        ("In progress", "none", "tm list category:active"),
        &active,
        active_total,
    );
    section(
        &mut text,
        ("Ready", "nothing is ready", "tm ready"),
        &ready,
        total,
    );
    let _ = writeln!(text, "Habits (tm guide has the rest):");
    for habit in HABITS {
        let _ = writeln!(text, "  {habit}");
    }

    let data = json!({
        "project": project,
        "scheme": scheme,
        "actor": actor.as_ref().map(|actor| json!({
            "name": actor.name(),
            "agent": actor.is_agent(),
            "on_behalf_of": actor.on_behalf_of(),
        })),
        "claims": claim_items.held,
        "other_session_claims": claim_items.others,
        "lapsed_claims": claim_items.lapsed,
        "in_progress": {
            "issues": active.iter().map(|view| item(view, &projects)).collect::<Vec<_>>(),
            "total": active_total,
        },
        "ready": {
            "issues": ready.iter().map(|view| item(view, &projects)).collect::<Vec<_>>(),
            "total": total,
        },
        "habits": HABITS,
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

/// An Agent's Claims as one session sees them.
struct SessionClaims {
    mine: Vec<Claim>,
    others: Vec<Claim>,
    lapsed: Vec<Claim>,
}

/// `claims` sorted by whether `session` took them. Another session's lapsed
/// Claim is left out: it reaches this session through tm ready.
fn by_session(claims: AgentClaims, session: Option<&str>) -> SessionClaims {
    let (mine, others) = claims
        .held
        .into_iter()
        .partition(|held| held.taken_in(session));
    let lapsed = claims
        .lapsed
        .into_iter()
        .filter(|lapsed| lapsed.taken_in(session))
        .collect();
    SessionClaims {
        mine,
        others,
        lapsed,
    }
}

/// This session's held and takeable lapsed Claims, and those its other
/// sessions hold, each as `--json` gives it.
fn claims_sections(
    text: &mut String,
    claims: &SessionClaims,
    claimed: &[IssueView],
    projects: &HashMap<ProjectId, Option<String>>,
    now_ms: i64,
) -> ClaimItems {
    let mine = with_issues(&claims.mine, claimed, |_| true);
    let others = with_issues(&claims.others, claimed, |_| true);
    let lapsed = with_issues(&claims.lapsed, claimed, claim::takeable);
    ClaimItems {
        held: claims_section(text, "Your Claims:", token, &mine, projects, now_ms),
        others: if others.is_empty() {
            Vec::new()
        } else {
            claims_section(
                text,
                "Held by your other sessions:",
                session,
                &others,
                projects,
                now_ms,
            )
        },
        lapsed: if lapsed.is_empty() {
            Vec::new()
        } else {
            claims_section(
                text,
                "Your lapsed Claims (another Agent may take these; tm heartbeat KEY takes one back):",
                token,
                &lapsed,
                projects,
                now_ms,
            )
        },
    }
}

/// The caller's Claims as `--json` gives them, by section.
struct ClaimItems {
    held: Vec<Value>,
    others: Vec<Value>,
    lapsed: Vec<Value>,
}

/// A Claim by the token that releases it.
fn token(claim: &Claim) -> String {
    format!("Claim {}", claim.id)
}

/// A Claim by the session holding it, so this session does not read it as
/// its own.
fn session(claim: &Claim) -> String {
    claim
        .session_label()
        .unwrap_or_else(|| "no session".to_owned())
}

/// Each of `claims` with its Issue, for those whose Issue was read and that
/// `keep` accepts.
fn with_issues<'a>(
    claims: &'a [Claim],
    claimed: &'a [IssueView],
    keep: fn(&IssueView) -> bool,
) -> Vec<(&'a Claim, &'a IssueView)> {
    let by_id: HashMap<Uuid, _> = claimed.iter().map(|view| (view.id, view)).collect();
    claims
        .iter()
        .filter_map(|claim| Some((claim, *by_id.get(&claim.issue)?)))
        .filter(|(_, view)| keep(view))
        .collect()
}

/// `heading`, the first [`CLAIMS_SHOWN`] of `shown` in `text`, each named by
/// `named` with its expiry at `now_ms`, and each as `--json` gives it.
fn claims_section(
    text: &mut String,
    heading: &str,
    named: fn(&Claim) -> String,
    shown: &[(&Claim, &IssueView)],
    projects: &HashMap<ProjectId, Option<String>>,
    now_ms: i64,
) -> Vec<Value> {
    let _ = writeln!(text, "{heading}");
    let mut claim_items = Vec::with_capacity(shown.len());
    for &(claim, view) in shown {
        if claim_items.len() < CLAIMS_SHOWN {
            let _ = writeln!(
                text,
                "  {}  ({}, {})",
                line_within(view, TITLE_BYTES),
                named(claim),
                claim::expiry(claim, now_ms)
            );
        }
        let mut entry = item(view, projects);
        entry["claim"] = claim::claim_json(claim, now_ms);
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
    claim_items
}

/// A heading with the count of `total`, the first of them, and the command
/// that lists the rest: `words` is the heading, the line for none, and that
/// command.
fn section(text: &mut String, words: (&str, &str, &str), shown: &[IssueView], total: usize) {
    let (heading, none, more) = words;
    let _ = writeln!(text, "{heading} ({total}):");
    if total == 0 {
        let _ = writeln!(text, "  {none}");
    }
    for view in shown {
        let _ = writeln!(text, "  {}", line_within(view, TITLE_BYTES));
    }
    if shown.len() < total {
        let _ = writeln!(text, "  and {} more: {more}", total - shown.len());
    }
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
