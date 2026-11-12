//! The TUI's screens, opened by `tm tui` (ADR 0018).

mod board;
mod cursor;
mod editor;
mod issue;
mod keys;
mod list;
mod markdown;
mod picker;
mod read;
mod ready;
mod scroll;
mod stack;
mod terminal;
#[cfg(test)]
mod tests;
mod triage;
mod views;

use std::path::PathBuf;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use tm_core::ProjectId;
use tm_store::Store;
use uuid::Uuid;

use board::Board;
pub use editor::{Composed, Unwritten};
use issue::IssueScreen;
pub use issue::{
    Change, Claim, Comment, History, Issue, LATEST_WRITES, Link, Logged, Related, Shown, Status,
};
use keys::{Binding, ch, ctrl, plain};
use list::List;
use picker::{Picked, Picker};
use ready::Ready;
use stack::Stack;
pub use terminal::{Ended, run};
use tm_core::text::printable;
use tm_store::view::View;
use triage::Triage;
use views::{Confirm, Sidebar};

const VIEW_DRAFT: editor::Draft<'static> = editor::Draft {
    start: "",
    suffix: "toml",
    noun: "View",
};

#[derive(Clone, Copy)]
enum Global {
    Quit,
    Suspend,
    Help,
    HideHelp,
    Views,
}

/// Keys every screen has, ahead of its own: Ctrl-C and Ctrl-Z even while
/// text is typed, and Esc only while these keys are shown.
const GLOBAL: [Binding<Global>; 5] = [
    Binding {
        keys: &[ch('?')],
        action: Global::Help,
        does: "show or hide these keys",
        hint: Some("keys"),
    },
    Binding {
        keys: &[ch('v')],
        action: Global::Views,
        does: "pick a View from .config/tm.toml",
        hint: Some("views"),
    },
    Binding {
        keys: &[plain(KeyCode::Esc)],
        action: Global::HideHelp,
        does: "hide these keys",
        hint: None,
    },
    Binding {
        keys: &[ctrl('c')],
        action: Global::Quit,
        does: "quit",
        hint: None,
    },
    Binding {
        keys: &[ctrl('z')],
        action: Global::Suspend,
        does: "suspend to the shell; fg comes back",
        hint: None,
    },
];

/// What the screens take from the CLI, so they match its commands (ADR 0018).
pub trait Actions {
    /// Who `@me` is in a filter, as `tm list` reads it, or why no one can be.
    ///
    /// # Errors
    ///
    /// Why the person at the terminal cannot be named.
    fn me(&self) -> Result<&str, &str>;

    /// The Project this directory maps to, whose Issues the screens read
    /// unless a filter names another; `None` reads every Project.
    fn project(&self) -> Option<ProjectId> {
        None
    }

    /// The Views the repo file holds for that Project, read afresh.
    ///
    /// # Errors
    ///
    /// Why the repo file cannot be read.
    fn views(&self) -> Result<Vec<tm_store::view::View>, String> {
        Ok(Vec::new())
    }

    /// # Errors
    ///
    /// Why the person here may not change the repo file's Views.
    fn may_edit_views(&self) -> Result<(), String> {
        Err("Views are read-only here".to_owned())
    }

    /// The repo file `edit` would write, to show before writing it.
    ///
    /// # Errors
    ///
    /// Why it cannot be made.
    fn propose(&self, edit: &ViewEdit) -> Result<Proposal, String> {
        let _ = edit;
        Err("Views are read-only here".to_owned())
    }

    /// Writes `proposal`, which a person said yes to: what was saved.
    ///
    /// # Errors
    ///
    /// Why it was not.
    fn save(&self, proposal: &Proposal) -> Result<String, String> {
        let _ = proposal;
        Err("Views are read-only here".to_owned())
    }

