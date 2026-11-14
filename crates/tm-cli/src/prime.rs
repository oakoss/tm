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
use crate::covering;
use crate::error::CliError;
use crate::guide::HABITS;
use crate::here;
use crate::identity;
use crate::list::{item, line_within, project_keys};
use crate::mention;
use crate::next;
use crate::output::{Output, printable};

/// Ready Issues shown; the rest are counted, so 200 Issues stay near 1k
/// tokens.
pub(crate) const READY_SHOWN: usize = 15;

/// Fewer for a session with Short-lists, which `tm next` takes from first.
const READY_SHOWN_WITH_LISTS: usize = 3;

/// Unread Notifications `--json` lists; text shows the newest and counts
/// them, as the budget has no room for more.
const NOTIFICATIONS_SHOWN: usize = 5;

/// The files a Path Claim's line names before counting the rest.
const PATH_FILES_SHOWN: usize = 3;

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
    let memories = match &mapped {
        Some((project, _)) => here.store.memories(*project).unwrap_or_else(|err| {
            warnings.push(format!("this Project's memories: {err}"));
            Vec::new()
        }),
        None => Vec::new(),
    };
    let mapped_id = mapped.as_ref().map(|(project, _)| *project);
    let project = mapped.map(|(_, key)| key);
    let ready_shown = if lists.is_empty() {
        READY_SHOWN
    } else {
        READY_SHOWN_WITH_LISTS
    };
    let ready_of = (mapped_id, ready_shown);
    let read = here
        .store
        .read(|store| gather(store, actor.as_ref(), ready_of, &mut warnings));
    let Read {
        claims,
        notes,
        waiting,
        claimed,
        active: (active, active_total),
        ready: (ready, total, passed),
        projects,
    } = read.map_err(|err| err.warned(warnings.clone()))?;
    let resumes = resumes(&here.store, &waiting, &mut warnings);
    let names = asked_names(&here.store, &waiting, &notes)?;
    let scheme = here.store.scheme();
    let touching = path_claims(&here, actor.as_ref(), &mut warnings);
    let unread = notifications(&here.store, actor.as_ref(), &mut warnings);

    let mut text = String::new();
    if !json {
        for warning in &warnings {
            let _ = writeln!(text, "warning: {}", printable(warning));
        }
    }
    let who = printable(&actor.as_ref().map_or_else(|| "nobody".to_owned(), named));
    let key = project.as_deref().unwrap_or("-");
    let session = actor.as_ref().and_then(Actor::session);
    let in_session =
        session.map_or_else(String::new, |id| format!(" in session {}", printable(id)));
    let _ = writeln!(text, "tm: Project {key}, writing as {who}{in_session}");
    let scheme = scheme_brief(&mut text, scheme);

    let now_ms = claims.at_ms;
    let memories = memories_section(&mut text, &memories, crate::memory::now_ms());
    let asked = (actor.as_ref(), &resumes);
    let waiting_items = waiting_section(&mut text, asked, (&waiting, &names), &projects, now_ms);
    let sessions = by_session(claims, session);
    let claim_items = claims_sections(
        &mut text,
        (&sessions, &notes, &names),
        &claimed,
        &projects,
        now_ms,
    );
    let path_claims = path_claims_section(&mut text, &here, &touching, now_ms);
    let shortlists = next::lists_section(&mut text, &lists, LISTS_ROOM);
    queues(&mut text, (&active, active_total), (&ready, passed, total));
    let notifications = notifications_section(&mut text, unread);
    let _ = writeln!(text, "Habits (tm guide has the rest):");
    for habit in HABITS {
        let _ = writeln!(text, "  {habit}");
    }

    let data = json!({
        "project": project,
        "scheme": scheme,
        "actor": actor.as_ref().map(actor_json),
        "memories": memories,
        "waiting": waiting_items,
        "handed_claims": claim_items.handed,
        "claims": claim_items.held,
        "other_session_claims": claim_items.others,
        "lapsed_claims": claim_items.lapsed,
        "path_claims": path_claims,
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
        "notifications": notifications,
        "habits": HABITS,
    });
    let mut output = mention::named(&here.store, Output::new(text, data))?;
    if json {
        output.warnings = warnings;
    }
    Ok(output)
}

