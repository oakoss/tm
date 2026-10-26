//! `tm next`: take the next Issue from the session's Short-lists, or from
//! the Project's ready queue when it has none (ADR 0016).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::scheme::Category;
use tm_core::{Actor, ProjectId};
use tm_store::{Member, PassedOver, Shortlist, StoreError};
use uuid::Uuid;

use crate::claim::Lasts;
use crate::error::CliError;
use crate::here::{self, Here};
use crate::identity::{self, Identity};
use crate::output::{Output, printable};
use crate::shortlist::reason;
use crate::show;
use crate::start::{self, Started};

/// Binds a session to one of its Short-lists by name, as `TM_AGENT` names
/// its Agent.
const BOUND: &str = "TM_SHORTLIST";
const BOUND_FIX: &str = "unset TM_SHORTLIST, or set it to a list tm shortlist shows";

/// Where `tm next` takes from, each candidate with why it would be passed
/// over, as the take judges them.
enum Source {
    /// The session's lists, oldest first, each with its members in order.
    Lists(Vec<(Shortlist, Vec<Member>)>),
    /// For a session with no Short-lists: the Project's ready queue less
    /// Issues on a Short-list, and those, by Key and the list's name.
    Ready {
        project: String,
        members: Vec<Member>,
        listed: Vec<(Option<String>, Option<String>)>,
    },
}

impl Source {
    fn members(&self) -> impl Iterator<Item = &Member> {
        let (lists, ready) = match self {
            Self::Lists(lists) => (lists.as_slice(), None),
            Self::Ready { members, .. } => (&[][..], Some(members)),
        };
        lists
            .iter()
            .flat_map(|(_, members)| members)
            .chain(ready.into_iter().flatten())
    }

    fn candidates(&self) -> Vec<Uuid> {
        self.members().map(|member| member.issue.id).collect()
    }

    /// The Issue a take would choose, as read now.
    fn first(&self) -> Option<Uuid> {
        self.members()
            .find(|member| eligible(member))
            .map(|member| member.issue.id)
    }

    /// Where `issue` came from, as "on docs" or "in TM's ready queue".
    fn place(&self, issue: Uuid) -> String {
        match self {
            Self::Lists(lists) => lists
                .iter()
                .find(|(_, members)| members.iter().any(|m| m.issue.id == issue))
                .map_or_else(String::new, |(list, _)| format!("on {}", name(list))),
            Self::Ready { project, .. } => format!("in {project}'s ready queue"),
        }
    }

    const fn kind(&self) -> &'static str {
        match self {
            Self::Lists(_) => "shortlist",
            Self::Ready { .. } => "ready",
        }
    }
}

pub(crate) fn run(peek: bool, lasts: &Lasts) -> Result<Output, CliError> {
    if peek {
        return look();
    }
    here::write(|here| {
        let identity = identity(here)?;
        match take(here, &identity.actor, lasts) {
            Ok(output) => Ok((output, identity.warnings)),
            Err(err) => Err(err.warned(identity.warnings)),
        }
    })
}

fn identity(here: &Here) -> Result<Identity, CliError> {
    identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))
}

/// `--peek`: what a take would choose, without writing, so it works from a
/// sandbox's Snapshot.
fn look() -> Result<Output, CliError> {
    let mut here = here::read()?;
    let warnings = std::mem::take(&mut here.warnings);
    let looked = identity(&here).and_then(|identity| {
        let peeked = peek(&here, &identity.actor);
        match peeked {
            Ok(mut output) => {
                output.warnings = identity.warnings;
                Ok(output)
            }
            Err(err) => Err(err.warned(identity.warnings)),
        }
    });
    match looked {
        Ok(mut output) => {
            let mut all = warnings;
            all.append(&mut output.warnings);
            output.warnings = all;
            Ok(output)
        }
        Err(err) => Err(err.warned(warnings)),
    }
}

