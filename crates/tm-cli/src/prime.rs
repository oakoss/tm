//! `tm prime`: the context an Agent session starts from (design.md
//! "Interface"): who it writes as, the Scheme, its Claims, the work in
//! progress and the ready queue, kept small enough for a `SessionStart` hook.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::IsTerminal as _;

use serde_json::{Value, json};
use tm_core::filter::Context;
use tm_core::{Actor, ProjectId, Scheme};
use tm_store::{
    AgentClaims, Claim, CommentRole, CommentView, IssueView, Member, Settled, Shortlist, Store,
};
use uuid::Uuid;

use crate::claim;
use crate::error::CliError;
use crate::guide::HABITS;
use crate::here;
use crate::identity;
use crate::list::{item, line_within, project_keys};
use crate::next;
use crate::output::{Output, printable};

/// Ready Issues shown; the rest are counted, so 200 Issues stay near 1k
/// tokens.
pub(crate) const READY_SHOWN: usize = 15;

/// Fewer for a session with Short-lists, which `tm next` takes from first.
const READY_SHOWN_WITH_LISTS: usize = 3;

/// The lists' share of the budget, 1,000 bytes, which the one `tm next`
/// takes from, at most about 450 bytes, may pass.
const LISTS_ROOM: next::Room = next::Room {
    title_bytes: TITLE_BYTES,
    bytes: 1_000,
};

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
    let (actor, mapped, mut warnings) = context(&here);
    let lists = lists(&here, actor.as_ref(), mapped.as_ref(), &mut warnings);
    let project = mapped.map(|(_, key)| key);
    let ready_shown = if lists.is_empty() {
        READY_SHOWN
    } else {
        READY_SHOWN_WITH_LISTS
    };
    let read = here
        .store
        .read(|store| gather(store, actor.as_ref(), ready_shown, &mut warnings));
    let Read {
        claims,
        notes,
        waiting,
        claimed,
        active: (active, active_total),
        ready: (ready, total, passed),
        projects,
    } = read.map_err(|err| err.warned(warnings.clone()))?;
    let scheme = here.store.scheme();

    let mut text = String::new();
    if !json {
        for warning in &warnings {
            let _ = writeln!(text, "warning: {}", printable(warning));
        }
    }
    let who = printable(&actor.as_ref().map_or_else(|| "nobody".to_owned(), named));
    let key = project.as_deref().unwrap_or("-");
    let _ = writeln!(text, "tm: Project {key}, writing as {who}");
    let scheme = scheme_brief(&mut text, scheme);

    let session = actor.as_ref().and_then(Actor::session);
    let now_ms = claims.at_ms;
    let waiting_items = waiting_section(&mut text, actor.as_ref(), &waiting, &projects, now_ms);
    let sessions = by_session(claims, session);
    let claim_items = claims_sections(&mut text, (&sessions, &notes), &claimed, &projects, now_ms);
    let shortlists = next::lists_section(&mut text, &lists, LISTS_ROOM);
    section(
        &mut text,
        ("In progress", "none", "tm list category:active"),
        (&active, 0),
        active_total,
    );
    section(
        &mut text,
        ("Ready", "nothing is ready", "tm ready"),
        (&ready, passed),
        total,
    );
    let _ = writeln!(text, "Habits (tm guide has the rest):");
    for habit in HABITS {
        let _ = writeln!(text, "  {habit}");
    }

    let data = json!({
        "project": project,
        "scheme": scheme,
        "actor": actor.as_ref().map(actor_json),
        "waiting": waiting_items,
        "handed_claims": claim_items.handed,
        "claims": claim_items.held,
        "other_session_claims": claim_items.others,
        "lapsed_claims": claim_items.lapsed,
        "shortlists": shortlists,
        "in_progress": {
            "issues": active.iter().map(|view| item(view, &projects)).collect::<Vec<_>>(),
            "total": active_total,
        },
        "ready": {
            "issues": ready.iter().map(|view| item(view, &projects)).collect::<Vec<_>>(),
            "total": total,
            "on_shortlists": passed,
        },
        "habits": HABITS,
    });
    let mut output = Output::new(text, data);
    if json {
        output.warnings = warnings;
    }
    Ok(output)
}

/// What prime reads, in one read transaction.
struct Read {
    claims: AgentClaims,
    notes: HashMap<Uuid, Handoff>,
    waiting: Vec<(IssueView, Option<CommentView>)>,
    claimed: Vec<IssueView>,
    active: (Vec<IssueView>, usize),
    ready: (Vec<IssueView>, usize, usize),
    projects: HashMap<ProjectId, Option<String>>,
}

