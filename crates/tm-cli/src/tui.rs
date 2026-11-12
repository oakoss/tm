//! `tm tui`: the screens over this directory's Workspace (ADR 0018).

use std::io::{self, IsTerminal};
use std::path::Path;
use std::process::ExitCode;

use tm_core::ProjectId;
use tm_store::view::View;
use tm_store::workspace::RepoFile;
use tm_store::{Store, StoreError};
use tm_tui::Seen;
use tm_tui::{Proposal, ViewEdit};
use uuid::Uuid;

use crate::error::CliError;
use crate::identity::Identity;
use crate::output;
use crate::{
    comment, edit, here, history, identity, instructions, moves, prompt, ready, show, undo, views,
    watch,
};

pub(crate) fn run(json: bool) -> ExitCode {
    match open(json) {
        Ok(tm_tui::Ended::Quit) => ExitCode::SUCCESS,
        // As a shell reports a process a signal ended.
        Ok(tm_tui::Ended::Signal(signal)) => {
            ExitCode::from(u8::try_from(128 + signal).unwrap_or(u8::MAX))
        }
        Err(err) => output::finish(json, Err(err), &[]),
    }
}

fn open(json: bool) -> Result<tm_tui::Ended, CliError> {
    if json {
        return Err(CliError::new(
            "usage",
            "tm tui draws screens, so it has no JSON form (--json or TM_JSON=1); \
             tm list, tm ready and tm show give a script the same",
        )
        .fix("tm list --json"));
    }
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        return Err(CliError::new(
            "not_a_terminal",
            "tm tui draws screens, so it needs a terminal for its input and output; \
             tm list, tm ready and tm show print the same for a script",
        )
        .fix("tm list"));
    }
    prompt::hold();
    let here = here::read()?;
    let (project, key) = here::mapped_project(&here.store, &here.found, &here.cwd)?;
    let mut place = key.to_owned();
    if here.snapshot.is_some() {
        place.push_str(" (read-only Snapshot)");
    }
    let (me, reader) = session(identity::env, || identity::git_email(&here.found.dir));
    // A Snapshot is opened immutable and a replaced store file is another
    // file, so the list's reader is opened again on each change, and first
    // after the watch starts, so no change between the two is missed.
    let snapshot = here.snapshot.is_some();
    let workspace = here.workspace;
    let open = || {
        if snapshot {
            Store::open_snapshot(&workspace.snapshot_path(), workspace.id)
        } else {
            Store::open_existing(&workspace.store_path(), workspace.id)
        }
    };
    let mut watch = watch::watch()?;
    let store = open()?;
    let me = me.as_deref().map_err(String::as_str);
    let mut pending = false;
    let agent = identity::agent_detected(identity::env);
    let edits = views::editable(&here.found.path, snapshot, agent);
    let published = snapshot.then(|| workspace.snapshot_path());
    let actions = Cli {
        me,
        project,
        reader: reader.as_ref().map_err(String::as_str),
        repo: Some((&here.found.path, key)),
        edits,
        snapshot: published.as_deref(),
    };
    tm_tui::run(place, store, &actions, |store| {
        let (opened, seen) = refresh(watch.changed(), &mut pending, open, || {
            jiff::Zoned::now().strftime("%H:%M:%S").to_string()
        })?;
        if let Some(opened) = opened {
            *store = opened;
        }
        Ok(seen)
    })
    .map_err(|ended| match ended {
        Ended::Terminal(err) => CliError::new("io", format!("the terminal: {err}")),
        Ended::Store(err) => err,
    })
}

/// Who `@me` is, as `tm list` reads it, and the session as `tm ready` reads
/// it, which for an Agent needs no email; each kept with why it cannot be
/// said.
fn session(
    var: impl Fn(&str) -> Option<identity::Var> + Copy,
    email: impl Fn() -> Result<String, String>,
) -> (Result<String, String>, Result<Identity, String>) {
    let me = identity::resolve(var, &email).map(|identity| {
        let actor = identity.actor;
        actor
            .on_behalf_of()
            .unwrap_or_else(|| actor.name())
            .to_owned()
    });
    (me, identity::reader(var, email))
}

