//! `tm list`: the Issues a filter matches.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use tm_core::ProjectId;
use tm_core::filter::{Context, FilterError, parse};
use tm_core::scheme::Category;
use tm_store::{IssueView, Store, StoreError};
use uuid::Uuid;

use crate::error::CliError;
use crate::here::{self, Here};
use crate::identity;
use crate::output::{Output, printable};

/// With no filter, the open work: triage, todo and active.
const OPEN: &str = "-category:done,cancelled";

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
    let (total, views, projects) = here.store.read(|store| {
        let ids = find(here, &filter)?;
        let shown = &ids[..limit.map_or(ids.len(), |limit| limit.min(ids.len()))];
        let views = store.issues(shown)?;
        let projects = project_keys(store, &views)?;
        Ok::<_, CliError>((ids.len(), views, projects))
    })?;

    // Only the form printed is built: at thousands of rows, the other costs
    // as much as reading them.
    if json {
        let issues = views.iter().map(|view| Item::of(view, &projects)).collect();
        return Listed { issues, total }.output();
    }
    let mut text = String::new();
    for view in &views {
        let _ = writeln!(text, "{}", line(view));
    }
    if total == 0 {
        text.push_str("no Issues match\n");
    } else if views.len() < total {
        let _ = writeln!(text, "({} of {total} shown)", views.len());
    }
    Ok(Output::new(text, Value::Null))
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
    category: &'a Option<Category>,
    #[serde(serialize_with = "without_rank")]
    fields: &'a BTreeMap<String, Value>,
    id: Uuid,
    key: &'a Option<String>,
    project: Option<&'a Option<String>>,
    sets: &'a BTreeMap<String, BTreeMap<String, String>>,
    /// Only `tm ready` gives what each Issue would unblock.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) unblocks: Option<Vec<Value>>,
}

impl<'a> Item<'a> {
    pub(crate) fn of(
        view: &'a IssueView,
        projects: &'a HashMap<ProjectId, Option<String>>,
    ) -> Self {
        Self {
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

/// [`line`] with the printed title cut to at most `title_bytes`, on a
/// character boundary, ending in `…`.
pub(crate) fn line_within(view: &IssueView, title_bytes: usize) -> String {
    let field = |name: &str| view.fields.get(name).and_then(Value::as_str).unwrap_or("-");
    let mut title = printable(field("title"));
    if title.len() > title_bytes {
        let room = title_bytes.saturating_sub('…'.len_utf8());
        let end = title
            .char_indices()
            .map(|(at, c)| at + c.len_utf8())
            .take_while(|end| *end <= room)
            .last()
            .unwrap_or(0);
        title.truncate(end);
        title.push('…');
    }
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
}