    /// `tm show --json` and `tm history --json` for `id`, read from `store`.
    ///
    /// # Errors
    ///
    /// Why it cannot be shown.
    fn issue(&self, store: &Store, id: Uuid) -> Result<Issue, String>;

    /// `tm ready` for the person at the terminal, read from `store`.
    ///
    /// # Errors
    ///
    /// Why the queue cannot be read.
    fn ready(&self, store: &Store) -> Result<String, String>;

    /// `write` as `tm move` or `tm edit` makes it, as the person at the
    /// terminal.
    ///
    /// # Errors
    ///
    /// Why it was refused, such as a field changed since `version`.
    fn write(&self, write: &Write) -> Result<Written, String>;

    /// `tm undo` of `batch`: what it reverted, and the CLI's warnings.
    ///
    /// # Errors
    ///
    /// Why it cannot be undone.
    fn undo(&self, batch: Uuid) -> Result<Written, String>;
}

/// A write a screen asks for, refused if a field it sets changed after
/// `version`, the one the screen read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    /// `tm move KEY STATUS`.
    Move {
        key: String,
        status: String,
        version: i64,
    },
    /// `tm edit KEY --set FIELD=VALUE`, or `--unset FIELD` with no value.
    Set {
        key: String,
        field: String,
        value: Option<String>,
        version: i64,
    },
    /// `tm comment KEY BODY`; comments are append-only, so no version.
    Comment { key: String, body: String },
}

impl Write {
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::Move { key, .. } | Self::Set { key, .. } | Self::Comment { key, .. } => key,
        }
    }
}

/// What a write did: the CLI's line for it, the batch `tm undo` names (none
/// when nothing changed), and the CLI's warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub said: String,
    pub batch: Option<Uuid>,
    pub warnings: Vec<String>,
}

/// What a look for writes elsewhere found.
#[derive(Debug, PartialEq, Eq)]
pub enum Seen {
    Nothing,
    /// Another process wrote; the time it was seen.
    Changed(String),
    /// Another process holds the write lock, so nothing could be read.
    Busy,
}

/// A screen of the TUI; each acts on its own keys.
pub(crate) enum Screen {
    List(List),
    Ready(Ready),
    Issue(IssueScreen),
    Triage(Triage),
    Board(Board),
}

/// What a key on a screen asks of the Shell.
pub(crate) enum Outcome {
    Stay,
    /// Read the screen again, as its filter changed.
    Reload,
    /// Show this screen over the current one.
    Open(Box<Screen>),
    /// Go back to the screen below.
    Back,
    /// Make these writes, then read the screen again.
    Write(Vec<Write>),
    /// Undo this session's last write.
    Undo,
    /// Write a comment on the Issue with this Key in `$EDITOR`.
    Compose(String),
    /// Open this picker over the screen; it takes the keys until it closes.
    Pick(Picker),
    Quit,
}

impl Screen {
    fn load(&mut self, store: &Store, actions: &dyn Actions) {
        match self {
            Self::List(list) => list.load(store, actions),
            Self::Ready(ready) => ready.load(store, actions),
            Self::Issue(issue) => issue.load(store, actions),
            Self::Triage(triage) => triage.load(store, actions),
            Self::Board(board) => board.load(store, actions),
        }
    }

    /// The breadcrumb's name for the screen.
    fn title(&self) -> String {
        match self {
            Self::List(list) => list.title(),
            Self::Ready(_) => "Ready".to_owned(),
            Self::Issue(issue) => issue.title(),
            Self::Triage(_) => "Triage".to_owned(),
            Self::Board(board) => board.title(),
        }
    }

    fn key(&mut self, key: KeyEvent) -> Outcome {
        match self {
            Self::List(list) => list.key(key),
            Self::Ready(ready) => ready.key(key),
            Self::Issue(issue) => issue.key(key),
            Self::Triage(triage) => triage.key(key),
            Self::Board(board) => board.key(key),
        }
    }

