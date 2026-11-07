//! `tm tui`: the screens over this directory's Workspace (ADR 0018).

use std::io::{self, IsTerminal};
use std::process::ExitCode;

use tm_core::ProjectId;
use tm_store::{Store, StoreError};
use tm_tui::Seen;
use uuid::Uuid;

use crate::error::CliError;
use crate::identity::Identity;
use crate::output;
use crate::{edit, here, history, identity, moves, ready, show, undo, watch};

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
    let actions = Cli {
        me,
        project,
        reader: reader.as_ref().map_err(String::as_str),
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
/// reads it or why it cannot, and `project` the one this directory maps to.
struct Cli<'a> {
    me: Result<&'a str, &'a str>,
    project: ProjectId,
    reader: Result<&'a Identity, &'a str>,
}

impl tm_tui::Actions for Cli<'_> {
    fn me(&self) -> Result<&str, &str> {
        self.me
    }

    fn detail(&self, store: &Store, id: Uuid) -> Result<String, String> {
        let me = || self.me.ok().map(str::to_owned);
        let shown = show::render_with(store, id, show::Comments::All, None);
        let written = history::render(store, id, None, false, &me);
        let (shown, written) = shown
            .and_then(|shown| Ok((shown, written?)))
            .map_err(|err| err.message)?;
        // History's first line names the Issue, as show's does.
        let writes = written
            .text
            .split_once('\n')
            .map_or("", |(_, writes)| writes);
        Ok(format!("{}\nHistory\n{writes}", shown.text))
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
        let text = ready::render(store, &lists).map_err(|err| err.message)?;
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
    fn the_detail_is_show_with_every_comment_then_history_naming_the_issue_once() {
        let dir = std::env::temp_dir().join(format!("tm-detail-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = Store::open(&dir.join("store.db")).unwrap();
        let jace = Actor::human("jace");
        let (project, _) = store.create_project(&jace, "TM", "Core").unwrap();
        let (id, _) = store.create_issue(&jace, project, "task", "alpha").unwrap();
        for n in 1..=6 {
            let change = Change::Comment {
                body: format!("note {n}"),
            };
            let draft = Draft {
                actor: jace.clone(),
                project,
                subject: id,
                change,
            };
            store.append(draft).unwrap();
        }
        let cli = Cli {
            me: Ok("jace"),
            project,
            reader: Err("unused"),
        };
        let text = cli.detail(&store, id).unwrap();
        let (shown, writes) = text.split_once("\nHistory\n").expect("a History heading");
        assert!(shown.contains("note 1"), "every comment: {shown}");
        assert!(
            writes.contains("commented: note 6"),
            "history's writes: {writes}"
        );
        assert_eq!(text.matches("TM-1  alpha").count(), 1, "{text}");
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
        };
        let text = cli.ready(&store).unwrap();
        assert!(text.starts_with("warning: found two harnesses\n"), "{text}");
        let lists = text.find("Short-lists (tm next takes the first that can start):");
        let queue = text.find("Ready (2):");
        assert!(lists.is_some() && queue > lists, "{text}");
        assert!(text.contains("\nTM-2  todo  none  beta\n"), "{text}");
        let nobody = Cli {
            me: Err("unused"),
            project,
            reader: Err("set TM_ACTOR"),
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
}
