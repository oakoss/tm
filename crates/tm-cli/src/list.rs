//! `tm list`: the Issues a filter matches.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fmt::Write as _;

use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use tm_core::ProjectId;
use tm_core::filter::{Context, FilterError, OPEN, parse};
use tm_core::scheme::Category;
use tm_store::{IssueView, Store, StoreError};
use uuid::Uuid;

use crate::error::CliError;
use crate::here::{self, Here};
use crate::identity;
use crate::output::{Output, printable};

pub(crate) fn run(
    filter: &[String],
    limit: Option<usize>,
    all: bool,
    json: bool,
) -> Result<Output, CliError> {
    let here = here::read()?;
    match list(&here, filter, limit, all, json) {
        Ok(mut output) => {
            output.warnings = here.warnings;
            Ok(output)
        }
        Err(err) => Err(err.warned(here.warnings)),
    }
}

fn list(
    here: &Here,
    filter: &[String],
    limit: Option<usize>,
    all: bool,
    json: bool,
) -> Result<Output, CliError> {
    let filter = filter.join(" ");
    // A filter that only sorts still lists the open work, unless `--all`.
    // The default goes last so error spans still point into what was typed.
    let filter = match parse(&filter) {
        Ok(query) if query.terms.is_empty() && !all => format!("{filter} {OPEN}"),
        _ => filter,
    };
    let (total, views, projects, blockers) = here.store.read(|store| {
        let ids = find(here, &filter)?;
        let shown = &ids[..limit.map_or(ids.len(), |limit| limit.min(ids.len()))];
        let views = store.issues(shown)?;
        let projects = project_keys(store, &views)?;
        let blockers = named(store, store.open_blockers(shown)?)?;
        Ok::<_, CliError>((ids.len(), views, projects, blockers))
    })?;

    // Only the form printed is built: at thousands of rows, the other costs
    // as much as reading them.
    if json {
        let issues = views
            .iter()
            .map(|view| {
                let mut item = Item::of(view, &projects);
                item.blocked_by = blockers.get(&view.id).map(Vec::as_slice);
                item
            })
            .collect();
        let shortlists = None;
        return Listed {
            issues,
            shortlists,
            total,
        }
        .output();
    }
    let mut text = String::new();
    for view in &views {
        let _ = write!(text, "{}", line(view));
        if let Some(found) = blockers.get(&view.id) {
            let _ = write!(text, "  [blocked by {}]", joined(found));
        }
        text.push('\n');
    }
    if total == 0 {
        text.push_str("no Issues match\n");
    } else if views.len() < total {
        let _ = writeln!(text, "({} of {total} shown)", views.len());
    }
    Ok(Output::new(text, Value::Null))
}