fn gather(
    store: &Store,
    actor: Option<&Actor>,
    ready_shown: usize,
    warnings: &mut Vec<String>,
) -> Result<Read, CliError> {
    let waiting = waiting(store, actor, warnings)?;
    let mut claims = match actor.filter(|actor| actor.is_agent()) {
        Some(actor) => store.claims_by(actor.name(), holder(actor))?,
        None => AgentClaims::default(),
    };
    // A question's Claim waits with it, and is shown there.
    let asked: HashSet<Uuid> = waiting.iter().map(|(view, _)| view.id).collect();
    claims.held.retain(|claim| !asked.contains(&claim.issue));
    let mut active = store.find("category:active", Context { me: None })?;
    let active_total = active.len();
    active.truncate(ACTIVE_SHOWN);
    let (ready, total, passed) = ready_queue(store, ready_shown)?;
    let ids: Vec<Uuid> = claims
        .held
        .iter()
        .chain(&claims.lapsed)
        .map(|claim| claim.issue)
        .collect();
    let claimed = store.issues(&ids)?;
    let (notes, unread) = handed_notes(store, &claims.held);
    warnings.extend(unread);
    let active = store.issues(&active)?;
    let ready = store.issues(&ready)?;
    let asked: Vec<IssueView> = waiting.iter().map(|(view, _)| view.clone()).collect();
    let projects = project_keys(
        store,
        &[claimed.clone(), active.clone(), ready.clone(), asked].concat(),
    )?;
    Ok(Read {
        claims,
        notes,
        waiting,
        claimed,
        active: (active, active_total),
        ready: (ready, total, passed),
        projects,
    })
}

/// The Issues whose question waits on the session's human, each with its
/// open question, oldest question first.
pub(crate) fn waiting(
    store: &Store,
    actor: Option<&Actor>,
    warnings: &mut Vec<String>,
) -> Result<Vec<(IssueView, Option<CommentView>)>, CliError> {
    let Some(actor) = actor else {
        return Ok(Vec::new());
    };
    let human = actor.on_behalf_of().unwrap_or_else(|| actor.name());
    let ids = store.find("waiting_on:@me", Context { me: Some(human) })?;
    let mut waiting = Vec::with_capacity(ids.len());
    for view in store.issues(&ids)? {
        let question = match store.open_question(view.id) {
            Ok(question) => question,
            Err(err) => {
                let name = view.key.clone().unwrap_or_else(|| view.id.to_string());
                warnings.push(format!("the question {name} waits on: {err}"));
                None
            }
        };
        waiting.push((view, question));
    }
    waiting.sort_by_key(|(_, question)| question.as_ref().map_or(i64::MAX, |q| q.utc_ms));
    Ok(waiting)
}

/// What waits on the session's human. An Agent's session is told to bring the
/// questions to its human, who is often there; a person's lists them.
fn waiting_section(
    text: &mut String,
    actor: Option<&Actor>,
    waiting: &[(IssueView, Option<CommentView>)],
    projects: &HashMap<ProjectId, Option<String>>,
    now_ms: i64,
) -> Value {
    let human = actor.map(|actor| actor.on_behalf_of().unwrap_or_else(|| actor.name()));
    if waiting.is_empty() {
        return json!({ "on": human, "issues": [] });
    }
    let count = waiting.len();
    let heading = match actor.filter(|actor| actor.is_agent()) {
        Some(_) => format!(
            "Waiting on {} ({count}): ask them, then tm answer KEY \"...\" records each reply:",
            printable(human.unwrap_or_default())
        ),
        None => format!("Waiting on you ({count}): tm answer KEY \"...\" replies:"),
    };
    let _ = writeln!(text, "{heading}");
    let mut issues = Vec::with_capacity(count);
    for (i, (view, question)) in waiting.iter().enumerate() {
        if i < CLAIMS_SHOWN {
            let _ = writeln!(text, "  {}", line_within(view, TITLE_BYTES));
            if let Some(question) = question {
                let first = question.body.lines().next().unwrap_or_default();
                let _ = writeln!(
                    text,
                    "    {} asked {} ago: {}",
                    printable(&question.actor),
                    crate::output::span(now_ms - question.utc_ms),
                    crate::list::cut(&printable(first), TITLE_BYTES * 2)
                );
                if !question.options.is_empty() {
                    let options = crate::show::numbered(&question.options);
                    let _ = writeln!(
                        text,
                        "      options: {}",
                        crate::list::cut(&options, TITLE_BYTES * 2)
                    );
                }
            }
        }
        let mut entry = item(view, projects);
        entry["question"] = question.as_ref().map_or(Value::Null, |question| {
            json!({
                "id": question.id,
                "actor": question.actor,
                "body": question.body,
                "options": question.options,
                "utc_ms": question.utc_ms,
            })
        });
        issues.push(entry);
    }
    if count > CLAIMS_SHOWN {
        let _ = writeln!(
            text,
            "  and {} more: tm list waiting_on:@me",
            count - CLAIMS_SHOWN
        );
    }
    json!({ "on": human, "issues": issues })
}