fn peek(here: &Here, actor: &Actor) -> Result<Output, CliError> {
    let from = source(here, actor)?;
    let Some(issue) = from.first() else {
        return nothing(here, actor, &from);
    };
    let mut shown = show::render(here, issue)?;
    let key = shown.data["key"].as_str().unwrap_or_default().to_owned();
    let line = format!("{key} is next {}; tm next takes it\n", from.place(issue));
    shown.text = line + &shown.text;
    shown.data["from"] = json!(from.kind());
    shown.data["taken"] = json!(false);
    Ok(shown)
}

fn take(here: &mut Here, actor: &Actor, lasts: &Lasts) -> Result<Output, CliError> {
    let from = source(here, actor)?;
    let ttl_ms = lasts.ttl_ms(here);
    let scheme = here.store.scheme().clone();
    let begin = |lineage: &_| start::begin(&scheme, actor, lineage);
    let Some(next) = here
        .store
        .take_next(actor, &from.candidates(), ttl_ms, begin)?
    else {
        // Read again: what passed every candidate over may have changed.
        return nothing(here, actor, &source(here, actor)?);
    };
    let place = from.place(next.issue);
    let started = Started {
        id: next.issue,
        claim: next.claim,
        held: None,
        ops: next.ops,
        begun: next.made,
    };
    let mut output = start::report(here, actor, &next.issue.to_string(), started)?;
    let key = output.data["key"].as_str().unwrap_or_default().to_owned();
    output.text = format!("{key} is next {place}\n{}", output.text);
    output.data["from"] = json!(from.kind());
    output.data["taken"] = json!(true);
    Ok(output)
}

/// The session's lists, or the Project's ready queue when it has none.
fn source(here: &Here, actor: &Actor) -> Result<Source, CliError> {
    let project = here.project()?;
    let lists = session_lists(here, project, actor)?;
    if lists.is_empty() {
        return ready(here, project);
    }
    Ok(Source::Lists(lists))
}

/// The session's lists in `project`, oldest first, each with its members,
/// or the one `TM_SHORTLIST` names, which never falls back to the others.
pub(crate) fn session_lists(
    here: &Here,
    project: ProjectId,
    actor: &Actor,
) -> Result<Vec<(Shortlist, Vec<Member>)>, CliError> {
    let mut lists = here.store.shortlists_of(project, actor)?;
    if let Some(bound) = bound()? {
        let named: Vec<Shortlist> = lists
            .iter()
            .filter(|list| {
                let name = list.name.as_deref();
                name.is_some_and(|name| name.eq_ignore_ascii_case(&bound))
            })
            .cloned()
            .collect();
        // A name a merge left on two of them is refused, as ADR 0016 has it.
        let [list] = named.as_slice() else {
            return Err(unbound(&bound, actor, &lists, named.len()));
        };
        lists = vec![list.clone()];
    }
    let mut read = Vec::with_capacity(lists.len());
    for list in lists {
        let members = here.store.shortlist_view(list.id)?;
        read.push((list, members));
    }
    Ok(read)
}

/// Lists shown at session start; `tm shortlist` shows the rest.
const LISTS_SHOWN: usize = 3;

/// Members shown of the list `tm next` takes from, from the one it takes;
/// one fewer of each other list.
const MEMBERS_SHOWN: usize = 3;

/// How the section is cut: titles and names to `title_bytes`, and lists to
/// `bytes` in all, the one `tm next` takes from counted first; `tm prime`
/// cuts to its budget, `tm ready` not at all.
#[derive(Clone, Copy)]
pub(crate) struct Room {
    pub(crate) title_bytes: usize,
    pub(crate) bytes: usize,
}

impl Room {
    pub(crate) const WHOLE: Self = Self {
        title_bytes: usize::MAX,
        bytes: usize::MAX,
    };
}