    /// What is being typed, as the footer shows it: while there is any, it
    /// takes every key but Ctrl-C.
    fn prompt(&self) -> Option<String> {
        match self {
            Self::List(list) => list.prompt(),
            _ => None,
        }
    }

    /// Whether the last read failed in a way that leaves an open picker
    /// pointing at Issues the screen no longer shows.
    fn unread(&self) -> bool {
        matches!(self, Self::Board(board) if board.unread())
    }

    /// The writes the screen asked for are done.
    fn wrote(&mut self, attempted: &[String], landed: &[String], refused: bool) {
        if let Self::Triage(triage) = self {
            triage.wrote(attempted, landed, refused);
        }
    }

    fn help(&self) -> Vec<(String, &'static str)> {
        match self {
            Self::List(list) => keys::help(list.keys()),
            Self::Ready(_) => keys::help(&ready::KEYS),
            Self::Issue(_) => keys::help(&issue::KEYS),
            Self::Triage(_) => keys::help(&triage::KEYS),
            Self::Board(_) => keys::help(&board::KEYS),
        }
    }

    /// The footer: the text typed, else the screen's hints.
    fn footer(&self) -> String {
        if let Some(prompt) = self.prompt() {
            return prompt;
        }
        let mut hints = match self {
            Self::List(list) => keys::hints(list.keys()),
            Self::Ready(_) => keys::hints(&ready::KEYS),
            Self::Issue(_) => keys::hints(&issue::KEYS),
            Self::Triage(_) => keys::hints(&triage::KEYS),
            Self::Board(_) => keys::hints(&board::KEYS),
        };
        hints.extend(keys::hints(&GLOBAL));
        hints.join(" · ")
    }

    fn render(&mut self, frame: &mut Frame, area: ratatui::layout::Rect) {
        match self {
            Self::List(list) => list.render(frame, area),
            Self::Ready(ready) => ready.render(frame, area),
            Self::Issue(issue) => issue.render(frame, area),
            Self::Triage(triage) => triage.render(frame, area),
            Self::Board(board) => board.render(frame, area),
        }
    }
}

/// What a screen asked of the Shell that needs the CLI.
enum Pending {
    Writes(Vec<Write>),
    Undo,
    Compose(String),
    /// A comment composed in the file `draft`, kept if the write is refused.
    Comment {
        write: Write,
        draft: PathBuf,
    },
    /// A View to write in an editor: `was` as the sidebar read it, none for
    /// a new one.
    ComposeView(Option<View>),
    /// A View change to show as a diff, from the editor's `draft`, if any.
    Propose {
        edit: ViewEdit,
        draft: Option<PathBuf>,
    },
    /// A diff a person said yes to.
    Save {
        proposal: Proposal,
        draft: Option<PathBuf>,
    },
}

/// A change to the repo file's Views: `was` as the sidebar read it, none
/// for a new one, and what it `becomes`, none to remove it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewEdit {
    pub was: Option<View>,
    pub becomes: Option<View>,
}

/// The repo file as read and as the change would write it, and their diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub diff: String,
    pub before: String,
    pub after: String,
}

/// What can be open over the screen shown.
enum Overlay {
    Picker(Picker),
    Views(Sidebar),
    Confirm(Confirm),
}