/// The first `shown` ready Issues, how many are ready and how many of those
/// are on a Short-list, which are passed over: a list holds them for its
/// Actors, and the session's own lists show above.
fn ready_queue(store: &Store, shown: usize) -> Result<(Vec<Uuid>, usize, usize), CliError> {
    let ready = store.ready()?;
    let total = ready.len();
    let listed: HashSet<Uuid> = store
        .find("has:shortlist", Context { me: None })?
        .into_iter()
        .collect();
    let open: Vec<Uuid> = ready
        .into_iter()
        .map(|ready| ready.issue)
        .filter(|issue| !listed.contains(issue))
        .collect();
    let passed = total - open.len();
    Ok((open.into_iter().take(shown).collect(), total, passed))
}

/// Who the session writes as and its Project, each `None` with a warning
/// rather than an error.
fn context(here: &here::Here) -> (Option<Actor>, Option<(ProjectId, String)>, Vec<String>) {
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
        Ok((id, key)) => Some((id, key.to_owned())),
        Err(err) => {
            warnings.push(err.message);
            None
        }
    };
    (actor, project, warnings)
}

fn actor_json(actor: &Actor) -> Value {
    json!({
        "name": actor.name(),
        "agent": actor.is_agent(),
        "on_behalf_of": actor.on_behalf_of(),
    })
}

/// The session's lists; one it cannot read is a warning, as prime never
/// fails a session start.
fn lists(
    here: &here::Here,
    actor: Option<&Actor>,
    mapped: Option<&(ProjectId, String)>,
    warnings: &mut Vec<String>,
) -> Vec<(Shortlist, Vec<Member>)> {
    let (Some(actor), Some((project, _))) = (actor, mapped) else {
        return Vec::new();
    };
    next::session_lists(here, *project, actor).unwrap_or_else(|err| {
        warnings.push(next::unread(&err));
        Vec::new()
    })
}

/// An Agent's Claims as one session sees them.
struct SessionClaims {
    handed: Vec<Claim>,
    mine: Vec<Claim>,
    others: Vec<Claim>,
    lapsed: Vec<Claim>,
}

/// `claims` sorted by whether `session` took them; one handed to the Agent
/// is no session's until one takes it. Another session's lapsed Claim, and a
/// lapsed handoff, are left out: they reach this session through tm ready.
fn by_session(claims: AgentClaims, session: Option<&str>) -> SessionClaims {
    let (handed, held): (Vec<Claim>, Vec<Claim>) =
        claims.held.into_iter().partition(|held| held.handed);
    let (mine, others) = held.into_iter().partition(|held| held.taken_in(session));
    let lapsed = claims
        .lapsed
        .into_iter()
        .filter(|lapsed| !lapsed.handed && lapsed.taken_in(session))
        .collect();
    SessionClaims {
        handed,
        mine,
        others,
        lapsed,
    }
}

