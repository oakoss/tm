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
#[cfg(test)]
mod tests;
mod triage;

use std::io::{self, Write as _};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    Clear as ClearScreen, ClearType, EnterAlternateScreen, enable_raw_mode,
};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::{SigId, flag, low_level};
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
use ready::Ready;
use stack::Stack;
use tm_core::text::printable;
use triage::Triage;

const KEPT: &str = "comment kept, its path printed on exit";

/// How often a write elsewhere is looked for between keys (ADR 0018).
const POLL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy)]
enum Global {
    Quit,
    Help,
    HideHelp,
}

/// Keys every screen has, ahead of its own: Ctrl-C even while text is
/// typed, and Esc only while these keys are shown.
const GLOBAL: [Binding<Global>; 3] = [
    Binding {
        keys: &[ch('?')],
        action: Global::Help,
        does: "show or hide these keys",
        hint: Some("keys"),
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
];

/// What the screens take from the CLI, so they match its commands (ADR 0018).
pub trait Actions {
    /// Who `@me` is in a filter, as `tm list` reads it, or why no one can be.
    ///
    /// # Errors
    ///
    /// Why the person at the terminal cannot be named.
    fn me(&self) -> Result<&str, &str>;

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
    Quit,
}

impl Screen {
    fn load(&mut self, store: &Store, actions: &dyn Actions) {
        match self {
            Self::List(list) => list.load(store, actions.me()),
            Self::Ready(ready) => ready.load(store, actions),
            Self::Issue(issue) => issue.load(store, actions),
            Self::Triage(triage) => triage.load(store, actions),
            Self::Board(board) => board.load(store, actions),
        }
    }

    /// The breadcrumb's name for the screen.
    fn title(&self) -> String {
        match self {
            Self::List(_) => "Issues".to_owned(),
            Self::Ready(_) => "Ready".to_owned(),
            Self::Issue(issue) => issue.title(),
            Self::Triage(_) => "Triage".to_owned(),
            Self::Board(_) => "Board".to_owned(),
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
            Self::Ready(_) => None,
            Self::Issue(issue) => issue.prompt(),
            Self::Triage(triage) => triage.prompt(),
            Self::Board(board) => board.prompt(),
        }
    }

    /// The writes the screen asked for are done.
    fn wrote(&mut self, landed: &[String], refused: bool) {
        if let Self::Triage(triage) = self {
            triage.wrote(landed, refused);
        }
    }

    fn help(&self) -> Vec<(String, &'static str)> {
        match self {
            Self::List(_) => keys::help(&list::KEYS),
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
            Self::List(_) => keys::hints(&list::KEYS),
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
    pending: Option<Pending>,
    /// This session's writes, each action's batches, newest last.
    written: Vec<Vec<Uuid>>,
    /// What the last action did, until the next key.
    said: Option<String>,
    /// Whether the last action wrote anything, so `u` can say when it
    /// reaches back past one that did not.
    last_wrote: bool,
    /// The file holding the comment being written.
    draft: Option<PathBuf>,
    finished: Option<PathBuf>,
    /// Drafts of comments not written, named once the terminal is restored,
    /// as the status line is too narrow for their paths.
    kept: Vec<PathBuf>,
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
            pending: None,
            written: Vec::new(),
            said: None,
            last_wrote: true,
            draft: None,
            finished: None,
            kept: Vec::new(),
        }
    }

    /// Makes the writes or undo a key asked for through `actions`, and says
    /// what it did until the next key.
    pub fn write(&mut self, actions: &dyn Actions) {
        let mut said = match self.pending.take() {
            None | Some(Pending::Compose(_)) => return,
            Some(Pending::Writes(writes)) => self.make(&writes, actions),
            Some(Pending::Undo) => self.undo(actions),
        };
        if let Some(draft) = self.draft.take() {
            match said.strip_prefix("✗ ") {
                Some(why) => {
                    said = format!("✗ {KEPT} · {why}");
                    self.kept.push(draft);
                }
                None => self.finished = Some(draft),
            }
        }
        self.said = Some(printable(said.lines().next().unwrap_or_default()));
    }

    /// Takes what an editor saved for the comment a key asked for, or why
    /// nothing was; [`Shell::write`] then writes a comment that is not
    /// blank, and keeps its draft if the write is refused.
    pub fn compose(&mut self, composed: Result<Composed, Unwritten>) {
        let Some(Pending::Compose(key)) = self.pending.take() else {
            return;
        };
        match composed {
            Ok(Composed { text, draft }) if !text.trim().is_empty() => {
                self.draft = Some(draft);
                self.pending = Some(Pending::Writes(vec![Write::Comment { key, body: text }]));
            }
            Ok(Composed { draft, .. }) => {
                self.finished = Some(draft);
                self.said = Some("nothing written: the comment was empty".to_owned());
            }
            Err(Unwritten { why, kept }) => {
                let why = why.lines().next().unwrap_or_default();
                let said = match kept {
                    Some(draft) => {
                        self.kept.push(draft);
                        format!("✗ {KEPT} · {why}")
                    }
                    None => format!("✗ no comment written: {why}"),
                };
                self.said = Some(printable(&said));
            }
        }
    }

    /// A comment's draft no longer needed, as its text was written or blank.
    pub fn finished_draft(&mut self) -> Option<PathBuf> {
        self.finished.take()
    }

    /// The drafts of comments not written, to name once the terminal is
    /// restored.
    #[must_use]
    pub fn kept_drafts(&self) -> &[PathBuf] {
        &self.kept
    }

    /// Makes `writes`, keeping the batches that landed for `u`, and says
    /// what happened. A write that changed nothing has no batch, so `u`
    /// is not offered for it.
    fn make(&mut self, writes: &[Write], actions: &dyn Actions) -> String {
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
        self.shown().wrote(&landed, !refused.is_empty());
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
            return format!("✗ {refused}{landed}{warned}");
        }
        if count == 0 {
            let said = first_of(&unchanged, "more unchanged").unwrap_or_default();
            return format!("{said}{warned}");
        }
        let said = first_of(&written_lines, "more written").unwrap_or_default();
        let unchanged = match unchanged.len() {
            0 => String::new(),
            n => format!(" · {n} unchanged"),
        };
        format!("✓ {said}{unchanged} · u undo{warned}")
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
    }

    pub fn key(&mut self, key: KeyEvent) -> Flow {
        if key.kind != KeyEventKind::Press {
            return Flow::Stay;
        }
        self.said = None;
        let global = keys::action(&GLOBAL, key);
        if matches!(global, Some(Global::Quit)) {
            return Flow::Quit;
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
        let footer = self.shown().footer();
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

/// Runs the TUI over `store` until a key quits it. Between keys `look` finds
/// writes elsewhere, reopening `store` if it must, as for a replaced
/// Snapshot; its error ends the TUI. The terminal is restored on return, and
/// on panic even where a panic aborts.
///
/// # Errors
///
/// A terminal failure, starting it included, or `look`'s error.
pub fn run<E: From<io::Error>>(
    place: impl Into<String>,
    store: Store,
    actions: &dyn Actions,
    look: impl FnMut(&mut Store) -> Result<Seen, E>,
) -> Result<Ended, E> {
    // Ratatui's own hook leaves the alternate screen and raw mode but not
    // the hidden cursor, which only dropping the terminal shows.
    let shown = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), Show);
        shown(info);
    }));
    // A signal that ends the process would skip the restore below, so the
    // first is caught and the loop leaves through it. A second, or one after
    // the TUI returns, acts as it would have. SIGHUP is left alone: the
    // terminal is gone, and caught it would leave input polling spinning.
    let mut signals = Signals {
        caught: Arc::new(AtomicUsize::new(0)),
        default: Arc::new(AtomicBool::new(false)),
        interrupt: Vec::new(),
    };
    signals.catch(SIGTERM)?;
    signals.listen()?;
    let mut shell = Shell::new(place);
    shell.load(&store, actions);
    let ran = match ratatui::try_init() {
        Ok(mut terminal) => {
            let ran = drive(
                &mut terminal,
                &mut shell,
                (store, actions),
                look,
                &mut signals,
            );
            drop(terminal);
            ran
        }
        // Raw mode may be on already.
        Err(err) => Err(err.into()),
    };
    ratatui::restore();
    signals.default.store(true, Ordering::Relaxed);
    // Written, not `eprintln!`, which panics on a broken stderr pipe and
    // would lose `ran`.
    let mut stderr = io::stderr().lock();
    for draft in shell.kept_drafts() {
        let _ = writeln!(
            stderr,
            "tm: a comment not written is kept in {}",
            draft.display()
        );
    }
    ran
}

