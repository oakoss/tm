//! `tm ready`: the work an Agent can start now, and what each would unblock,
//! after the session's Short-lists (ADR 0016).

use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::Value;
use tm_core::{Actor, ProjectId};
use tm_store::{IssueView, Member, Shortlist, Store};
use uuid::Uuid;

use crate::error::CliError;
use crate::here::{self, Here};
use crate::identity;
use crate::list::{Item, Listed, Named, joined, line, named, project_keys, shortlists_of};
use crate::next;
use crate::output::Output;

pub(crate) fn run(limit: Option<usize>, json: bool) -> Result<Output, CliError> {
    let mut here = here::read()?;
    let project = mapped_or_every(&mut here);
    let lists = project.map_or_else(Vec::new, |project| session_lists(&mut here, project));
    let queue = match here.store.read(|store| Queue::read(store, limit, project)) {
        Ok(queue) => queue,
        Err(err) => return Err(err.warned(here.warnings)),
    };

    // Only the form printed is built, as for `tm list`.
    if json {
        let mut text = String::new();
        let shown = next::lists_section(&mut text, &lists, next::Room::WHOLE);
        let issues = queue
            .views
            .iter()
            .map(|view| {
                let mut item = Item::of(view, &queue.projects);
                item.unblocks = Some(queue.unblocks.get(&view.id).map_or(&[], Vec::as_slice));
                item
            })
            .collect();
        let listed = Listed {
            issues,
            shortlists: (!lists.is_empty()).then_some(shown),
            total: queue.total,
        };
        return match listed.output() {
            Ok(mut output) => {
                output.warnings = here.warnings;
                Ok(output)
            }
            Err(err) => Err(err.warned(here.warnings)),
        };
    }
    let mut output = Output::new(queue.text(&lists), Value::Null);
    output.warnings = here.warnings;
    Ok(output)
}

/// What `tm ready` prints, read from `store`: `lists` first, then the queue.
pub(crate) fn render(
    store: &Store,
    project: ProjectId,
    lists: &[(Shortlist, Vec<Member>)],
) -> Result<String, CliError> {
    store
        .read(|store| Queue::read(store, None, Some(project)))
        .map(|queue| queue.text(lists))
}

/// The ready queue, of one Project or every one, cut to a limit, with what
/// each Issue unblocks, its Project's Key and the Short-list it is on.
struct Queue {
    total: usize,
    views: Vec<IssueView>,
    unblocks: HashMap<Uuid, Vec<Named>>,
    projects: HashMap<ProjectId, Option<String>>,
    on: HashMap<Uuid, String>,
}

impl Queue {
    fn read(
        store: &Store,
        limit: Option<usize>,
        project: Option<ProjectId>,
    ) -> Result<Self, CliError> {
        let mut ready = store.ready_in(project)?;
        let total = ready.len();
        ready.truncate(limit.unwrap_or(total));
        let ids: Vec<Uuid> = ready.iter().map(|ready| ready.issue).collect();
        let views = store.issues(&ids)?;
        let unblocks = ready
            .iter()
            .map(|ready| (ready.issue, ready.unblocks.clone()))
            .collect();
        let unblocks = named(store, unblocks)?;
        let projects = project_keys(store, &views)?;
        let on = shortlists_of(store, &views)?;
        Ok(Self {
            total,
            views,
            unblocks,
            projects,
            on,
        })
    }

    fn text(&self, lists: &[(Shortlist, Vec<Member>)]) -> String {
        let mut text = String::new();
        next::lists_section(&mut text, lists, next::Room::WHOLE);
        let total = self.total;
        if !lists.is_empty() {
            let _ = writeln!(text, "Ready ({total}):");
        }
        for view in &self.views {
            let _ = write!(text, "{}", line(view));
            if let Some(found) = self
                .unblocks
                .get(&view.id)
                .filter(|found| !found.is_empty())
            {
                let _ = write!(text, "  (unblocks {})", joined(found));
            }
            if let Some(list) = self.on.get(&view.id) {
                let _ = write!(text, "  [on {list}]");
            }
            text.push('\n');
        }
        if total == 0 {
            text.push_str("nothing is ready\n");
        } else if self.views.len() < total {
            let _ = writeln!(text, "({} of {total} shown)", self.views.len());
        }
        text
    }
}

/// The Project this directory maps to, whose queue `tm ready` shows; with
/// none, every Project's, and [`unmapped`]'s warnings.
fn mapped_or_every(here: &mut Here) -> Option<ProjectId> {
    match here.project() {
        Ok(project) => Some(project),
        Err(err) => {
            here.warnings.extend(unmapped(&err));
            None
        }
    }
}

/// Why `tm ready` and `tm prime` show every Project's queue when the
/// directory maps to none, `err` saying why, and that a `TM_SHORTLIST`
/// binding goes unused, since Short-lists belong to a Project.
pub(crate) fn unmapped(err: &CliError) -> Vec<String> {
    let why = next::unread(err);
    let mut warnings = vec![format!("the ready queue spans every Project: {why}")];
    if std::env::var_os("TM_SHORTLIST").is_some() {
        warnings.push("TM_SHORTLIST is unused, as no Project is mapped".to_owned());
    }
    warnings
}

/// The session's lists, which `tm ready` leads with. Who the session is, which
/// may run git, is read only when its Project has a list or `TM_SHORTLIST`
/// names one; a session that cannot say, a binding that names no list, or a
/// list it cannot read is a warning, so the queue still prints.
fn session_lists(here: &mut Here, project: ProjectId) -> Vec<(Shortlist, Vec<Member>)> {
    let Here {
        store,
        found,
        warnings,
        ..
    } = here;
    let actor = || match identity::reader(identity::env, || identity::git_email(&found.dir)) {
        Ok(identity) => {
            warnings.extend(identity.warnings);
            Ok(identity.actor)
        }
        Err(message) => Err(format!("no identity, so no Short-lists: {message}")),
    };
    let read = lists_in(store, project, actor);
    read.unwrap_or_else(|warning| {
        here.warnings.push(warning);
        Vec::new()
    })
}

/// `actor`'s lists in `project` as `tm ready` leads with them, `actor` asked
/// only when the Project has a list or `TM_SHORTLIST` names one; why they
/// cannot be read.
pub(crate) fn lists_in(
    store: &Store,
    project: ProjectId,
    actor: impl FnOnce() -> Result<Actor, String>,
) -> Result<Vec<(Shortlist, Vec<Member>)>, String> {
    match store.shortlists(project) {
        Ok(lists) if lists.is_empty() && std::env::var_os("TM_SHORTLIST").is_none() => {
            Ok(Vec::new())
        }
        Ok(_) => {
            let actor = actor()?;
            next::session_lists(store, project, &actor).map_err(|err| next::unread(&err))
        }
        Err(err) => Err(format!("no Short-lists: {err}")),
    }
}