/// The frame around the screens: where it reads from, the keys, and what
/// the last look for writes elsewhere found.
pub struct Shell {
    place: String,
    help: bool,
    changed: Option<String>,
    busy: bool,
    /// The Issue list, and the screens shown over it.
    screens: Stack,
    /// What is open over the screen shown, which takes every key but Ctrl-C
    /// and Ctrl-Z until it closes.
    overlay: Option<Overlay>,
    pending: Option<Pending>,
    /// This session's writes, each action's batches, newest last.
    written: Vec<Vec<Uuid>>,
    /// What the last action did, until the next key.
    said: Option<String>,
    /// Whether the last action wrote anything, so `u` can say when it
    /// reaches back past one that did not.
    last_wrote: bool,
    /// Drafts of comments not written, named once the terminal is restored,
    /// as the status line is too narrow for their paths.
    kept: Vec<(&'static str, PathBuf)>,
}

/// What the TUI does after a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Flow {
    Stay,
    /// Read the screen shown again, as its filter or the screen changed.
    Reload,
    /// Make the writes a screen asked for with [`Shell::write`], then read
    /// the screen again.
    Write,
    /// Take a comment's text with the terminal handed to an editor, give it
    /// to [`Shell::compose`], then go on as for [`Flow::Write`].
    Compose,
    /// Hand the terminal back and stop, as Ctrl-Z does at a shell.
    Suspend,
    Quit,
}

impl Shell {
    /// `place` names what the TUI reads, such as the Project's Key.
    #[must_use]
    pub fn new(place: impl Into<String>) -> Self {
        Self {
            place: place.into(),
            help: false,
            changed: None,
            busy: false,
            screens: Stack::new(Screen::List(List::default())),
            overlay: None,
            pending: None,
            written: Vec::new(),
            said: None,
            last_wrote: true,
            kept: Vec::new(),
        }
    }

    /// Makes the writes or undo a key asked for through `actions`, and says
    /// what it did until the next key.
    pub fn write(&mut self, actions: &dyn Actions) {
        let said = match self.pending.take() {
            None | Some(Pending::Compose(_) | Pending::ComposeView(_)) => return,
            // A View change is no batch `u` undoes, so the next `u` names
            // what it reaches back to.
            Some(Pending::Propose { edit, draft }) => match actions.propose(&edit) {
                Ok(proposal) => {
                    self.overlay = Some(Overlay::Confirm(Confirm::new(proposal, draft)));
                    return;
                }
                Err(why) => {
                    self.last_wrote = false;
                    match draft {
                        Some(draft) => self.keep("View", draft, &why),
                        None => format!("✗ {why}"),
                    }
                }
            },
            Some(Pending::Save { proposal, draft }) => match actions.save(&proposal) {
                Ok(said) => {
                    self.last_wrote = false;
                    if let Some(draft) = draft {
                        let _ = std::fs::remove_file(draft);
                    }
                    format!("✓ {said}")
                }
                Err(why) => {
                    self.last_wrote = false;
                    match draft {
                        Some(draft) => self.keep("View", draft, &why),
                        None => format!("✗ {why}"),
                    }
                }
            },
            Some(Pending::Writes(writes)) => self.make(&writes, actions).1,
            Some(Pending::Undo) => self.undo(actions),
            Some(Pending::Comment { write, draft }) => {
                let (refused, said) = self.make(&[write], actions);
                if refused {
                    self.keep("comment", draft, said.trim_start_matches("✗ "))
                } else {
                    let _ = std::fs::remove_file(draft);
                    said
                }
            }
        };
        self.said = Some(printable(said.lines().next().unwrap_or_default()));
    }

    /// Takes what an editor saved for the comment a key asked for, or why
    /// nothing was; [`Shell::write`] then writes it.
    pub fn compose(&mut self, composed: Result<Composed, Unwritten>) {
        let key = match self.pending.take() {
            Some(Pending::Compose(key)) => key,
            Some(Pending::ComposeView(was)) => return self.composed_view(was, composed),
            other => {
                self.pending = other;
                return;
            }
        };
        match composed {
            Ok(Composed { text, draft }) => {
                let write = Write::Comment { key, body: text };
                self.pending = Some(Pending::Comment { write, draft });
            }
            Err(Unwritten { why, kept }) => {
                let why = why.lines().next().unwrap_or_default();
                let said = match kept {
                    Some(draft) => self.keep("comment", draft, why),
                    None => format!("✗ no comment written: {why}"),
                };
                self.said = Some(printable(&said));
            }
        }
    }