/// The screens' text from the CLI's own commands, `me` the person at the
/// terminal or why no one can be named, `reader` the session as `tm ready`
/// reads it or why it cannot, `project` the one this directory maps to, and
/// `repo` the repo file holding its Views with that Project's key.
struct Cli<'a> {
    me: Result<&'a str, &'a str>,
    project: ProjectId,
    reader: Result<&'a Identity, &'a str>,
    repo: Option<(&'a Path, &'a str)>,
    /// Whether the person here may change that file's Views, or why not.
    edits: Result<(), String>,
    /// The Snapshot read in place of a store this process may not open.
    snapshot: Option<&'a Path>,
}

impl tm_tui::Actions for Cli<'_> {
    /// As `here::read` warns, but what did not apply first: the row shows
    /// one warning, and the title already says a Snapshot is read.
    fn warnings(&self, store: &Store) -> Vec<String> {
        let mut warnings = Vec::new();
        match here::unapplied(store) {
            Ok(unapplied) => warnings.extend(unapplied),
            Err(err) => warnings.push(format!(
                "cannot count the logged operations that did not apply: {err}"
            )),
        }
        if let Some(path) = self.snapshot {
            warnings.push(
                match std::fs::metadata(path).and_then(|meta| meta.modified()) {
                    Ok(published) => here::snapshot_read(published),
                    Err(err) => format!("{}: read as the Snapshot, {err}", path.display()),
                },
            );
        }
        warnings
    }

    fn may_edit_views(&self) -> Result<(), String> {
        self.edits.clone()
    }

    fn propose(&self, edit: &ViewEdit) -> Result<Proposal, String> {
        self.edits.clone()?;
        let (path, _) = self.repo.ok_or("no repo file holds the Views")?;
        let file = path.display().to_string();
        let before = std::fs::read_to_string(path).map_err(|err| format!("{file}: {err}"))?;
        let after = views::changed(&before, edit, &file)?;
        let shown = path.file_name().map_or(file, |name| {
            let parent = path.parent().and_then(Path::file_name);
            parent.map_or_else(
                || name.to_string_lossy().into_owned(),
                |dir| format!("{}/{}", dir.to_string_lossy(), name.to_string_lossy()),
            )
        });
        Ok(Proposal {
            diff: crate::diff::unified(&shown, Some(&before), &after),
            before,
            after,
        })
    }

    fn save(&self, proposal: &Proposal) -> Result<String, String> {
        self.edits.clone()?;
        let (path, _) = self.repo.ok_or("no repo file holds the Views")?;
        let file = path.display().to_string();
        views::writable(path, &file)?;
        // Another hand may have edited the file while the diff was shown.
        if std::fs::read_to_string(path).ok().as_deref() != Some(proposal.before.as_str()) {
            return Err(format!(
                "{file} changed since the diff was shown; press v and make the change again"
            ));
        }
        instructions::write(path, &proposal.after, false)
            .map_err(|err| format!("writing {file} failed: {err}"))?;
        Ok(format!("saved to {file}"))
    }

    fn me(&self) -> Result<&str, &str> {
        self.me
    }

    fn project(&self) -> Option<ProjectId> {
        Some(self.project)
    }

    fn views(&self) -> Result<Vec<View>, String> {
        let Some((path, key)) = self.repo else {
            return Ok(Vec::new());
        };
        let file = std::fs::read_to_string(path)
            .map_err(|err| err.to_string())
            .and_then(|text| RepoFile::parse(&text))
            .map_err(|why| format!("{}: {why}", path.display()))?;
        Ok(file
            .views
            .into_iter()
            .filter(|view| view.shows_in(key))
            .collect())
    }

    fn issue(&self, store: &Store, id: Uuid) -> Result<tm_tui::Issue, String> {
        let me = || self.me.ok().map(str::to_owned);
        let shown = show::render_with(store, id, show::Comments::All, None);
        let latest = Some(tm_tui::LATEST_WRITES);
        let written = history::render(store, id, latest, false, &me);
        let (shown, written) = shown
            .and_then(|shown| Ok((shown, written?)))
            .map_err(|err| err.message)?;
        tm_tui::Issue::read(shown.data, written.data)
    }

    /// `tm ready`'s warnings, which it prints on stderr, lead the text.
    fn ready(&self, store: &Store) -> Result<String, String> {
        let mut warnings = Vec::new();
        let actor = || match self.reader {
            Ok(identity) => {
                warnings.extend(identity.warnings.iter().cloned());
                Ok(identity.actor.clone())
            }
            Err(why) => Err(format!("no identity, so no Short-lists: {why}")),
        };
        let lists = ready::lists_in(store, self.project, actor).unwrap_or_else(|warning| {
            warnings.push(warning);
            Vec::new()
        });
        let text = ready::render(store, self.project, &lists).map_err(|err| err.message)?;
        let mut lead = String::new();
        for warning in warnings {
            lead.push_str("warning: ");
            lead.push_str(&warning);
            lead.push('\n');
        }
        Ok(lead + &text)
    }

    fn write(&self, write: &tm_tui::Write) -> Result<tm_tui::Written, String> {
        let output = match write {
            tm_tui::Write::Move {
                key,
                status,
                version,
            } => moves::run(&moves::Move::to(key, status, if_version(*version))),
            tm_tui::Write::Set {
                key,
                field,
                value,
                version,
            } => edit::run(&edit::Edit::field(
                key,
                field,
                value.as_deref(),
                if_version(*version),
            )),
            tm_tui::Write::Comment { key, body } => comment::add(key, body),
        };
        output.map(|output| written(&output)).map_err(refusal)
    }

    fn undo(&self, batch: Uuid) -> Result<tm_tui::Written, String> {
        let output = undo::run(Some(&batch.to_string()), false, false).map_err(refusal)?;
        // The first line names the batch; the ones after say what reverted.
        let reverted: Vec<&str> = output.text.lines().skip(1).map(str::trim).collect();
        let said = if reverted.is_empty() {
            output.text.lines().next().unwrap_or_default().to_owned()
        } else {
            format!("undid {}", reverted.join("; "))
        };
        Ok(tm_tui::Written {
            said,
            batch: None,
            warnings: output.warnings,
        })
    }
}