/// The signals that end the TUI: the last one caught, and whether the next
/// takes its default action.
struct Signals {
    caught: Arc<AtomicUsize>,
    default: Arc<AtomicBool>,
    /// SIGINT's handlers, dropped while an editor has the terminal.
    interrupt: Vec<SigId>,
}

impl Signals {
    fn catch(&self, signal: i32) -> io::Result<Vec<SigId>> {
        let value = usize::try_from(signal).unwrap_or(usize::MAX);
        Ok(vec![
            flag::register_conditional_default(signal, Arc::clone(&self.default))?,
            flag::register_usize(signal, Arc::clone(&self.caught), value)?,
            flag::register(signal, Arc::clone(&self.default))?,
        ])
    }

    fn listen(&mut self) -> io::Result<()> {
        self.interrupt = self.catch(SIGINT)?;
        Ok(())
    }

    /// Leaves SIGINT to an editor, as git does: Ctrl-C there reaches this
    /// process too, as one of its group, and is the editor's to act on.
    fn hold(&mut self) {
        for id in self.interrupt.drain(..) {
            low_level::unregister(id);
        }
    }
}

/// Why the TUI ended without an error.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    Quit,
    /// A signal asked it to stop; its number.
    Signal(i32),
}

/// Runs `run` with the terminal as the TUI found it, then takes it back and
/// redraws it whole.
fn suspended<T>(terminal: &mut DefaultTerminal, run: impl FnOnce() -> T) -> io::Result<T> {
    ratatui::restore();
    execute!(io::stdout(), Show)?;
    let ran = run();
    enable_raw_mode()?;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        ClearScreen(ClearType::All)
    )?;
    // Both buffers emptied, so the next draw writes every cell. Not
    // `terminal.clear()`, which first asks the terminal where its cursor is
    // and fails where nothing answers.
    terminal.swap_buffers();
    terminal.swap_buffers();
    Ok(ran)
}