    fn composed_view(&mut self, was: Option<View>, composed: Result<Composed, Unwritten>) {
        let said = match composed {
            Ok(Composed { text, draft }) if was.is_none() && text == tm_store::view::BLANK => {
                let _ = std::fs::remove_file(draft);
                "no View added".to_owned()
            }
            Ok(Composed { text, draft }) => match View::from_snippet(&text) {
                Ok(becomes) if was.as_ref() == Some(&becomes) => {
                    let _ = std::fs::remove_file(draft);
                    "the View is unchanged; nothing to save".to_owned()
                }
                Ok(becomes) => {
                    let edit = ViewEdit {
                        was,
                        becomes: Some(becomes),
                    };
                    self.pending = Some(Pending::Propose {
                        edit,
                        draft: Some(draft),
                    });
                    return;
                }
                Err(why) => self.keep("View", draft, &why),
            },
            Err(Unwritten { why, kept }) => {
                let why = why.lines().next().unwrap_or_default();
                match kept {
                    Some(draft) => self.keep("View", draft, why),
                    None => format!("✗ no View saved: {why}"),
                }
            }
        };
        self.said = Some(printable(said.lines().next().unwrap_or_default()));
    }

    pub(crate) fn drafting(&self) -> (String, editor::Draft<'static>) {
        match &self.pending {
            Some(Pending::ComposeView(was)) => {
                let start = was
                    .as_ref()
                    .map_or_else(|| tm_store::view::BLANK.to_owned(), View::snippet);
                (start, VIEW_DRAFT)
            }
            _ => (String::new(), editor::COMMENT),
        }
    }

    /// Keeps `draft` to name once the terminal is restored, and says so
    /// beside `why`.
    fn keep(&mut self, noun: &'static str, draft: PathBuf, why: &str) -> String {
        self.kept.push((noun, draft));
        format!("✗ {noun} kept, its path printed on exit · {why}")
    }

    /// The drafts not written, each with what it is, to name once the
    /// terminal is restored: those kept, and one still waiting on its diff,
    /// as the TUI can end before the diff is answered.
    pub(crate) fn kept_drafts(&self) -> Vec<(&'static str, PathBuf)> {
        let waiting = match (&self.overlay, &self.pending) {
            (Some(Overlay::Confirm(confirm)), _) => confirm.draft(),
            (_, Some(Pending::Propose { draft, .. } | Pending::Save { draft, .. })) => {
                draft.as_ref()
            }
            _ => None,
        };
        let mut kept = self.kept.clone();
        kept.extend(waiting.map(|draft| ("View", draft.clone())));
        kept
    }

    /// Says `text` until the next key.
    pub(crate) fn say(&mut self, text: &str) {
        self.said = Some(text.to_owned());
    }

    /// Makes `writes`, keeping the batches that landed for `u`; whether any
    /// was refused, and what happened. A write that changed nothing has no
    /// batch, so `u` is not offered for it.
    fn make(&mut self, writes: &[Write], actions: &dyn Actions) -> (bool, String) {
        let mut batches = Vec::new();
        let mut landed = Vec::new();
        let mut written_lines = Vec::new();
        let mut unchanged = Vec::new();
        let mut warnings = Vec::new();
        let mut refused = Vec::new();
        for write in writes {
            match actions.write(write) {
                Ok(written) => {
                    match written.batch {
                        Some(batch) => {
                            batches.push(batch);
                            landed.push(write.key().to_owned());
                            written_lines.push(written.said);
                        }
                        None => unchanged.push(written.said),
                    }
                    warnings.extend(written.warnings);
                }
                Err(why) => refused.push(format!("{}: {why}", write.key())),
            }
        }
        let attempted: Vec<String> = writes.iter().map(|write| write.key().to_owned()).collect();
        self.shown().wrote(&attempted, &landed, !refused.is_empty());
        let count = batches.len();
        self.last_wrote = count > 0;
        if count > 0 {
            self.written.push(batches);
        }
        let warned = first_of(&warnings, "more warnings")
            .map(|warning| format!(" · warning: {warning}"))
            .unwrap_or_default();
        if let Some(refused) = first_of(&refused, "more refused") {
            let landed = match count {
                0 => String::new(),
                n => format!(" ({n} written; u undoes them)"),
            };
            return (true, format!("✗ {refused}{landed}{warned}"));
        }
        if count == 0 {
            let said = first_of(&unchanged, "more unchanged").unwrap_or_default();
            return (false, format!("{said}{warned}"));
        }
        let said = first_of(&written_lines, "more written").unwrap_or_default();
        let unchanged = match unchanged.len() {
            0 => String::new(),
            n => format!(" · {n} unchanged"),
        };
        (false, format!("✓ {said}{unchanged} · u undo{warned}"))
    }