fn memories_section(text: &mut String, memories: &[tm_store::Memory], now_ms: i64) -> Value {
    let shown = &memories[..memories.len().min(crate::memory::PRIME_SHOWN)];
    if !shown.is_empty() {
        let _ = writeln!(text, "Remember (tm remember \"...\" adds one):");
        for memory in shown {
            let _ = writeln!(text, "  {}", crate::memory::line(memory, now_ms));
        }
        if memories.len() > shown.len() {
            let more = memories.len() - shown.len();
            let _ = writeln!(text, "  and {more} more: tm memories");
        }
    }
    json!({
        "memories": shown.iter().map(crate::memory::memory_json).collect::<Vec<_>>(),
        "total": memories.len(),
    })
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
    (project, ready_shown): (Option<ProjectId>, usize),
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
    let (ready, total, passed) = ready_queue(store, project, ready_shown)?;
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

/// A command that resumes a session, and why it may no longer work.
type Resume = (String, Option<&'static str>);

/// The command resuming the session that asked each waiting question, by the
/// question's id, when that session ran here; a warning for each that cannot
/// be read, as prime never fails a session start.
fn resumes(
    store: &Store,
    waiting: &[(IssueView, Option<CommentView>)],
    warnings: &mut Vec<String>,
) -> HashMap<Uuid, Resume> {
    let mut resumes = HashMap::new();
    for (view, question) in waiting {
        let Some(question) = question else { continue };
        match store.written_by_session(question.id) {
            Ok(Some((session, Some(place)))) => {
                if let Some(harness) = crate::session::harness(&place) {
                    let command = harness.resume(&crate::fields::quoted(&place.dir), &session);
                    resumes.insert(question.id, (command, harness.expiry()));
                }
            }
            Ok(_) => {}
            Err(err) => {
                let name = view.key.clone().unwrap_or_else(|| view.id.to_string());
                warnings.push(format!("the session that asked {name}: {err}"));
            }
        }
    }
    resumes
}

/// What waits on the session's human. An Agent's session is told to bring the
/// questions to its human, who is often there; a person's lists them.
/// How the mentions in the questions and handoff notes shown are named.
fn asked_names(
    store: &Store,
    waiting: &[(IssueView, Option<CommentView>)],
    notes: &HashMap<Uuid, Handoff>,
) -> Result<mention::Names, CliError> {
    let asked = waiting.iter().filter_map(|(_, asked)| asked.as_ref());
    let texts = asked
        .map(|asked| asked.body.as_str())
        .chain(notes.values().map(|(_, note, _)| note.as_str()));
    Ok(mention::Names::of(store, texts)?)
}

fn waiting_section(
    text: &mut String,
    (actor, resumes): (Option<&Actor>, &HashMap<Uuid, Resume>),
    (waiting, names): (&[(IssueView, Option<CommentView>)], &mention::Names),
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
    // Once each after the section, not under every question.
    let mut expiries = std::collections::BTreeSet::new();
    for (i, (view, question)) in waiting.iter().enumerate() {
        if i < CLAIMS_SHOWN {
            let _ = writeln!(text, "  {}", line_within(view, TITLE_BYTES));
            if let Some(question) = question {
                let body = names.shown(&question.body);
                let first = body.lines().next().unwrap_or_default();
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
                if let Some((command, expiry)) = resumes.get(&question.id) {
                    let _ = writeln!(text, "      the asking session resumes with: {command}");
                    expiries.extend(*expiry);
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
                "resume": resumes.get(&question.id).map(|(command, _)| command),
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
    for expiry in expiries {
        let _ = writeln!(text, "  {expiry}");
    }
    json!({ "on": human, "issues": issues })
}

/// The first `shown` ready Issues, how many are ready and how many of those
/// are on a Short-list, which are passed over: a list holds them for its
/// Actors, and the session's own lists show above.
fn ready_queue(
    store: &Store,
    project: Option<ProjectId>,
    shown: usize,
) -> Result<(Vec<Uuid>, usize, usize), CliError> {
    let ready = store.ready_in(project)?;
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
    let actor = match identity::writer_in(here.env(), &here.found.dir) {
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
            warnings.extend(crate::ready::unmapped(&err));
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
        "session": actor.session(),
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
    next::session_lists(&here.store, *project, actor).unwrap_or_else(|err| {
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

/// The files this checkout changes that other sessions' live Claims cover;
/// git runs only when such a Claim names paths or a branch.
fn path_claims(
    here: &here::Here,
    me: Option<&Actor>,
    warnings: &mut Vec<String>,
) -> Vec<covering::Overlap> {
    let claims = match here.store.claims() {
        Ok(claims) => claims,
        Err(err) => {
            warnings.push(format!("Path Claims: {err}"));
            return Vec::new();
        }
    };
    let others = covering::others(&claims, me);
    if others.is_empty() {
        return Vec::new();
    }
    let mut git = covering::Git::new(&here.found.dir);
    let files = git.changed();
    let check = covering::overlaps(&mut git, &others, &files);
    warnings.extend(
        check
            .unchecked
            .into_iter()
            .map(|why| format!("Path Claims: {why}")),
    );
    check.found
}

/// `Path Claims on your changes`, one line per Claim with the files it
/// covers; nothing when none does.
fn path_claims_section(
    text: &mut String,
    here: &here::Here,
    touching: &[covering::Overlap],
    now_ms: i64,
) -> Value {
    let mut by_claim: Vec<(&Claim, Vec<&covering::Overlap>)> = Vec::new();
    for overlap in touching {
        match by_claim
            .iter_mut()
            .find(|(claim, _)| claim.id == overlap.claim.id)
        {
            Some((_, files)) => files.push(overlap),
            None => by_claim.push((&overlap.claim, vec![overlap])),
        }
    }
    if by_claim.is_empty() {
        return json!([]);
    }
    let _ = writeln!(text, "Path Claims on your changes ({}):", by_claim.len());
    let mut items = Vec::with_capacity(by_claim.len());
    for (claim, files) in by_claim {
        let key = here
            .store
            .issue(claim.issue)
            .ok()
            .flatten()
            .and_then(|view| view.key);
        let shown = key.clone().unwrap_or_else(|| claim.issue.to_string());
        let names: Vec<String> = files
            .iter()
            .map(|overlap| printable(&overlap.file))
            .collect();
        let listed = names[..names.len().min(PATH_FILES_SHOWN)].join(", ");
        let more = match names.len().saturating_sub(PATH_FILES_SHOWN) {
            0 => String::new(),
            more => format!(", and {more} more"),
        };
        let _ = writeln!(
            text,
            "  {shown}  {} covers {listed}{more}",
            claim::holder(claim)
        );
        let files: Vec<Value> = files
            .iter()
            .map(|overlap| match &overlap.why {
                covering::Why::Path(path) => json!({ "file": overlap.file, "path": path }),
                covering::Why::Branch(branch) => {
                    json!({ "file": overlap.file, "branch": branch })
                }
            })
            .collect();
        items.push(json!({
            "issue": { "id": claim.issue, "key": key },
            "claim": claim::claim_json(claim, now_ms),
            "files": files,
        }));
    }
    Value::Array(items)
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
    (notes, names): (&HashMap<Uuid, Handoff>, &mention::Names),
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
            let note = names.shown(note);
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
    (claims, notes, names): (&SessionClaims, &HashMap<Uuid, Handoff>, &mention::Names),
    claimed: &[IssueView],
    projects: &HashMap<ProjectId, Option<String>>,
    now_ms: i64,
) -> ClaimItems {
    let handed = with_issues(&claims.handed, claimed, |_| true);
    let handed = handed_section(text, &handed, (notes, names), projects, now_ms);
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
/// The In progress and Ready queues.
fn queues(
    text: &mut String,
    (active, active_total): (&[IssueView], usize),
    (ready, passed, total): (&[IssueView], usize, usize),
) {
    let in_progress = ("In progress", "none", "tm list category:active");
    section(text, in_progress, (active, 0), active_total);
    section(
        text,
        ("Ready", "nothing is ready", "tm ready"),
        (ready, passed),
        total,
    );
}

/// How many Notifications are unread, the newest in brief, and the newest
/// few as JSON.
type Unread = (usize, Option<String>, Vec<Value>);

/// The session's unread Notifications; a read that fails warns, as prime
/// never fails a session start.
fn notifications(store: &Store, actor: Option<&Actor>, warnings: &mut Vec<String>) -> Unread {
    let Some(actor) = actor else {
        return (0, None, Vec::new());
    };
    let unread = store.notifications(actor, false).map_err(CliError::from);
    let shown = unread.and_then(|unread| {
        let first = &unread[..unread.len().min(NOTIFICATIONS_SHOWN)];
        let shown = crate::notifications::shown(store, actor, first)?;
        let newest = shown.briefs.into_iter().next();
        Ok((unread.len(), newest, shown.entries))
    });
    shown.unwrap_or_else(|err| {
        warnings.push(format!("your Notifications: {}", err.message));
        (0, None, Vec::new())
    })
}

/// A line counting the unread Notifications, with the newest in brief,
/// until they are marked read.
fn notifications_section(text: &mut String, (total, newest, entries): Unread) -> Value {
    if let Some(newest) = newest {
        let _ = writeln!(
            text,
            "Notifications ({total} unread, tm notifications lists them): {newest}"
        );
    }
    json!({ "notifications": entries, "total": total })
}

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