/// Each Claim handed to the session's Agent, with who handed it on and the
/// first line of their note, as `--json` gives it with the whole note.
fn handed_section(
    text: &mut String,
    shown: &[(&Claim, &IssueView)],
    notes: &HashMap<Uuid, Handoff>,
    projects: &HashMap<ProjectId, Option<String>>,
    now_ms: i64,
) -> Vec<Value> {
    if shown.is_empty() {
        return Vec::new();
    }
    let _ = writeln!(text, "Handed to you (tm start KEY takes one):");
    let mut items = Vec::with_capacity(shown.len());
    for &(handed, view) in shown {
        let found = notes.get(&handed.id);
        let (by, note) = found.map_or(("", ""), |(by, note, _)| (by.as_str(), note.as_str()));
        if items.len() < CLAIMS_SHOWN {
            let from = if by.is_empty() {
                String::new()
            } else {
                format!("from {}, ", printable(by))
            };
            let _ = writeln!(
                text,
                "  {}  ({from}{})",
                line_within(view, TITLE_BYTES),
                claim::expiry(handed, now_ms)
            );
            if let Some(first) = note.lines().next().filter(|line| !line.is_empty()) {
                let _ = writeln!(text, "    {}", printable(&clip(first, TITLE_BYTES)));
            }
        }
        let mut entry = item(view, projects);
        entry["claim"] = claim::claim_json(handed, now_ms);
        // Null, not blank, when the note could not be read: a note is never blank.
        entry["from"] = json!(found.map(|(by, _, _)| by));
        entry["note"] = json!(found.map(|(_, note, _)| note));
        entry["answer"] = found.map_or(Value::Null, |(_, _, answer)| answer.clone());
        items.push(entry);
    }
    if items.len() > CLAIMS_SHOWN {
        let _ = writeln!(
            text,
            "  and {} more: tm --json prime lists them all",
            items.len() - CLAIMS_SHOWN
        );
    }
    items
}

/// A handed Claim's handoff: who handed it on, their note, and how the
/// question that kept it was settled, as JSON.
type Handoff = (String, String, Value);

/// Each of `held`'s handoff, by Claim id, and a warning for each that cannot
/// be read: prime never fails a session start.
fn handed_notes(store: &Store, held: &[Claim]) -> (HashMap<Uuid, Handoff>, Vec<String>) {
    let mut notes = HashMap::new();
    let mut unread = Vec::new();
    for handed in held.iter().filter(|held| held.handed) {
        match store.handed_by(handed.id) {
            Ok(Some((by, note))) => {
                // A Claim a question kept reads as how the question was settled.
                let (note, answer) = match store.answer_for(handed.id) {
                    Ok(Some(Settled { comment, choice })) => {
                        let withdrew = comment.role == Some(CommentRole::Withdrawn);
                        let how = if withdrew {
                            "withdrew the question"
                        } else {
                            "answered"
                        };
                        let answer = json!({
                            "by": comment.actor,
                            "role": comment.role,
                            "reply": comment.body,
                            "choice": choice,
                        });
                        (format!("{} {how}: {}", comment.actor, comment.body), answer)
                    }
                    Ok(None) => (note, Value::Null),
                    Err(err) => {
                        unread.push(format!(
                            "the answer to Claim {}'s question: {err}",
                            handed.id
                        ));
                        (note, Value::Null)
                    }
                };
                notes.insert(handed.id, (by, note, answer));
            }
            Ok(None) => {}
            Err(err) => unread.push(format!("the note handed with Claim {}: {err}", handed.id)),
        }
    }
    (notes, unread)
}

/// `text` cut to at most `bytes`, at a character boundary, marked when cut.
fn clip(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let mut end = bytes.saturating_sub(3);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// This session's held and takeable lapsed Claims, and those its other
/// sessions hold, each as `--json` gives it.
fn claims_sections(
    text: &mut String,
    (claims, notes): (&SessionClaims, &HashMap<Uuid, Handoff>),
    claimed: &[IssueView],
    projects: &HashMap<ProjectId, Option<String>>,
    now_ms: i64,
) -> ClaimItems {
    let handed = with_issues(&claims.handed, claimed, |_| true);
    let handed = handed_section(text, &handed, notes, projects, now_ms);
    let mine = with_issues(&claims.mine, claimed, |_| true);
    let others = with_issues(&claims.others, claimed, |_| true);
    let lapsed = with_issues(&claims.lapsed, claimed, claim::takeable);
    ClaimItems {
        handed,
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
    handed: Vec<Value>,
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
/// command. `passed` of `total` are on a Short-list and only counted.
fn section(
    text: &mut String,
    words: (&str, &str, &str),
    (shown, passed): (&[IssueView], usize),
    total: usize,
) {
    let (heading, none, more) = words;
    let _ = writeln!(text, "{heading} ({total}):");
    if total == 0 {
        let _ = writeln!(text, "  {none}");
    }
    for view in shown {
        let _ = writeln!(text, "  {}", line_within(view, TITLE_BYTES));
    }
    if passed > 0 {
        let _ = writeln!(
            text,
            "  {passed} on Short-lists, passed over: {more} marks them"
        );
    }
    let rest = total - shown.len() - passed;
    if rest > 0 {
        let _ = writeln!(text, "  and {rest} more: {more}");
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