    /// Undoes this session's last action, newest batch first; a batch that
    /// is refused stays for the next `u`.
    fn undo(&mut self, actions: &dyn Actions) -> String {
        let Some(batches) = self.written.pop() else {
            return "nothing written here to undo".to_owned();
        };
        // The action shown last wrote nothing, so this one came before it.
        let earlier = if std::mem::replace(&mut self.last_wrote, true) {
            ""
        } else {
            "the earlier action: "
        };
        let mut undone = Vec::new();
        let mut warnings = Vec::new();
        let mut kept = Vec::new();
        let mut refused = None;
        for batch in batches.iter().rev() {
            match actions.undo(*batch) {
                Ok(written) => {
                    undone.push(written.said);
                    warnings.extend(written.warnings);
                }
                Err(why) => {
                    kept.push(*batch);
                    refused.get_or_insert(why);
                }
            }
        }
        let warned = first_of(&warnings, "more warnings")
            .map(|warning| format!(" · warning: {warning}"))
            .unwrap_or_default();
        let Some(why) = refused else {
            return format!("✓ {earlier}{}{warned}", undone.join("; "));
        };
        let total = batches.len();
        kept.reverse();
        self.written.push(kept);
        match undone.len() {
            0 => format!("✗ {earlier}not undone: {why}"),
            n => {
                format!("✗ {earlier}{why} ({n} of {total} undone; u tries the rest again){warned}")
            }
        }
    }

    fn shown(&mut self) -> &mut Screen {
        self.screens.top()
    }

    /// Reads the screen shown from `store`.
    pub fn load(&mut self, store: &Store, actions: &dyn Actions) {
        self.shown().load(store, actions);
        match &mut self.overlay {
            Some(Overlay::Picker(_)) if self.screens.top().unread() => self.overlay = None,
            Some(Overlay::Views(sidebar)) => sidebar.read(actions),
            _ => {}
        }
    }

    fn picked_view(&mut self, picked: views::Picked) -> Flow {
        match picked {
            views::Picked::Stay => return Flow::Stay,
            views::Picked::Refused(why) => {
                self.said = Some(printable(&format!("✗ {why}")));
                return Flow::Stay;
            }
            _ => {}
        }
        self.overlay = None;
        match picked {
            views::Picked::Open(view) => {
                self.screens.push(views::screen(view));
                Flow::Reload
            }
            views::Picked::Edit(was) => {
                self.pending = Some(Pending::ComposeView(was));
                Flow::Compose
            }
            views::Picked::Remove(was) => {
                let edit = ViewEdit {
                    was: Some(was),
                    becomes: None,
                };
                self.pending = Some(Pending::Propose { edit, draft: None });
                Flow::Write
            }
            views::Picked::Stay | views::Picked::Refused(_) | views::Picked::Closed => Flow::Stay,
        }
    }

