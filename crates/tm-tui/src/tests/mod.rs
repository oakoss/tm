//! The screens driven through the Shell, as `tm tui` drives them, over a
//! store and a fake of the CLI's actions.

mod board;
mod issue;
mod list;
mod notices;
mod ready;
mod shell;
mod triage;
mod views;

use std::fmt::Write as _;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyModifiers;
use tm_core::{Actor, Change};
use tm_store::Draft;

use super::*;

/// No one to stand for `@me`, and why.
const NOBODY: Result<&str, &str> = Err("set TM_ACTOR or git's user.email");

fn drawn_sized(shell: &mut Shell, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| shell.render(frame)).unwrap();
    terminal.backend().to_string()
}

fn drawn_at(shell: &mut Shell, width: u16) -> String {
    drawn_sized(shell, width, 10)
}

fn drawn(shell: &mut Shell) -> String {
    drawn_at(shell, 60)
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn typed(shell: &mut Shell, text: &str) -> Flow {
    for c in text.chars() {
        shell.key(press(KeyCode::Char(c)));
    }
    shell.key(press(KeyCode::Enter))
}

fn typed_after_slash(shell: &mut Shell, text: &str) -> Flow {
    shell.key(press(KeyCode::Char('/')));
    while list(shell).prompt().is_some_and(|prompt| prompt.len() > 1) {
        shell.key(press(KeyCode::Backspace));
    }
    for c in text.chars() {
        shell.key(press(KeyCode::Char(c)));
    }
    shell.key(press(KeyCode::Enter))
}

/// A store with three open Issues, the newest assigned to jace, and one
/// done, titled for what they test.
fn store() -> Store {
    let dir = std::env::temp_dir().join(format!("tm-tui-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = Store::open(&dir.join("store.db")).unwrap();
    let jace = Actor::human("jace");
    let (project, _) = store.create_project(&jace, "TM", "Core").unwrap();
    let mut ids = Vec::new();
    for title in ["first", "second\twith a tab", "third \u{1b}[31mred"] {
        ids.push(store.create_issue(&jace, project, "task", title).unwrap().0);
    }
    let (done, _) = store
        .create_issue(&jace, project, "task", "finished")
        .unwrap();
    for (subject, field, value) in [(done, "status", "done"), (ids[2], "assignee", "jace")] {
        let change = Change::SetField {
            field: field.into(),
            label: field.into(),
            value: value.into(),
            value_label: None,
        };
        let _ = store
            .append(Draft {
                actor: jace.clone(),
                project,
                subject,
                change,
            })
            .unwrap();
    }
    store
}

/// Four Issues in triage, TM-1 to TM-4, newest first in the queue; bob
/// holds open work.
fn triage_store() -> Store {
    let mut store = store();
    let jace = Actor::human("jace");
    let project = store.project_by_key("TM").unwrap().unwrap();
    let mut ids = Vec::new();
    for title in ["a", "b", "c", "d"] {
        ids.push(store.create_issue(&jace, project, "task", title).unwrap().0);
    }
    for (subject, field, value) in ids
        .iter()
        .map(|id| (*id, "status", "triage"))
        .chain([(ids[0], "assignee", "bob")])
    {
        let change = Change::SetField {
            field: field.into(),
            label: field.into(),
            value: value.into(),
            value_label: None,
        };
        let _ = store
            .append(Draft {
                actor: jace.clone(),
                project,
                subject,
                change,
            })
            .unwrap();
    }
    store
}

/// Sets `key`'s `field` as another process would.
fn set(store: &mut Store, key: &str, field: &str, value: &str) {
    let id = store.issue_by_key(key).unwrap().unwrap();
    let project = store.project_by_key("TM").unwrap().unwrap();
    let change = Change::SetField {
        field: field.into(),
        label: field.into(),
        value: value.into(),
        value_label: None,
    };
    let _ = store
        .append(Draft {
            actor: Actor::human("jace"),
            project,
            subject: id,
            change,
        })
        .unwrap();
}

fn version(store: &Store, key: &str) -> i64 {
    let id = store.issue_by_key(key).unwrap().unwrap();
    store.issue_with_version(id).unwrap().unwrap().1
}

/// Reports each Issue as [`laid_out`] does, and a ready queue with an escape
/// the terminal must not act on.
struct Fake;

impl Actions for Fake {
    fn write(&self, write: &Write) -> Result<Written, String> {
        Ok(Written {
            said: format!("{} written", write.key()),
            batch: Some(Uuid::nil()),
            warnings: Vec::new(),
        })
    }

    fn undo(&self, _: Uuid) -> Result<Written, String> {
        Ok(Written {
            said: "undone".into(),
            batch: None,
            warnings: Vec::new(),
        })
    }

    fn me(&self) -> Result<&str, &str> {
        NOBODY
    }

    fn ready(&self, store: &Store) -> Result<String, String> {
        let ready = store.ready().map_err(|err| err.to_string())?;
        Ok(format!("{} ready \u{1b}[2Jsafe\n", ready.len()))
    }

    fn issue(&self, store: &Store, id: Uuid) -> Result<Issue, String> {
        laid_out(store, id)
    }
}

/// `id` as the CLI would report it, with a description of thirty list
/// items to scroll, an escape the terminal must not act on, and a write.
fn laid_out(store: &Store, id: Uuid) -> Result<Issue, String> {
    serde_json::from_value(reported(store, id)?).map_err(|err| err.to_string())
}

/// What [`laid_out`] reads, as JSON, so a test can add to it.
fn reported(store: &Store, id: Uuid) -> Result<serde_json::Value, String> {
    let view = store.issue(id).map_err(|err| err.to_string())?;
    let view = view.ok_or("no such Issue")?;
    let items = (1..=30).fold(String::new(), |mut items, n| {
        let _ = writeln!(items, "- line {n}");
        items
    });
    let parent = view.fields.get("parent").and_then(|parent| parent.as_str());
    let parent = parent.map(|parent| {
        let id: Uuid = parent.parse().unwrap();
        let key = store.issue(id).unwrap().and_then(|view| view.key);
        serde_json::json!({ "id": id, "key": key })
    });
    let status = view.fields.get("status").and_then(|status| status.as_str());
    let issue_type = view.fields.get("type").and_then(|value| value.as_str());
    let moves_to: Vec<_> = issue_type
        .and_then(|issue_type| store.scheme().workflow(issue_type))
        .map(|workflow| workflow.statuses.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|other| Some(other.id.as_str()) != status)
        .map(|other| serde_json::json!({ "id": other.id, "label": other.label }))
        .collect();
    let version = store
        .issue_with_version(id)
        .map_err(|err| err.to_string())?;
    let version = version.map_or(0, |(_, version)| version);
    Ok(serde_json::json!({
        "shown": {
            "id": id,
            "key": view.key,
            "version": version,
            "moves_to": moves_to,
            "fields": {
                "title": view.fields.get("title"),
                "status": view.fields.get("status"),
                "description": format!("shown \u{1b}[2Jsafe\n\n{items}"),
            },
            "claim": null,
            "lapsed_claim": null,
            "parent": parent,
            "links": [],
            "comments": [],
            "mentions": [],
        },
        "history": {
            "total": 1,
            "writes": [{
                "actor": "jace",
                "on_behalf_of": null,
                "utc_ms": 0,
                "changes": [{ "line": "created" }],
            }],
        },
    }))
}

/// [`Fake`], with jace at the terminal.
struct Jace;

impl Actions for Jace {
    fn write(&self, write: &Write) -> Result<Written, String> {
        Fake.write(write)
    }

    fn undo(&self, batch: Uuid) -> Result<Written, String> {
        Fake.undo(batch)
    }

    fn me(&self) -> Result<&str, &str> {
        Ok("jace")
    }

    fn issue(&self, store: &Store, id: Uuid) -> Result<Issue, String> {
        laid_out(store, id)
    }

    fn ready(&self, store: &Store) -> Result<String, String> {
        Fake.ready(store)
    }
}

struct Gone;

impl Actions for Gone {
    fn write(&self, write: &Write) -> Result<Written, String> {
        Err(format!("{} not written", write.key()))
    }

    fn undo(&self, _: Uuid) -> Result<Written, String> {
        Err("nothing undone".into())
    }

    fn me(&self) -> Result<&str, &str> {
        NOBODY
    }

    fn issue(&self, _: &Store, _: Uuid) -> Result<Issue, String> {
        Err("Issue TM-2 \u{1b}[2Jdoes not exist".into())
    }

    fn ready(&self, _: &Store) -> Result<String, String> {
        Err("the queue \u{1b}[2Jcannot be read".into())
    }
}

/// Records each write and undo; refuses writes when `refuse` is set.
struct Record {
    me: Result<&'static str, &'static str>,
    refuse: bool,
    /// A Key whose write is refused.
    refuse_key: Option<String>,
    /// A Key whose write changes nothing.
    unchanged_key: Option<String>,
    /// Refuse the next undo.
    refuse_undo: std::cell::Cell<bool>,
    writes: std::cell::RefCell<Vec<Write>>,
    issued: std::cell::RefCell<Vec<Uuid>>,
    undone: std::cell::RefCell<Vec<Uuid>>,
}

impl Record {
    fn new() -> Self {
        Self {
            me: Ok("jace"),
            refuse: false,
            refuse_key: None,
            unchanged_key: None,
            refuse_undo: std::cell::Cell::new(false),
            writes: std::cell::RefCell::default(),
            issued: std::cell::RefCell::default(),
            undone: std::cell::RefCell::default(),
        }
    }
}

impl Actions for Record {
    fn me(&self) -> Result<&str, &str> {
        self.me
    }

    fn issue(&self, store: &Store, id: Uuid) -> Result<Issue, String> {
        laid_out(store, id)
    }

    fn ready(&self, store: &Store) -> Result<String, String> {
        Fake.ready(store)
    }

    fn write(&self, write: &Write) -> Result<Written, String> {
        if self.refuse || self.refuse_key.as_deref() == Some(write.key()) {
            return Err("TM-2 changed since you read it".into());
        }
        self.writes.borrow_mut().push(write.clone());
        if self.unchanged_key.as_deref() == Some(write.key()) {
            return Ok(Written {
                said: format!("{}: nothing changed", write.key()),
                batch: None,
                warnings: Vec::new(),
            });
        }
        let batch = Uuid::now_v7();
        self.issued.borrow_mut().push(batch);
        Ok(Written {
            said: format!("{}  written", write.key()),
            batch: Some(batch),
            warnings: vec!["the Snapshot was not updated".into()],
        })
    }

    fn undo(&self, batch: Uuid) -> Result<Written, String> {
        if self.refuse_undo.replace(false) {
            return Err("changed since".into());
        }
        self.undone.borrow_mut().push(batch);
        Ok(Written {
            said: "undid the write".into(),
            batch: None,
            warnings: vec!["a backup was not made".into()],
        })
    }
}

fn list(shell: &Shell) -> &List {
    match shell.screens.base() {
        Screen::List(list) => list,
        Screen::Ready(_)
        | Screen::Issue(_)
        | Screen::Triage(_)
        | Screen::Board(_)
        | Screen::Notices(_) => {
            unreachable!("the list is first")
        }
    }
}

/// The list with its second Issue open.
fn opened(store: &Store, actions: &dyn Actions) -> Shell {
    let mut shell = Shell::new("TM");
    shell.load(store, actions);
    shell.key(press(KeyCode::Char('j')));
    assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Reload);
    shell.load(store, actions);
    shell
}

fn triaging(store: &Store, actions: &dyn Actions) -> Shell {
    let mut shell = Shell::new("TM");
    shell.load(store, actions);
    assert_eq!(shell.key(press(KeyCode::Char('t'))), Flow::Reload);
    shell.load(store, actions);
    shell
}

fn queue_key(shell: &mut Shell) -> Option<String> {
    match shell.shown() {
        Screen::Triage(triage) => triage.selected_key(),
        _ => None,
    }
}

/// Picks the first choice `typed` narrows `opener`'s picker to.
fn pick(shell: &mut Shell, opener: char, typed: &str) -> Flow {
    shell.key(press(KeyCode::Char(opener)));
    for c in typed.chars() {
        shell.key(press(KeyCode::Char(c)));
    }
    shell.key(press(KeyCode::Enter))
}
