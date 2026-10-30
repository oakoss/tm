//! `tm shortlist`: named, ordered lists of what to do next, and the Actors
//! who work through them (ADR 0016).

use std::fmt::Write as _;

use clap::{ArgGroup, Args, Subcommand};
use serde_json::{Value, json};
use tm_core::{Actor, ProjectId};
use tm_store::{Member, PassedOver, Place, Shortlist, ShortlistEdit, StoreError};
use uuid::Uuid;

use crate::error::CliError;
use crate::here::{self, Here};
use crate::identity::{self, Identity};
use crate::list::{Item, line, project_keys};
use crate::output::{Output, printable};

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
pub(crate) struct Shortlists {
    /// Every list in the Project rather than only the ones you work through.
    #[arg(long)]
    all: bool,
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// Create a list worked through by the named Actors: an Agent's name,
    /// such as claude, or a human's as tm records it; @me is you.
    Create {
        name: String,
        #[arg(long = "actor", value_name = "NAME", required = true)]
        actors: Vec<String>,
        /// How many Issues it holds; 10 unless set.
        #[arg(long)]
        size: Option<u64>,
    },
    /// Put Issues on a list, in the order given, at its end unless placed;
    /// an Issue on another list leaves it.
    Add {
        /// The list's name, or its id.
        list: String,
        #[arg(value_name = "KEY", required = true)]
        issues: Vec<String>,
        #[command(flatten)]
        at: At,
    },
    /// Move an Issue within its list.
    #[command(group = ArgGroup::new("place").required(true).args(["top", "end", "before", "after"]))]
    Move {
        #[arg(value_name = "KEY")]
        issue: String,
        #[command(flatten)]
        at: At,
    },
    /// Take Issues off their lists.
    Remove {
        #[arg(value_name = "KEY", required = true)]
        issues: Vec<String>,
    },
    /// Rename a list, change its size, or add and remove its Actors.
    Edit {
        /// The list's name, or its id, as when two lists share a name.
        list: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        size: Option<u64>,
        #[arg(long = "actor", value_name = "NAME")]
        add: Vec<String>,
        #[arg(long = "remove-actor", value_name = "NAME")]
        remove: Vec<String>,
    },
    /// Delete a list, taking its Issues off; tm undo restores both.
    Delete {
        /// The list's name, or its id.
        list: String,
    },
}

/// Where Issues go; with none, the list's end.
#[derive(Args)]
#[group(multiple = false)]
struct At {
    /// At the top.
    #[arg(long)]
    top: bool,
    /// At the end.
    #[arg(long)]
    end: bool,
    /// Just before this member.
    #[arg(long, value_name = "KEY")]
    before: Option<String>,
    /// Just after this member.
    #[arg(long, value_name = "KEY")]
    after: Option<String>,
}

pub(crate) fn run(args: &Shortlists, json: bool) -> Result<Output, CliError> {
    match &args.action {
        None => {
            let mut here = here::read()?;
            let warnings = std::mem::take(&mut here.warnings);
            match show(&here, args.all, json) {
                Ok(mut output) => {
                    let mut all = warnings;
                    all.append(&mut output.warnings);
                    output.warnings = all;
                    Ok(output)
                }
                Err(err) => Err(err.warned(warnings)),
            }
        }
        Some(action) => here::write(|here| {
            let identity = identity(here)?;
            match write(here, action, &identity.actor) {
                Ok(output) => Ok((output, identity.warnings)),
                Err(err) => Err(err.warned(identity.warnings)),
            }
        }),
    }
}

fn identity(here: &Here) -> Result<Identity, CliError> {
    identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))
}

/// The lists the session works through, or every list with `all`, each with
/// its members in order.
fn show(here: &Here, all: bool, json: bool) -> Result<Output, CliError> {
    let project = here.project()?;
    let actor = if all { None } else { Some(identity(here)?) };
    let lists = match &actor {
        Some(identity) => here.store.shortlists_of(project, &identity.actor)?,
        None => here.store.shortlists(project)?,
    };
    let mut text = String::new();
    let mut shown = Vec::new();
    for list in &lists {
        let members = here.store.shortlist_view(list.id)?;
        render(&mut text, list, &members);
        if json {
            shown.push(list_json(here, list, &members)?);
        }
    }
    if lists.is_empty() {
        text = match &actor {
            Some(identity) => format!(
                "no Short-list names {}; tm shortlist --all shows every list\n",
                printable(identity.actor.name())
            ),
            None => {
                "no Short-lists in this Project; tm shortlist create NAME --actor NAME\n".into()
            }
        };
    }
    let mut output = Output::new(text, json!({ "shortlists": shown }));
    if let Some(identity) = actor {
        output.warnings = identity.warnings;
    }
    Ok(output)
}