/// The session's lists as `tm prime` and `tm ready` lead with them: each
/// list's first Issue that can start and the few after it, members `tm next`
/// passes over marked with why, those above it counted by reason, and a
/// count of the rest, within `room`. Only the Issue `tm next` takes is
/// marked next, and its list always shows. Each list as `--json` gives it.
/// No lists, no section.
pub(crate) fn lists_section(
    text: &mut String,
    lists: &[(Shortlist, Vec<Member>)],
    room: Room,
) -> Vec<Value> {
    if lists.is_empty() {
        return Vec::new();
    }
    text.push_str("Short-lists (tm next takes the first that can start):\n");
    let taken = lists
        .iter()
        .position(|(_, members)| members.iter().any(eligible));
    let windows: Vec<(Option<usize>, Vec<&Member>)> = lists
        .iter()
        .enumerate()
        .map(|(at, (_, members))| {
            let first = members.iter().position(eligible);
            let size = MEMBERS_SHOWN - usize::from(taken != Some(at));
            let window = members.iter().skip(first.unwrap_or(0)).take(size).collect();
            (first, window)
        })
        .collect();
    let rendered: Vec<String> = lists
        .iter()
        .zip(&windows)
        .enumerate()
        .map(|(at, ((list, members), (first, window)))| {
            let mark = if taken == Some(at) { "next" } else { "then" };
            let mut one = String::new();
            write_list(&mut one, (list, members), *first, (window, mark), room);
            one
        })
        .collect();
    let printed = fit(&rendered, taken, room.bytes);
    for (at, one) in rendered.iter().enumerate() {
        if printed.contains(&at) {
            text.push_str(one);
        }
    }
    let mut shown = Vec::with_capacity(lists.len());
    for (at, ((list, members), (first, window))) in lists.iter().zip(&windows).enumerate() {
        let first = first.map(|at| &members[at].issue);
        shown.push(json!({
            "id": list.id,
            "name": list.name,
            "count": members.len(),
            "first": first.map(|issue| json!({ "id": issue.id, "key": issue.key })),
            "next": taken == Some(at),
            "members": window.iter().map(|member| json!({
                "id": member.issue.id,
                "key": member.issue.key,
                "passed_over": member.passed_over,
            })).collect::<Vec<_>>(),
        }));
    }
    let more = lists.len() - printed.len();
    if more > 0 {
        let lists = if more == 1 { "list" } else { "lists" };
        let _ = writeln!(text, "  and {more} more {lists}: tm shortlist");
    }
    shown
}

/// Which of `rendered` print: the one `taken` names, then the others in
/// order while they fit in `bytes` and number at most [`LISTS_SHOWN`].
fn fit(rendered: &[String], taken: Option<usize>, bytes: usize) -> Vec<usize> {
    let mut used = taken.map_or(0, |at| rendered[at].len());
    let mut printed: Vec<usize> = taken.into_iter().collect();
    for (at, one) in rendered.iter().enumerate() {
        if Some(at) == taken || printed.len() >= LISTS_SHOWN {
            continue;
        }
        if used.saturating_add(one.len()) > bytes {
            break;
        }
        used += one.len();
        printed.push(at);
    }
    printed
}