    /// Saves the diff shown on `y`; any other answer leaves it unsaved,
    /// keeping what the editor wrote.
    fn answered(&mut self, answer: views::Answer) -> Flow {
        if matches!(answer, views::Answer::Scrolled) {
            return Flow::Stay;
        }
        let Some(Overlay::Confirm(confirm)) = self.overlay.take() else {
            return Flow::Stay;
        };
        let (proposal, draft) = confirm.into_parts();
        if matches!(answer, views::Answer::Yes) {
            self.pending = Some(Pending::Save { proposal, draft });
            return Flow::Write;
        }
        let said = match draft {
            Some(draft) => self.keep("View", draft, "not saved"),
            None => "not saved".to_owned(),
        };
        self.said = Some(said);
        Flow::Stay
    }

    pub fn key(&mut self, key: KeyEvent) -> Flow {
        if key.kind != KeyEventKind::Press {
            return Flow::Stay;
        }
        self.said = None;
        let global = keys::action(&GLOBAL, key);
        match global {
            Some(Global::Quit) => return Flow::Quit,
            Some(Global::Suspend) => return Flow::Suspend,
            _ => {}
        }
        match &mut self.overlay {
            Some(Overlay::Picker(picker)) => {
                return match picker.key(key) {
                    Picked::Open => Flow::Stay,
                    Picked::Cancelled => {
                        self.overlay = None;
                        Flow::Stay
                    }
                    Picked::Chosen(writes) => {
                        self.overlay = None;
                        self.pending = Some(Pending::Writes(writes));
                        Flow::Write
                    }
                };
            }
            Some(Overlay::Views(sidebar)) => {
                let picked = sidebar.key(key);
                return self.picked_view(picked);
            }
            Some(Overlay::Confirm(confirm)) => {
                let answer = confirm.key(key);
                return self.answered(answer);
            }
            None => {}
        }
        if self.shown().prompt().is_none() {
            match global {
                Some(Global::Help) => {
                    self.help = !self.help;
                    return Flow::Stay;
                }
                Some(Global::HideHelp) if self.help => {
                    self.help = false;
                    return Flow::Stay;
                }
                Some(Global::Views) => {
                    self.help = false;
                    self.overlay = Some(Overlay::Views(Sidebar::default()));
                    return Flow::Reload;
                }
                _ => {}
            }
        }
        match self.shown().key(key) {
            Outcome::Reload => Flow::Reload,
            Outcome::Open(screen) => {
                self.screens.push(*screen);
                Flow::Reload
            }
            Outcome::Back if self.screens.pop() => Flow::Reload,
            Outcome::Stay | Outcome::Back => Flow::Stay,
            Outcome::Write(writes) => {
                self.pending = Some(Pending::Writes(writes));
                Flow::Write
            }
            Outcome::Undo => {
                self.pending = Some(Pending::Undo);
                Flow::Write
            }
            Outcome::Compose(key) => {
                self.pending = Some(Pending::Compose(key));
                Flow::Compose
            }
            Outcome::Pick(picker) => {
                // The keys go to what is drawn on top.
                self.help = false;
                self.overlay = Some(Overlay::Picker(picker));
                Flow::Stay
            }
            Outcome::Quit => Flow::Quit,
        }
    }