fn render(text: &mut String, list: &Shortlist, members: &[Member]) {
    let name = printable(list.name.as_deref().unwrap_or("(unnamed)"));
    let actors = printable(&list.actors.join(", "));
    let count = members.len();
    let over = if over_size(list, count) {
        ", over size"
    } else {
        ""
    };
    let _ = writeln!(text, "{name}  ({actors})  {count} of {}{over}", list.size);
    for (position, member) in (1..).zip(members) {
        let _ = write!(text, "  {position}  {}", line(&member.issue));
        if !member.passed_over.is_empty() {
            let reasons: Vec<&str> = member.passed_over.iter().map(|r| reason(*r)).collect();
            let _ = write!(text, "  [{}]", reasons.join(", "));
        }
        text.push('\n');
    }
}

fn over_size(list: &Shortlist, count: usize) -> bool {
    u64::try_from(count).is_ok_and(|count| count > list.size)
}

pub(crate) const fn reason(passed: PassedOver) -> &'static str {
    match passed {
        PassedOver::Blocked => "blocked",
        PassedOver::Claimed => "claimed",
        PassedOver::Started => "started",
        PassedOver::Archived => "archived",
        PassedOver::Epic => "epic",
        PassedOver::Triage => "in triage",
        PassedOver::Waiting => "waiting",
    }
}

/// A list and its members; a member's position stands for its rank, which
/// readers never see (ADR 0016).
fn list_json(here: &Here, list: &Shortlist, members: &[Member]) -> Result<Value, CliError> {
    let views: Vec<_> = members.iter().map(|member| member.issue.clone()).collect();
    let projects = project_keys(&here.store, &views)?;
    let members: Vec<Value> = (1..)
        .zip(members)
        .map(|(position, member)| {
            let mut item = json!(Item::of(&member.issue, &projects));
            if let Some(fields) = item["fields"].as_object_mut() {
                fields.remove("shortlist");
            }
            item["position"] = json!(position);
            item["passed_over"] = json!(member.passed_over);
            item
        })
        .collect();
    Ok(json!({
        "id": list.id,
        "name": list.name,
        "actors": list.actors,
        "size": list.size,
        "count": members.len(),
        "over_size": over_size(list, members.len()),
        "members": members,
    }))
}

/// A write, then every list it touched as each now reads: the one written
/// first, then any an Issue left.
fn write(here: &mut Here, action: &Action, me: &Actor) -> Result<Output, CliError> {
    let project = here.project()?;
    let mut said = String::new();
    let (touched, ops) = match action {
        Action::Create { name, actors, size } => {
            let actors: Vec<String> = actors.iter().map(|who| named(who, me)).collect();
            let (id, ops) = here
                .store
                .create_shortlist(me, project, name, &actors, *size)?;
            (vec![id], ops)
        }
        Action::Add { list, issues, at } => {
            let (id, name) = by_name(here, project, list)?;
            let issues = resolve(here, issues)?;
            let at = place(here, at)?;
            let placed = renamed(here.store.shortlist_add(me, id, &issues, at), &name)?;
            let mut touched = vec![id];
            touched.extend(lists(&placed.from));
            (touched, placed.ops)
        }
        Action::Move { issue, at } => {
            let issue = here.issue(issue)?;
            let at = place(here, at)?;
            let placed = here.store.shortlist_move(me, issue, at)?;
            (lists(&placed.from), placed.ops)
        }
        Action::Remove { issues } => {
            let issues = resolve(here, issues)?;
            let placed = here.store.shortlist_remove(me, &issues)?;
            for (issue, list) in &placed.from {
                let key = here.store.issue(*issue)?.and_then(|view| view.key);
                let name = here.store.shortlist(*list)?.and_then(|list| list.name);
                let key = key.unwrap_or_else(|| issue.to_string());
                let name = printable(name.as_deref().unwrap_or("(unnamed)"));
                let _ = writeln!(said, "took {key} off {name}");
            }
            (lists(&placed.from), placed.ops)
        }
        Action::Edit {
            list,
            name,
            size,
            add,
            remove,
        } => {
            let (id, current) = by_name(here, project, list)?;
            let add: Vec<String> = add.iter().map(|who| named(who, me)).collect();
            let remove: Vec<String> = remove.iter().map(|who| named(who, me)).collect();
            let edit = ShortlistEdit {
                name: name.as_deref(),
                size: *size,
                add: &add,
                remove: &remove,
            };
            let ops = here.store.edit_shortlist(me, id, &edit);
            (vec![id], renamed(ops, &current)?)
        }
        Action::Delete { list } => {
            let (id, name) = by_name(here, project, list)?;
            let ops = here.store.delete_shortlist(me, id);
            (vec![id], renamed(ops, &name)?)
        }
    };
    report(here, &touched, &ops, said, me)
}