/// One list: its heading, the members above its first that can start
/// counted by reason, `window` from that member with it marked `mark`, and
/// how many follow.
fn write_list(
    text: &mut String,
    (list, members): (&Shortlist, &[Member]),
    first: Option<usize>,
    (window, mark): (&[&Member], &str),
    room: Room,
) {
    let title_bytes = room.title_bytes;
    // Half a title's room, so names cannot crowd out the Issues.
    let name = crate::list::cut(&name(list), title_bytes / 2);
    let count = members.len();
    let issues = if count == 1 { "Issue" } else { "Issues" };
    let Some(first) = first else {
        let why = "none can start now; tm shortlist shows why";
        let _ = if count == 0 {
            writeln!(text, "  {name}: no Issues")
        } else {
            writeln!(text, "  {name} ({count} {issues}): {why}")
        };
        return;
    };
    let _ = writeln!(text, "  {name} ({count} {issues})");
    if first > 0 {
        let above = crate::list::cut(&counted(&members[..first]), title_bytes / 2);
        let _ = if above.is_empty() {
            writeln!(text, "    {first} above it passed over")
        } else {
            writeln!(text, "    {first} above it passed over: {above}")
        };
    }
    for (n, member) in window.iter().enumerate() {
        let mark = if n == 0 { mark } else { "    " };
        let line = crate::list::line_within(&member.issue, title_bytes);
        let _ = write!(text, "    {mark}  {line}");
        let why: Vec<&str> = member.passed_over.iter().map(|why| reason(*why)).collect();
        // Cut lines name the first reason and count the rest, to keep a budget.
        let _ = match why.as_slice() {
            [] => Ok(()),
            [first, rest @ ..] if title_bytes < usize::MAX && !rest.is_empty() => {
                write!(text, "  [{first} +{}]", rest.len())
            }
            all => write!(text, "  [{}]", all.join(", ")),
        };
        text.push('\n');
    }
    let rest = count - first - window.len();
    if rest > 0 {
        let _ = writeln!(text, "    and {rest} more: tm shortlist");
    }
}

/// `members`' reasons counted, as "1 blocked, 2 started".
fn counted(members: &[Member]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for member in members {
        for why in &member.passed_over {
            *counts.entry(reason(*why)).or_default() += 1;
        }
    }
    let counted: Vec<String> = counts.iter().map(|(why, n)| format!("{n} {why}")).collect();
    counted.join(", ")
}

fn eligible(member: &Member) -> bool {
    member.passed_over.is_empty() && member.issue.category == Some(Category::Todo)
}

/// A list read that failed, as the warning `tm prime` and `tm ready` give
/// in its place, with its fix.
pub(crate) fn unread(err: &CliError) -> String {
    match &err.fix {
        Some(fix) => format!("{}; {fix}", err.message),
        None => err.message.clone(),
    }
}

/// The Project's ready queue, less Issues on a Short-list, which another
/// Actor works through.
fn ready(here: &Here, project: ProjectId) -> Result<Source, CliError> {
    let ready: Vec<Uuid> = here.store.ready()?.into_iter().map(|r| r.issue).collect();
    let members = here
        .store
        .eligibility(&ready)?
        .into_iter()
        .filter(|member| member.issue.project == project);
    let (listed, members): (Vec<Member>, Vec<Member>) = members.partition(|member| {
        let list = member.issue.fields.get("shortlist");
        list.and_then(Value::as_str).is_some()
    });
    let mut on_lists = Vec::with_capacity(listed.len());
    for member in listed {
        let list = member.issue.fields.get("shortlist").and_then(Value::as_str);
        let id = list.and_then(|id| Uuid::parse_str(id).ok());
        let name = match id {
            Some(id) => here.store.shortlist(id)?.and_then(|list| list.name),
            None => None,
        };
        on_lists.push((member.issue.key, name));
    }
    let key = here.store.project_key(project)?.unwrap_or_default();
    Ok(Source::Ready {
        project: printable(&key),
        members,
        listed: on_lists,
    })
}

/// The list `TM_SHORTLIST` names; a value that is set but names nothing is
/// refused, since falling back would take from lists it did not name.
fn bound() -> Result<Option<String>, CliError> {
    let Some(value) = std::env::var_os(BOUND) else {
        return Ok(None);
    };
    let invalid = |why: &str| {
        CliError::new("invalid_value", format!("{BOUND} {why}"))
            .with("field", BOUND)
            .fix(BOUND_FIX)
    };
    let Some(text) = value.to_str() else {
        return Err(invalid("is not valid UTF-8"));
    };
    let name = text.trim();
    if name.is_empty() {
        return Err(invalid("is set but names no list"));
    }
    Ok(Some(name.to_owned()))
}