/// A refused write as the status line says it: the CLI's message, then the
/// command that fixes it when the CLI gives one.
fn refusal(err: CliError) -> String {
    match err.fix {
        Some(fix) => format!("{} — try: {fix}", err.message),
        None => err.message,
    }
}

/// The first line the CLI printed for a write, and the batch it named.
fn written(output: &output::Output) -> tm_tui::Written {
    let said = output.text.lines().next().unwrap_or_default().to_owned();
    let batch = output.data["batch"]
        .as_str()
        .and_then(|batch| batch.parse().ok());
    tm_tui::Written {
        said,
        batch,
        warnings: output.warnings.clone(),
    }
}

/// A version the store read, as `--if-version` takes it.
fn if_version(version: i64) -> Option<u64> {
    u64::try_from(version).ok()
}

/// A look at the store: on a change the list's reader is opened again, and a
/// busy store leaves that `pending` for the next look, since the Watch has
/// already reported the change.
fn refresh(
    changed: Result<bool, StoreError>,
    pending: &mut bool,
    open: impl FnOnce() -> Result<Store, StoreError>,
    now: impl FnOnce() -> String,
) -> Result<(Option<Store>, Seen), Ended> {
    if !(*pending || matches!(changed, Ok(true))) {
        return seen(changed, now).map(|seen| (None, seen));
    }
    match open() {
        Ok(opened) => {
            *pending = false;
            // The reopen read the change, though the Watch may be busy now.
            let changed = match changed {
                Err(StoreError::Busy(_)) => Ok(true),
                other => other.map(|_| true),
            };
            seen(changed, now).map(|seen| (Some(opened), seen))
        }
        Err(StoreError::Busy(_)) => {
            *pending = true;
            Ok((None, Seen::Busy))
        }
        Err(err) => Err(Ended::Store(err.into())),
    }
}