    pub fn seen(&mut self, seen: Seen) {
        match seen {
            Seen::Nothing => self.busy = false,
            Seen::Changed(at) => {
                self.busy = false;
                self.changed = Some(at);
            }
            Seen::Busy => self.busy = true,
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        // What the last action did gets a row of its own, cut at its end,
        // so the start that names the Issue and the reason always shows.
        let said_rows = u16::from(self.said.is_some());
        let [top, body, said, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(said_rows),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let titles: Vec<String> = self.screens.iter().map(Screen::title).collect();
        let crumbs = crumbs(&self.place, &titles, usize::from(top.width));
        frame.render_widget(Line::from(crumbs), top);
        self.shown().render(frame, body);
        match &mut self.overlay {
            Some(Overlay::Picker(picker)) => picker.render(frame, body),
            Some(Overlay::Views(sidebar)) => sidebar.render(frame, body),
            Some(Overlay::Confirm(confirm)) => confirm.render(frame, body),
            None => {}
        }
        if let Some(text) = &self.said {
            frame.render_widget(Line::from(cut(text, usize::from(said.width))), said);
        }
        let status = if self.busy {
            Some("store busy, retrying".to_owned())
        } else {
            self.changed
                .as_ref()
                .map(|at| format!("changed elsewhere {at}"))
        };
        let width = status.as_ref().map_or(0, |status| status.chars().count());
        let [keys, right] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(u16::try_from(width).unwrap_or(u16::MAX)),
        ])
        .spacing(1)
        .areas(bottom);
        let footer = match &self.overlay {
            Some(Overlay::Picker(picker)) => picker.prompt(),
            Some(Overlay::Views(_)) => Sidebar::prompt(),
            Some(Overlay::Confirm(_)) => Confirm::prompt(),
            None => self.screens.top().footer(),
        };
        frame.render_widget(Line::from(footer), keys);
        if let Some(status) = status {
            frame.render_widget(Line::from(status).right_aligned(), right);
        }
        if self.help {
            let mut help = self.shown().help();
            help.extend(keys::help(&GLOBAL));
            let wide = help.iter().map(|(keys, _)| keys.chars().count()).max();
            let wide = wide.unwrap_or(0);
            let lines: Vec<Line> = help
                .iter()
                .map(|(keys, does)| Line::from(format!("{keys:>wide$}  {does}")))
                .collect();
            let height = u16::try_from(lines.len()).unwrap_or(u16::MAX) + 2;
            let width = lines.iter().map(Line::width).max().unwrap_or(0) + 2;
            let width = u16::try_from(width).unwrap_or(u16::MAX);
            let area = frame
                .area()
                .centered(Constraint::Length(width), Constraint::Length(height));
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(lines).block(Block::bordered().title("Keys")),
                area,
            );
        }
    }
}

/// `items`' first, with how many others there are, a repeat counted once.
fn first_of(items: &[String], more: &str) -> Option<String> {
    let mut seen = std::collections::BTreeSet::new();
    let items: Vec<&String> = items.iter().filter(|item| seen.insert(*item)).collect();
    let first = *items.first()?;
    Some(match items.len() {
        1 => first.clone(),
        n => format!("{first} (+{} {more})", n - 1),
    })
}

/// Columns `text` takes on a terminal, a wide character two.
fn columns(text: &str) -> usize {
    ratatui::text::Span::raw(text).width()
}

/// `text` cut to `width` terminal columns, ending in `…` when it was longer.
fn cut(text: &str, width: usize) -> String {
    if columns(text) <= width {
        return text.to_owned();
    }
    let mut kept = String::new();
    for c in text.chars() {
        let mut next = kept.clone();
        next.push(c);
        if columns(&next) + 1 > width {
            break;
        }
        kept = next;
    }
    format!("{kept}…")
}

/// The breadcrumb: the place, then the screens open; the oldest screens
/// give way to `…` first, so the one shown keeps its name.
fn crumbs(place: &str, titles: &[String], width: usize) -> String {
    let full = format!("tm · {place} › {}", titles.join(" › "));
    let Some((shown, earlier)) = titles.split_last() else {
        return full;
    };
    if columns(&full) <= width {
        return full;
    }
    for skip in 1..=earlier.len() {
        let kept = &earlier[skip..];
        let mut line = format!("tm · {place} › … › ");
        for title in kept {
            line.push_str(title);
            line.push_str(" › ");
        }
        line.push_str(shown);
        if columns(&line) <= width {
            return line;
        }
    }
    // Too narrow for the place: the screen shown keeps what room there is.
    let hidden = if earlier.is_empty() { "" } else { "… › " };
    cut(&format!("{hidden}{shown}"), width)
}