/// `TM_SHORTLIST` naming none of the session's lists, or `matched` of them.
fn unbound(bound: &str, actor: &Actor, lists: &[Shortlist], matched: usize) -> CliError {
    let names: Vec<String> = lists.iter().map(name).collect();
    let theirs = if names.is_empty() {
        format!("{} works through none", printable(actor.name()))
    } else {
        format!("{}'s are {}", printable(actor.name()), names.join(", "))
    };
    let (code, what) = if matched > 1 {
        let shared = format!("{matched} of this session's Short-lists; rename one");
        ("shortlist_name_shared", shared)
    } else {
        ("not_found", "none of this session's Short-lists".to_owned())
    };
    let message = format!("{BOUND} names {}, {what}; {theirs}", printable(bound));
    CliError::new(code, message)
        .with("kind", "shortlist")
        .with("name", bound)
        .with("valid", names)
        .fix(BOUND_FIX)
}

/// Why nothing was taken: per list (or the ready queue) how many Issues
/// were passed over and for what, any this session already holds, and where
/// other work is.
fn nothing(here: &Here, actor: &Actor, from: &Source) -> Result<Output, CliError> {
    let mut text = String::new();
    if from.first().is_some() {
        text.push_str("the Issues changed while tm next ran; run it again\n");
    }
    let mut passed = Vec::new();
    let mut held = Vec::new();
    let mut groups: Vec<(String, Option<&Shortlist>, &[Member], usize)> = Vec::new();
    match from {
        Source::Lists(lists) => {
            for (list, members) in lists {
                groups.push((format!("on {}", name(list)), Some(list), members, 0));
            }
        }
        Source::Ready {
            project,
            members,
            listed,
        } => {
            let place = format!("in {project}'s ready queue");
            groups.push((place, None, members, listed.len()));
            for (key, list) in listed {
                passed.push(json!({
                    "key": key,
                    "shortlist": list,
                    "passed_over": ["on_shortlist"],
                }));
            }
        }
    }
    for (place, list, members, listed) in groups {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        if listed > 0 {
            counts.insert("on a Short-list", listed);
        }
        for member in members {
            for why in &member.passed_over {
                *counts.entry(reason(*why)).or_default() += 1;
            }
            if member.passed_over.contains(&PassedOver::Claimed) && mine(here, actor, member)? {
                held.push(member.issue.key.clone().unwrap_or_default());
            }
            passed.push(json!({
                "key": member.issue.key,
                "shortlist": list.and_then(|list| list.name.clone()),
                "passed_over": member.passed_over,
            }));
        }
        let _ = match (members.len() + listed, list) {
            (0, Some(list)) => writeln!(text, "{} has no Issues", name(list)),
            (0, None) => writeln!(text, "nothing is ready {place}"),
            (count, _) => {
                let counted: Vec<String> =
                    counts.iter().map(|(why, n)| format!("{n} {why}")).collect();
                let issues = if count == 1 { "Issue" } else { "Issues" };
                writeln!(
                    text,
                    "nothing {place} can be taken next; of its {count} {issues}: {}",
                    counted.join(", ")
                )
            }
        };
    }
    for key in &held {
        let _ = writeln!(text, "you already hold {key}; tm show {key}");
    }
    text.push_str(match from {
        Source::Lists(_) => "tm ready lists other work\n",
        Source::Ready { listed, .. } if !listed.is_empty() => {
            "tm shortlist --all shows the lists holding the rest\n"
        }
        Source::Ready { .. } => "tm list shows the open work\n",
    });
    let data = json!({
        "taken": false,
        "key": Value::Null,
        "from": from.kind(),
        "passed_over": passed,
        "held": held,
    });
    Ok(Output::new(text, data))
}

/// Whether this session holds `member`'s Claim.
fn mine(here: &Here, actor: &Actor, member: &Member) -> Result<bool, StoreError> {
    let state = here.store.claim_state(member.issue.id)?;
    Ok(state
        .as_ref()
        .and_then(|state| state.held())
        .is_some_and(|claim| claim.taken_by(actor)))
}

fn name(list: &Shortlist) -> String {
    printable(list.name.as_deref().unwrap_or("(unnamed)"))
}