fn drive<E: From<io::Error>>(
    terminal: &mut DefaultTerminal,
    shell: &mut Shell,
    (mut store, actions): (Store, &dyn Actions),
    mut look: impl FnMut(&mut Store) -> Result<Seen, E>,
    signals: &mut Signals,
) -> Result<Ended, E> {
    loop {
        let caught = signals.caught.load(Ordering::Relaxed);
        if caught != 0 {
            return Ok(Ended::Signal(i32::try_from(caught).unwrap_or(i32::MAX)));
        }
        terminal.draw(|frame| shell.render(frame))?;
        if event::poll(POLL)?
            && let Event::Key(key) = event::read()?
        {
            match shell.key(key) {
                Flow::Quit => return Ok(Ended::Quit),
                Flow::Reload => shell.load(&store, actions),
                Flow::Write => {
                    shell.write(actions);
                    shell.load(&store, actions);
                }
                Flow::Compose => {
                    signals.hold();
                    let composed = suspended(terminal, editor::compose);
                    signals.listen()?;
                    shell.compose(composed?);
                    shell.write(actions);
                    if let Some(draft) = shell.finished_draft() {
                        let _ = std::fs::remove_file(draft);
                    }
                    shell.load(&store, actions);
                }
                Flow::Stay => {}
            }
        }
        let seen = look(&mut store)?;
        if matches!(seen, Seen::Changed(_)) {
            shell.load(&store, actions);
        }
        shell.seen(seen);
    }
}