/// Each list in `from` once, in order.
fn lists(from: &[(Uuid, Uuid)]) -> Vec<Uuid> {
    let mut lists = Vec::new();
    for (_, list) in from {
        if !lists.contains(list) {
            lists.push(*list);
        }
    }
    lists
}

/// Each list a write touched, as it now reads, after what `said` holds.
fn report(
    here: &Here,
    touched: &[Uuid],
    ops: &[tm_core::Op],
    mut said: String,
    me: &Actor,
) -> Result<Output, CliError> {
    let mut lists = Vec::new();
    for id in touched {
        let list = here
            .store
            .shortlist(*id)?
            .ok_or(StoreError::UnknownShortlist(*id))?;
        let name = printable(list.name.as_deref().unwrap_or("(unnamed)"));
        if list.deleted {
            let note = "tm undo restores it with its Issues";
            let _ = writeln!(said, "deleted Short-list {name}; {note}");
            lists.push(json!({ "id": list.id, "name": list.name, "deleted": true }));
            continue;
        }
        let members = here.store.shortlist_view(*id)?;
        if ops.is_empty() {
            let _ = writeln!(said, "{name}: nothing changed");
        } else {
            render(&mut said, &list, &members);
        }
        lists.push(list_json(here, &list, &members)?);
    }
    let mut output = Output::new(said, json!({ "shortlists": lists }));
    if let Some(first) = ops.first() {
        return here.undoable(output, first.op_id, me);
    }
    output.data["batch"] = Value::Null;
    Ok(output)
}

fn resolve(here: &Here, keys: &[String]) -> Result<Vec<Uuid>, CliError> {
    let mut issues: Vec<Uuid> = Vec::with_capacity(keys.len());
    for key in keys {
        let issue = here.issue(key)?;
        if !issues.contains(&issue) {
            issues.push(issue);
        }
    }
    Ok(issues)
}

/// `@me` is the session's own Actor; any other name is as given.
fn named(who: &str, me: &Actor) -> String {
    if who.eq_ignore_ascii_case("@me") {
        me.name().to_owned()
    } else {
        who.to_owned()
    }
}

/// A live list by id, which reaches a list whose name a merge left shared
/// with another, else by name, since a name may itself look like an id.
fn by_name(here: &Here, project: ProjectId, given: &str) -> Result<(Uuid, String), CliError> {
    if let Ok(id) = Uuid::parse_str(given)
        && let Some(list) = here
            .store
            .shortlist(id)?
            .filter(|list| !list.deleted && list.project == project)
    {
        return Ok((id, list.name.unwrap_or_else(|| given.to_owned())));
    }
    let list = here.store.shortlist_named(project, given)?;
    Ok((list.id, list.name.unwrap_or_else(|| given.to_owned())))
}

/// A list deleted between finding it by name and writing to it reads as
/// not found by that name, rather than by an id the caller never typed.
fn renamed<T>(result: Result<T, StoreError>, name: &str) -> Result<T, CliError> {
    result.map_err(|err| match err {
        StoreError::UnknownShortlist(_) => StoreError::ShortlistNotFound(name.to_owned()).into(),
        other => other.into(),
    })
}

fn place(here: &Here, at: &At) -> Result<Place, CliError> {
    Ok(match (at.top, &at.before, &at.after) {
        (true, ..) => Place::Top,
        (_, Some(key), _) => Place::Before(here.issue(key)?),
        (_, _, Some(key)) => Place::After(here.issue(key)?),
        _ => Place::End,
    })
}