/// `args` with `tm list`'s filter words moved after a `--`, in the order
/// typed, so clap reads a negated term such as `-type:spike` as a filter
/// word rather than a flag. Flags stay before it, each with the value `cli`
/// says it takes; other commands' arguments are returned as they are.
pub(crate) fn filter_last(cli: &mut clap::Command, args: Vec<OsString>) -> Vec<OsString> {
    let Some(at) = args
        .iter()
        .skip(1)
        .position(|arg| !arg.to_string_lossy().starts_with('-'))
        .map(|at| at + 1)
    else {
        return args;
    };
    if args[at] != "list" {
        return args;
    }
    cli.build();
    let Some(list) = cli.find_subcommand("list") else {
        return args;
    };
    let takes_value = |word: &str| {
        list.get_arguments().any(|arg| {
            let long = arg.get_long().map(|long| format!("--{long}"));
            let short = arg.get_short().map(|short| format!("-{short}"));
            arg.get_action().takes_values() && [long, short].contains(&Some(word.to_owned()))
        })
    };
    let negated = |word: &str| {
        word.strip_prefix('-')
            .is_some_and(|term| !term.starts_with('-') && term.contains(':'))
    };
    let mut rewritten = args[..=at].to_vec();
    let mut words = Vec::new();
    let mut rest = args[at + 1..].iter().peekable();
    while let Some(arg) = rest.next() {
        let word = arg.to_string_lossy();
        if word == "--" {
            words.extend(rest.by_ref().cloned());
        } else if word == "-" || negated(&word) || !word.starts_with('-') {
            words.push(arg.clone());
        } else {
            rewritten.push(arg.clone());
            if takes_value(&word) {
                let value = rest.next_if(|next| !negated(&next.to_string_lossy()));
                rewritten.extend(value.cloned());
            }
        }
    }
    rewritten.push("--".into());
    rewritten.extend(words);
    rewritten
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

/// An Issue a listed one names, such as what it unblocks or is blocked by.
#[derive(Serialize)]
pub(crate) struct Named {
    id: Uuid,
    key: Option<String>,
}

/// Each Issue's related Issues, by id and Key, reading each Issue once.
pub(crate) fn named(
    store: &Store,
    related: HashMap<Uuid, Vec<Uuid>>,
) -> Result<HashMap<Uuid, Vec<Named>>, CliError> {
    let ids: Vec<Uuid> = related
        .values()
        .flatten()
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let keys: HashMap<Uuid, Option<String>> = store
        .issues(&ids)?
        .into_iter()
        .map(|view| (view.id, view.key))
        .collect();
    Ok(related
        .into_iter()
        .map(|(issue, ids)| {
            let named = ids
                .into_iter()
                .map(|id| Named {
                    id,
                    key: keys.get(&id).cloned().flatten(),
                })
                .collect();
            (issue, named)
        })
        .collect())
}

pub(crate) fn joined(named: &[Named]) -> String {
    named
        .iter()
        .map(|named| named.key.clone().unwrap_or_else(|| named.id.to_string()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The key of each Project `views` belong to.
pub(crate) fn project_keys(
    store: &Store,
    views: &[IssueView],
) -> Result<HashMap<ProjectId, Option<String>>, CliError> {
    let mut projects = HashMap::new();
    for view in views {
        if let Entry::Vacant(entry) = projects.entry(view.project) {
            entry.insert(store.project_key(view.project)?);
        }
    }
    Ok(projects)
}

/// The name of the Short-list each of `views` is on, for those on one: `tm
/// next` passes them over for a session no list names (ADR 0016).
pub(crate) fn shortlists_of(
    store: &Store,
    views: &[IssueView],
) -> Result<HashMap<Uuid, String>, CliError> {
    let mut names: HashMap<&str, String> = HashMap::new();
    let mut on = HashMap::new();
    for view in views {
        let Some(list) = view.fields.get("shortlist").and_then(Value::as_str) else {
            continue;
        };
        let name = match names.entry(list) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                let read = match Uuid::parse_str(list) {
                    Ok(id) => store.shortlist(id)?.and_then(|list| list.name),
                    Err(_) => None,
                };
                let name = printable(read.as_deref().unwrap_or("(unnamed)"));
                entry.insert(name).clone()
            }
        };
        on.insert(view.id, name);
    }
    Ok(on)
}

/// An Issue as a list gives it: `show`'s shape without parent and Claim.
pub(crate) fn item(view: &IssueView, projects: &HashMap<ProjectId, Option<String>>) -> Value {
    let mut fields = view.fields.clone();
    fields.remove(RANK);
    json!({
        "id": view.id,
        "key": view.key,
        "project": projects.get(&view.project),
        "category": view.category,
        "fields": fields,
        "sets": view.sets,
    })
}

const RANK: &str = "shortlist_rank";

/// [`item`] serialized as it is written, for lists of thousands: its fields
/// are in the order a JSON value sorts its keys, so the bytes are the same.
#[derive(Serialize)]
pub(crate) struct Item<'a> {
    /// Only `tm list` gives the open Issues blocking each.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) blocked_by: Option<&'a [Named]>,
    category: &'a Option<Category>,
    #[serde(serialize_with = "without_rank")]
    fields: &'a BTreeMap<String, Value>,
    id: Uuid,
    key: &'a Option<String>,
    project: Option<&'a Option<String>>,
    sets: &'a BTreeMap<String, BTreeMap<String, String>>,
    /// Only `tm ready` gives what each Issue would unblock.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) unblocks: Option<&'a [Named]>,
}

impl<'a> Item<'a> {
    pub(crate) fn of(
        view: &'a IssueView,
        projects: &'a HashMap<ProjectId, Option<String>>,
    ) -> Self {
        Self {
            blocked_by: None,
            category: &view.category,
            fields: &view.fields,
            id: view.id,
            key: &view.key,
            project: projects.get(&view.project),
            sets: &view.sets,
            unblocks: None,
        }
    }
}

/// An Issue's fields less its Short-list rank, an ordering key readers never
/// see (ADR 0016).
#[allow(clippy::trivially_copy_pass_by_ref)]
fn without_rank<S: Serializer>(
    fields: &&BTreeMap<String, Value>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_map(fields.iter().filter(|(name, _)| *name != RANK))
}

/// A list's JSON data, its keys in sorted order as for [`Item`].
#[derive(Serialize)]
pub(crate) struct Listed<'a> {
    pub(crate) issues: Vec<Item<'a>>,
    /// Only `tm ready` leads with the session's Short-lists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) shortlists: Option<Vec<Value>>,
    pub(crate) total: usize,
}