/// What a look at the store shows: a busy store waits for the next look,
/// any other error ends the TUI.
fn seen(changed: Result<bool, StoreError>, now: impl FnOnce() -> String) -> Result<Seen, Ended> {
    match changed {
        Ok(true) => Ok(Seen::Changed(now())),
        Ok(false) => Ok(Seen::Nothing),
        Err(StoreError::Busy(_)) => Ok(Seen::Busy),
        Err(err) => Err(Ended::Store(err.into())),
    }
}

enum Ended {
    Terminal(io::Error),
    Store(CliError),
}

impl From<io::Error> for Ended {
    fn from(err: io::Error) -> Self {
        Self::Terminal(err)
    }
}

#[cfg(test)]
mod tests {
    use tm_core::{Actor, Change};
    use tm_store::Draft;
    use tm_tui::Actions as _;

    use super::*;

    #[test]
    fn the_issue_screen_reads_the_version_moves_and_lapsed_claim_tm_show_reports() {
        let dir = std::env::temp_dir().join(format!("tm-issue-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = Store::open(&dir.join("store.db")).unwrap();
        let jace = Actor::human("jace");
        let (project, _) = store.create_project(&jace, "TM", "Core").unwrap();
        let (id, _) = store.create_issue(&jace, project, "task", "alpha").unwrap();
        let claude = Actor::agent("claude", "jace");
        store.take(&claude, id, Some(1)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let cli = Cli {
            me: Ok("jace"),
            project,
            reader: Err("unused"),
            repo: None,
            edits: Ok(()),
            snapshot: None,
        };
        assert_eq!(cli.project(), Some(project));
        let shown = cli.issue(&store, id).unwrap().shown;
        let read = store.issue_with_version(id).unwrap().unwrap().1;
        assert_eq!(shown.version, read);
        let moves: Vec<&str> = shown.moves_to.iter().map(|to| to.id.as_str()).collect();
        assert!(
            moves.contains(&"done") && !moves.contains(&"todo"),
            "{moves:?}"
        );
        assert!(shown.claim.is_none(), "{shown:?}");
        let lapsed = shown.lapsed_claim.as_ref().unwrap();
        assert_eq!(
            (lapsed.agent.as_str(), lapsed.handed),
            ("claude", false),
            "{lapsed:?}"
        );
        assert!(lapsed.expires_ms.is_some(), "{lapsed:?}");
    }

    #[test]
    fn the_issue_screen_reads_show_and_history_json_as_they_are() {
        let dir = std::env::temp_dir().join(format!("tm-issue-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = Store::open(&dir.join("store.db")).unwrap();
        let jace = Actor::human("jace");
        let (project, _) = store.create_project(&jace, "TM", "Core").unwrap();
        let (parent, _) = store.create_issue(&jace, project, "epic", "whole").unwrap();
        let (id, _) = store.create_issue(&jace, project, "task", "alpha").unwrap();
        let (other, _) = store.create_issue(&jace, project, "task", "beta").unwrap();
        for (subject, change) in [
            (
                id,
                Change::SetField {
                    field: "parent".into(),
                    label: "Parent".into(),
                    value: parent.to_string().into(),
                    value_label: None,
                },
            ),
            (
                id,
                Change::Comment {
                    body: "**done** soon".into(),
                },
            ),
        ] {
            let draft = Draft {
                actor: jace.clone(),
                project,
                subject,
                change,
            };
            store.append(draft).unwrap();
        }
        store.link(&jace, "blocks", id, other).unwrap();
        let said = |change| Draft {
            actor: jace.clone(),
            project,
            subject: id,
            change,
        };
        let oops = store
            .append(said(Change::Comment {
                body: "oops".into(),
            }))
            .unwrap();
        let retract = Change::Retract {
            comment: oops.op_id,
            retracted: true,
        };
        store.append(said(retract)).unwrap();
        let cli = Cli {
            me: Ok("jace"),
            project,
            reader: Err("unused"),
            repo: None,
            edits: Ok(()),
            snapshot: None,
        };
        let issue = cli.issue(&store, id).unwrap();
        let shown = &issue.shown;
        assert_eq!(shown.key.as_deref(), Some("TM-2"));
        assert_eq!(shown.fields["title"], "alpha");
        assert_eq!(shown.parent.as_ref().map(|parent| parent.id), Some(parent));
        let links: Vec<_> = shown
            .links
            .iter()
            .map(|link| (link.name.as_str(), link.other.id))
            .collect();
        assert_eq!(links, [("blocks", other)]);
        assert_eq!(shown.comments[0].body.as_deref(), Some("**done** soon"));
        let retracted = &shown.comments[1];
        assert!(
            retracted.retracted && retracted.body.is_none(),
            "{retracted:?}"
        );
        let history = &issue.history;
        assert_eq!(history.writes.len(), tm_tui::LATEST_WRITES, "{history:?}");
        assert!(history.total > history.writes.len(), "{history:?}");
        let linked = history.writes.iter().flat_map(|write| &write.changes);
        assert!(
            linked.clone().any(|change| change.line.contains("blocks")),
            "{history:?}"
        );
        assert!(
            cli.issue(&store, Uuid::now_v7())
                .unwrap_err()
                .contains("does not exist")
        );
    }

    #[test]
    fn the_views_are_the_repo_files_for_this_project_read_afresh() {
        let dir = std::env::temp_dir().join(format!("tm-views-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tm.toml");
        let head = format!("workspace = \"{}\"\nproject = \"TM\"\n", Uuid::now_v7());
        let views = "[[views]]\nname = \"Mine\"\n\n[[views]]\nname = \"Web\"\nproject = \"web\"\n";
        std::fs::write(&path, format!("{head}{views}")).unwrap();
        let cli = |key| Cli {
            me: Err("unused"),
            project: ProjectId::new(),
            reader: Err("unused"),
            repo: Some((&path, key)),
            edits: Ok(()),
            snapshot: None,
        };
        let names = |key| -> Vec<String> {
            let views = cli(key).views().unwrap();
            views.into_iter().map(|view| view.name).collect()
        };
        assert_eq!(names("TM"), ["Mine"]);
        assert_eq!(names("WEB"), ["Mine", "Web"]);
        std::fs::write(&path, format!("{head}[[views]]\nname = \"x\"\nsort = 1\n")).unwrap();
        let why = cli("TM").views().unwrap_err();
        assert!(why.contains("tm.toml") && why.contains("sort"), "{why}");
    }

    #[test]
    fn a_view_change_is_shown_as_a_diff_then_saved_unless_the_file_moved_on() {
        let dir = std::env::temp_dir().join(format!("tm-view-save-{}", Uuid::now_v7()));
        std::fs::create_dir_all(dir.join(".config")).unwrap();
        let path = dir.join(".config/tm.toml");
        let text = format!(
            "workspace = \"{}\"\nproject = \"TM\"\n\n# kept\n[[views]]\nname = \"Mine\"\n",
            Uuid::now_v7()
        );
        std::fs::write(&path, &text).unwrap();
        let cli = Cli {
            me: Err("unused"),
            project: ProjectId::new(),
            reader: Err("unused"),
            repo: Some((&path, "TM")),
            edits: Ok(()),
            snapshot: None,
        };
        let mine = cli.views().unwrap().remove(0);
        let ours = View {
            name: "Ours".into(),
            ..mine.clone()
        };
        let edit = ViewEdit {
            was: Some(mine),
            becomes: Some(ours),
        };
        let proposal = cli.propose(&edit).unwrap();
        assert!(
            proposal.diff.contains("a/.config/tm.toml"),
            "{}",
            proposal.diff
        );
        assert!(
            proposal.diff.contains("-name = \"Mine\"\n+name = \"Ours\""),
            "{}",
            proposal.diff
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            text,
            "not yet written"
        );

        std::fs::write(&path, format!("{text}# a hand edit\n")).unwrap();
        let why = cli.save(&proposal).unwrap_err();
        assert!(why.contains("changed since the diff was shown"), "{why}");
        std::fs::write(&path, &text).unwrap();
        cli.save(&proposal).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(
            saved.contains("# kept\n[[views]]\nname = \"Ours\""),
            "{saved}"
        );

        let refused = Cli {
            edits: Err("an Agent".into()),
            snapshot: None,
            ..cli
        };
        assert_eq!(refused.propose(&edit).unwrap_err(), "an Agent");
        assert_eq!(refused.save(&proposal).unwrap_err(), "an Agent");
    }

    #[test]
    fn views_are_edited_only_by_a_person_in_a_writable_file() {
        let dir = std::env::temp_dir().join(format!("tm-view-who-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tm.toml");
        std::fs::write(&path, "").unwrap();
        assert_eq!(views::editable(&path, false, false), Ok(()));
        let snapshot = views::editable(&path, true, false).unwrap_err();
        assert!(snapshot.contains("read-only Snapshot"), "{snapshot}");
        let agent = views::editable(&path, false, true).unwrap_err();
        assert!(agent.contains("a person edits"), "{agent}");
        let link = dir.join("linked.toml");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let linked = views::editable(&link, false, false).unwrap_err();
        assert!(linked.contains("symlink"), "{linked}");
    }

    #[test]
    fn the_ready_screen_is_tm_ready_with_its_warnings_first() {
        let dir = std::env::temp_dir().join(format!("tm-ready-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = Store::open(&dir.join("store.db")).unwrap();
        let jace = Actor::human("jace");
        let (project, _) = store.create_project(&jace, "TM", "Core").unwrap();
        let (first, _) = store.create_issue(&jace, project, "task", "alpha").unwrap();
        store.create_issue(&jace, project, "task", "beta").unwrap();
        let (web, _) = store.create_project(&jace, "WEB", "Web").unwrap();
        store.create_issue(&jace, web, "task", "elsewhere").unwrap();
        let (list, _) = store
            .create_shortlist(&jace, project, "docs", &["jace".to_owned()], None)
            .unwrap();
        store
            .shortlist_add(&jace, list, &[first], tm_store::Place::End)
            .unwrap();
        let identity = Identity {
            actor: jace.clone(),
            warnings: vec!["found two harnesses".into()],
        };
        let cli = Cli {
            me: Ok("jace"),
            project,
            reader: Ok(&identity),
            repo: None,
            edits: Ok(()),
            snapshot: None,
        };
        let text = cli.ready(&store).unwrap();
        assert!(text.starts_with("warning: found two harnesses\n"), "{text}");
        let lists = text.find("Short-lists (tm next takes the first that can start):");
        let queue = text.find("Ready (2):");
        assert!(lists.is_some() && queue > lists, "{text}");
        assert!(text.contains("\nTM-2  todo  none  beta\n"), "{text}");
        assert!(!text.contains("WEB-1"), "{text}");
        let nobody = Cli {
            me: Err("unused"),
            project,
            reader: Err("set TM_ACTOR"),
            repo: None,
            edits: Ok(()),
            snapshot: None,
        };
        let text = nobody.ready(&store).unwrap();
        assert!(
            text.starts_with("warning: no identity, so no Short-lists: set TM_ACTOR\nTM-"),
            "{text}"
        );
    }

    #[test]
    fn an_agent_with_no_email_reads_its_lists_as_tm_ready_does() {
        let var = |name: &str| (name == "TM_AGENT").then(|| Ok("claude".to_owned()));
        let (me, reader) = session(var, || Err("no user.email".into()));
        assert!(me.is_err(), "@me needs a person: {me:?}");
        assert_eq!(reader.unwrap().actor.name(), "claude");
    }

    fn busy() -> StoreError {
        StoreError::Busy(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            None,
        ))
    }

    #[test]
    fn a_refusal_carries_the_command_that_fixes_it() {
        let plain = CliError::new("not_found", "no Issue is TM-9");
        assert_eq!(refusal(plain), "no Issue is TM-9");
        let fixable = CliError::new("claimed", "TM-2 is claimed").fix("tm take TM-2");
        assert_eq!(refusal(fixable), "TM-2 is claimed — try: tm take TM-2");
    }

    #[test]
    fn a_busy_store_waits_and_any_other_error_ends_the_tui() {
        let now = || "10:15:03".to_owned();
        assert!(matches!(seen(Err(busy()), now), Ok(Seen::Busy)));
        assert!(matches!(seen(Ok(false), now), Ok(Seen::Nothing)));
        assert!(matches!(seen(Ok(true), now), Ok(Seen::Changed(at)) if at == "10:15:03"));
        let gone = StoreError::Io(io::Error::from(io::ErrorKind::NotFound));
        assert!(matches!(seen(Err(gone), now), Err(Ended::Store(_))));
    }

    #[test]
    fn a_busy_reopen_is_tried_again_and_then_read_as_the_change() {
        let dir = std::env::temp_dir().join(format!("tm-refresh-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.db");
        drop(Store::open(&path).unwrap());
        let now = || "10:15:03".to_owned();
        let mut pending = false;
        let quiet = refresh(Ok(false), &mut pending, || panic!("opened"), now);
        assert!(matches!(quiet, Ok((None, Seen::Nothing))));
        let first = refresh(Ok(true), &mut pending, || Err(busy()), now);
        assert!(matches!(first, Ok((None, Seen::Busy))) && pending);
        let next = refresh(Ok(false), &mut pending, || Store::open(&path), now);
        assert!(matches!(next, Ok((Some(_), Seen::Changed(_)))) && !pending);
        let held = refresh(Ok(true), &mut pending, || Err(busy()), now);
        assert!(matches!(held, Ok((None, Seen::Busy))) && pending);
        let busy_watch = refresh(Err(busy()), &mut pending, || Store::open(&path), now);
        assert!(
            matches!(busy_watch, Ok((Some(_), Seen::Changed(_)))) && !pending,
            "a reopen that succeeds reports the change while the Watch is busy"
        );
        let mut pending = true;
        let watch_gone = Err(StoreError::Io(io::Error::from(io::ErrorKind::NotFound)));
        assert!(
            matches!(
                refresh(watch_gone, &mut pending, || Store::open(&path), now),
                Err(Ended::Store(_))
            ),
            "any other Watch error ends the TUI, reopened or not"
        );
        let gone = || Err(StoreError::Io(io::Error::from(io::ErrorKind::NotFound)));
        assert!(matches!(
            refresh(Ok(true), &mut pending, gone, now),
            Err(Ended::Store(_))
        ));
    }

    #[test]
    fn the_warnings_are_what_did_not_apply_then_the_snapshot_read_read_afresh() {
        let dir = std::env::temp_dir().join(format!("tm-warn-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = Store::open(&dir.join("store.db")).unwrap();
        let (project, _) = store
            .create_project(&Actor::human("jace"), "TM", "Core")
            .unwrap();
        let published = dir.join("snapshot.db");
        std::fs::write(&published, "").unwrap();
        let missing = dir.join("gone.db");
        let cli = |snapshot| Cli {
            me: Ok("jace"),
            project,
            reader: Err("unused"),
            repo: None,
            edits: Ok(()),
            snapshot,
        };
        assert_eq!(cli(None).warnings(&store), Vec::<String>::new());
        store
            .connection()
            .execute(
                "INSERT INTO unapplied (op_id, reason) VALUES (?1, ?2)",
                (Uuid::now_v7().as_bytes().to_vec(), "unknown field"),
            )
            .unwrap();
        let warnings = cli(Some(&published)).warnings(&store);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(
            warnings[0],
            "1 logged operation did not apply in full; tm doctor will list it"
        );
        assert!(
            warnings[1].starts_with("read from the Snapshot published "),
            "{warnings:?}"
        );
        let warnings = cli(Some(&missing)).warnings(&store);
        assert!(
            warnings[1].starts_with(&format!("{}: read as the Snapshot, ", missing.display())),
            "{warnings:?}"
        );
    }
}