impl Listed<'_> {
    pub(crate) fn output(&self) -> Result<Output, CliError> {
        serde_json::to_string(self)
            .map(Output::serialized)
            .map_err(|err| CliError::internal("serialize", err.to_string()))
    }
}

/// `KEY  status  priority  title`, the Key falling back to the id, on one
/// line whatever the title holds.
pub(crate) fn line(view: &IssueView) -> String {
    line_within(view, usize::MAX)
}

/// `text` cut to at most `bytes`, on a character boundary, ending in `…`.
pub(crate) fn cut(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let room = bytes.saturating_sub('…'.len_utf8());
    let end = text
        .char_indices()
        .map(|(at, c)| at + c.len_utf8())
        .take_while(|end| *end <= room)
        .last()
        .unwrap_or(0);
    format!("{}…", &text[..end])
}

/// [`line`] with the printed title cut to at most `title_bytes`, on a
/// character boundary, ending in `…`.
pub(crate) fn line_within(view: &IssueView, title_bytes: usize) -> String {
    let field = |name: &str| view.fields.get(name).and_then(Value::as_str).unwrap_or("-");
    let title = cut(&printable(field("title")), title_bytes);
    format!(
        "{}  {}  {}  {title}",
        view.key.clone().unwrap_or_else(|| view.id.to_string()),
        field("status"),
        field("priority"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_serializes_as_the_value_item_builds() {
        let project = ProjectId::new();
        let view = IssueView {
            id: Uuid::now_v7(),
            key: Some("TM-1".into()),
            project,
            category: Some(Category::Active),
            fields: BTreeMap::from([
                ("title".into(), json!("a \"quoted\" title")),
                ("created".into(), json!(1_792_206_249_000_i64)),
                ("shortlist_rank".into(), json!("V")),
            ]),
            sets: BTreeMap::from([(
                "labels".into(),
                BTreeMap::from([("cli".into(), "CLI".into())]),
            )]),
        };
        let projects = HashMap::from([(project, Some("TM".to_owned()))]);
        let written = serde_json::to_string(&Item::of(&view, &projects)).unwrap();
        assert_eq!(written, item(&view, &projects).to_string());
        assert!(!written.contains("shortlist_rank"), "{written}");
    }

    #[test]
    fn filter_words_go_after_dashes_in_order_and_flags_keep_their_values() {
        use clap::CommandFactory;
        let rewrite = |line: &str| {
            let args = line.split(' ').map(OsString::from).collect();
            let rewritten = filter_last(&mut crate::Cli::command(), args);
            rewritten
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(
            rewrite("tm list status:todo -type:spike --limit 2 priority:low --json"),
            "tm list --limit 2 --json -- status:todo -type:spike priority:low"
        );
        assert_eq!(
            rewrite("tm --json list -labels:bug --limit=2 --all"),
            "tm --json list --limit=2 --all -- -labels:bug"
        );
        assert_eq!(
            rewrite("tm list -h --limt 2 -x"),
            "tm list -h --limt -x -- 2",
            "a word without a colon stays a flag for clap to judge"
        );
        assert_eq!(
            rewrite("tm list -type:task -- -category:done --limit"),
            "tm list -- -type:task -category:done --limit",
            "what follows the typed -- stays filter"
        );
        assert_eq!(
            rewrite("tm list --limit -type:task"),
            "tm list --limit -- -type:task",
            "a flag's missing value is not taken from the filter"
        );
        assert_eq!(rewrite("tm list a:b - c:d"), "tm list -- a:b - c:d");
        assert_eq!(rewrite("tm show -type:task"), "tm show -type:task");
        assert_eq!(rewrite("tm --json"), "tm --json");
    }
}
