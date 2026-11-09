//! The TUI's screens, opened by `tm tui` (ADR 0018).

mod board;
mod detail;
mod keys;
mod list;
mod picker;
mod read;
mod stack;
mod triage;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::flag;
use tm_store::Store;
use uuid::Uuid;

use board::Board;
use detail::Detail;
use keys::{Binding, ch, ctrl, plain};
use list::List;
use stack::Stack;
use tm_core::text::printable;
use triage::Triage;

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

    /// `tm show` and `tm history` for `id`, read from `store`.
    ///
    /// # Errors
    ///
    /// Why it cannot be shown, such as that it no longer exists.
    fn detail(&self, store: &Store, id: Uuid) -> Result<String, String>;

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
}

impl Write {
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::Move { key, .. } | Self::Set { key, .. } => key,
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
    Detail(Detail),
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
    Quit,
}

impl Screen {
    fn load(&mut self, store: &Store, actions: &dyn Actions) {
        match self {
            Self::List(list) => list.load(store, actions.me()),
            Self::Detail(detail) => detail.load(store, actions),
            Self::Triage(triage) => triage.load(store, actions),
            Self::Board(board) => board.load(store, actions),
        }
    }

    /// The breadcrumb's name for the screen.
    fn title(&self) -> String {
        match self {
            Self::List(_) => "Issues".to_owned(),
            Self::Detail(detail) => detail.title(),
            Self::Triage(_) => "Triage".to_owned(),
            Self::Board(_) => "Board".to_owned(),
        }
    }

    fn key(&mut self, key: KeyEvent) -> Outcome {
        match self {
            Self::List(list) => list.key(key),
            Self::Detail(detail) => detail.key(key),
            Self::Triage(triage) => triage.key(key),
            Self::Board(board) => board.key(key),
        }
    }

    /// Whether keys are text being typed, which takes every key but Ctrl-C.
    const fn typing(&self) -> bool {
        match self {
            Self::List(list) => list.typing(),
            Self::Detail(_) => false,
            Self::Triage(triage) => triage.typing(),
            Self::Board(board) => board.typing(),
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
            Self::Detail(_) => keys::help(&detail::KEYS),
            Self::Triage(_) => keys::help(&triage::KEYS),
            Self::Board(_) => keys::help(&board::KEYS),
        }
    }

    /// The footer: the text typed, else the screen's hints.
    fn footer(&self) -> String {
        let prompt = match self {
            Self::List(list) => list.prompt(),
            Self::Detail(_) => None,
            Self::Triage(triage) => triage.prompt(),
            Self::Board(board) => board.prompt(),
        };
        if let Some(prompt) = prompt {
            return prompt;
        }
        let mut hints = match self {
            Self::List(_) => keys::hints(&list::KEYS),
            Self::Detail(_) => keys::hints(&detail::KEYS),
            Self::Triage(_) => keys::hints(&triage::KEYS),
            Self::Board(_) => keys::hints(&board::KEYS),
        };
        hints.extend(keys::hints(&GLOBAL));
        hints.join(" · ")
    }

    fn render(&mut self, frame: &mut Frame, area: ratatui::layout::Rect) {
        match self {
            Self::List(list) => list.render(frame, area),
            Self::Detail(detail) => detail.render(frame, area),
            Self::Triage(triage) => triage.render(frame, area),
            Self::Board(board) => board.render(frame, area),
        }
    }
}

/// What a screen asked of the Shell that needs the CLI.
enum Pending {
    Writes(Vec<Write>),
    Undo,
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
        }
    }

    /// Makes the writes or undo a key asked for through `actions`, and says
    /// what it did until the next key.
    pub fn write(&mut self, actions: &dyn Actions) {
        let said = match self.pending.take() {
            None => return,
            Some(Pending::Writes(writes)) => self.make(&writes, actions),
            Some(Pending::Undo) => self.undo(actions),
        };
        self.said = Some(printable(said.lines().next().unwrap_or_default()));
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
        if !self.shown().typing() {
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
    let signal = Arc::new(AtomicUsize::new(0));
    let default = Arc::new(AtomicBool::new(false));
    for caught in [SIGTERM, SIGINT] {
        flag::register_conditional_default(caught, Arc::clone(&default))?;
        let value = usize::try_from(caught).unwrap_or(usize::MAX);
        flag::register_usize(caught, Arc::clone(&signal), value)?;
        flag::register(caught, Arc::clone(&default))?;
    }
    let mut shell = Shell::new(place);
    shell.load(&store, actions);
    let ran = match ratatui::try_init() {
        Ok(mut terminal) => {
            let ran = drive(&mut terminal, shell, (store, actions), look, &signal);
            drop(terminal);
            ran
        }
        // Raw mode may be on already.
        Err(err) => Err(err.into()),
    };
    ratatui::restore();
    default.store(true, Ordering::Relaxed);
    ran
}

/// Why the TUI ended without an error.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    Quit,
    /// A signal asked it to stop; its number.
    Signal(i32),
}

fn drive<E: From<io::Error>>(
    terminal: &mut DefaultTerminal,
    mut shell: Shell,
    (mut store, actions): (Store, &dyn Actions),
    mut look: impl FnMut(&mut Store) -> Result<Seen, E>,
    signal: &AtomicUsize,
) -> Result<Ended, E> {
    loop {
        let caught = signal.load(Ordering::Relaxed);
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

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyEventState, KeyModifiers};
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
            store
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

    #[test]
    fn the_list_shows_the_open_issues_with_their_text_made_safe() {
        let store = store();
        let mut shell = Shell::new("TM");
        shell.load(&store, &Fake);
        let screen = drawn(&mut shell);
        insta::assert_snapshot!(screen);
        assert!(!screen.contains("finished"), "done is not open work");
        assert!(!screen.contains('\u{1b}'), "{screen}");
    }

    #[test]
    fn a_typed_filter_and_every_status_change_what_is_read() {
        let store = store();
        let mut shell = Shell::new("TM");
        shell.load(&store, &Fake);
        assert_eq!(shell.key(press(KeyCode::Char('/'))), Flow::Stay);
        assert_eq!(
            shell.key(press(KeyCode::Char('q'))),
            Flow::Stay,
            "q is text"
        );
        shell.key(press(KeyCode::Backspace));
        assert_eq!(typed(&mut shell, "title:first"), Flow::Reload);
        shell.load(&store, &Fake);
        let screen = drawn(&mut shell);
        assert!(
            screen.contains("first") && !screen.contains("second"),
            "{screen}"
        );
        assert!(screen.contains("1 Issue "), "{screen}");
        shell.key(press(KeyCode::Char('/')));
        for _ in 0.."title:first".len() {
            shell.key(press(KeyCode::Backspace));
        }
        shell.key(press(KeyCode::Enter));
        assert_eq!(shell.key(press(KeyCode::Char('a'))), Flow::Reload);
        shell.load(&store, &Fake);
        assert!(drawn(&mut shell).contains("finished"));
    }

    #[test]
    fn esc_drops_a_typed_filter_and_a_bad_one_says_why() {
        let store = store();
        let mut shell = Shell::new("TM");
        shell.load(&store, &Fake);
        shell.key(press(KeyCode::Char('/')));
        shell.key(press(KeyCode::Char('x')));
        assert!(drawn(&mut shell).contains("/x"));
        assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay);
        shell.load(&store, &Fake);
        let dropped = drawn(&mut shell);
        assert!(
            !dropped.contains("/x") && dropped.contains("open Issues"),
            "{dropped}"
        );
        assert!(dropped.contains("3 Issues"), "{dropped}");
        assert_eq!(typed_after_slash(&mut shell, "nonsense:1"), Flow::Reload);
        shell.load(&store, &Fake);
        assert!(drawn(&mut shell).contains("unknown field"));
    }

    #[test]
    fn at_me_reads_the_person_or_says_why_no_one_can() {
        let store = store();
        let mut shell = Shell::new("TM");
        typed_after_slash(&mut shell, "assignee:@me");
        shell.load(&store, &Jace);
        let mine = drawn(&mut shell);
        assert!(
            mine.contains("third") && mine.contains("1 Issue "),
            "{mine}"
        );
        shell.load(&store, &Fake);
        let nobody = drawn_at(&mut shell, 100);
        assert!(
            nobody.contains("@me needs to know who you are: set TM_ACTOR"),
            "{nobody}"
        );
    }

    #[test]
    fn a_read_that_shrinks_the_list_keeps_a_row_selected() {
        let store = store();
        let mut shell = Shell::new("TM");
        shell.load(&store, &Fake);
        shell.key(press(KeyCode::Char('G')));
        assert_eq!(list(&shell).selected_key().as_deref(), Some("TM-1"));
        typed_after_slash(&mut shell, "title:third");
        shell.load(&store, &Fake);
        assert_eq!(list(&shell).selected_key().as_deref(), Some("TM-3"));
    }

    #[test]
    fn while_typing_question_mark_is_text_and_a_control_key_is_not() {
        let mut shell = Shell::new("TM");
        shell.key(press(KeyCode::Char('/')));
        assert_eq!(shell.key(press(KeyCode::Char('?'))), Flow::Stay);
        let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(shell.key(ctrl_w), Flow::Stay);
        assert_eq!(list(&shell).prompt().as_deref(), Some("/?"));
        assert!(!drawn_sized(&mut shell, 60, 16).contains("Keys"));
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

    #[test]
    fn the_selection_moves_and_stays_on_its_issue_across_a_read() {
        let mut store = store();
        let mut shell = Shell::new("TM");
        shell.load(&store, &Fake);
        let selected = |shell: &Shell| list(shell).selected_key();
        assert_eq!(selected(&shell).as_deref(), Some("TM-3"), "the first row");
        shell.key(press(KeyCode::Char('G')));
        shell.key(press(KeyCode::Up));
        assert_eq!(selected(&shell).as_deref(), Some("TM-2"));
        let jace = Actor::human("jace");
        let project = store.project_by_key("TM").unwrap().unwrap();
        store
            .create_issue(&jace, project, "task", "zeroth")
            .unwrap();
        shell.load(&store, &Fake);
        assert_eq!(
            selected(&shell).as_deref(),
            Some("TM-2"),
            "kept across a read"
        );
        shell.key(press(KeyCode::Char('g')));
        assert_eq!(selected(&shell).as_deref(), Some("TM-5"));
        shell.key(press(KeyCode::PageDown));
        assert_eq!(
            selected(&shell).as_deref(),
            Some("TM-1"),
            "clamped to the last"
        );
    }

    #[test]
    fn q_and_ctrl_c_quit_and_a_release_does_nothing() {
        let mut shell = Shell::new("TM");
        assert_eq!(shell.key(press(KeyCode::Char('q'))), Flow::Quit);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(shell.key(ctrl_c), Flow::Quit);
        let release = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
            KeyEventState::NONE,
        );
        assert_eq!(shell.key(release), Flow::Stay);
        shell.key(press(KeyCode::Char('/')));
        assert_eq!(shell.key(ctrl_c), Flow::Quit, "even while typing");
    }

    #[test]
    fn question_mark_shows_the_keys_and_esc_hides_them() {
        let mut shell = Shell::new("TM");
        assert_eq!(shell.key(press(KeyCode::Char('?'))), Flow::Stay);
        insta::assert_snapshot!(drawn_sized(&mut shell, 60, 17));
        assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay);
        assert!(!drawn(&mut shell).contains("Keys"));
    }

    #[test]
    fn a_busy_store_shows_until_a_look_gets_through() {
        let mut shell = Shell::new("TM");
        shell.seen(Seen::Changed("10:15:03".into()));
        shell.seen(Seen::Busy);
        assert!(drawn(&mut shell).contains("store busy, retrying"));
        shell.seen(Seen::Nothing);
        let after = drawn(&mut shell);
        assert!(!after.contains("busy"), "{after}");
        assert!(after.contains("changed elsewhere 10:15:03"), "{after}");
    }

    #[test]
    fn a_narrow_footer_keeps_the_status_whole_beside_the_keys() {
        let mut shell = Shell::new("TM");
        shell.seen(Seen::Changed("10:15:03".into()));
        let footer = drawn_at(&mut shell, 34).lines().last().unwrap().to_owned();
        assert!(footer.contains("changed elsewhere 10:15:03"), "{footer}");
    }

    #[test]
    fn no_size_panics() {
        let store = store();
        let mut shell = Shell::new("TM");
        shell.load(&store, &Fake);
        shell.seen(Seen::Changed("10:15:03".into()));
        let states = [
            (None, false),
            (None, true),
            (Some(KeyCode::Enter), true),
            (Some(KeyCode::Enter), false),
            (Some(KeyCode::Char('r')), false),
            (Some(KeyCode::Char('r')), true),
        ];
        for (open, help) in states {
            if let Some(open) = open {
                if shell.screens.depth() > 1 {
                    shell.key(press(KeyCode::Esc));
                }
                shell.key(press(open));
                shell.load(&store, &Fake);
            }
            if help != shell.help {
                shell.key(press(KeyCode::Char('?')));
            }
            for width in 0..50 {
                for height in 0..12 {
                    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                    terminal.draw(|frame| shell.render(frame)).unwrap();
                    shell.key(press(KeyCode::Char('G')));
                }
            }
        }
    }

    /// Shows an Issue's Key beside a tab and an escape the terminal must not
    /// act on, then lines to scroll.
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

        fn detail(&self, store: &Store, id: Uuid) -> Result<String, String> {
            let view = store.issue(id).map_err(|err| err.to_string())?;
            let key = view.and_then(|view| view.key).ok_or("no such Issue")?;
            let lines = (1..=30).map(|n| format!("line {n}\n"));
            Ok(format!(
                "{key}\tshown \u{1b}[2Jsafe\n{}",
                lines.collect::<String>()
            ))
        }

        fn ready(&self, store: &Store) -> Result<String, String> {
            let ready = store.ready().map_err(|err| err.to_string())?;
            Ok(format!("{} ready \u{1b}[2Jsafe\n", ready.len()))
        }
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

        fn detail(&self, store: &Store, id: Uuid) -> Result<String, String> {
            Fake.detail(store, id)
        }

        fn ready(&self, store: &Store) -> Result<String, String> {
            Fake.ready(store)
        }
    }

    fn list(shell: &Shell) -> &List {
        match shell.screens.base() {
            Screen::List(list) => list,
            Screen::Detail(_) | Screen::Triage(_) | Screen::Board(_) => {
                unreachable!("the list is first")
            }
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

        fn detail(&self, _: &Store, _: Uuid) -> Result<String, String> {
            Err("Issue TM-2 \u{1b}[2Jdoes not exist".into())
        }

        fn ready(&self, _: &Store) -> Result<String, String> {
            Err("the queue \u{1b}[2Jcannot be read".into())
        }
    }

    /// Lines that each wrap once inside a 20-column frame's borders.
    struct Wide;

    impl Actions for Wide {
        fn write(&self, write: &Write) -> Result<Written, String> {
            Err(format!("{} not written", write.key()))
        }

        fn undo(&self, _: Uuid) -> Result<Written, String> {
            Err("nothing undone".into())
        }

        fn me(&self) -> Result<&str, &str> {
            NOBODY
        }

        fn detail(&self, _: &Store, _: Uuid) -> Result<String, String> {
            Ok((1..=20).fold(String::new(), |mut text, n| {
                let _ = writeln!(text, "row{n:02} wraps in here");
                text
            }))
        }

        fn ready(&self, store: &Store) -> Result<String, String> {
            self.detail(store, Uuid::nil())
        }
    }

    #[test]
    fn wrapped_text_scrolls_to_its_last_line() {
        let store = store();
        let mut shell = opened(&store, &Wide);
        drawn_sized(&mut shell, 20, 10);
        shell.key(press(KeyCode::Char('G')));
        let bottom = drawn_sized(&mut shell, 20, 10);
        assert!(bottom.contains("row20"), "{bottom}");
    }

    fn opened(store: &Store, actions: &dyn Actions) -> Shell {
        let mut shell = Shell::new("TM");
        shell.load(store, actions);
        shell.key(press(KeyCode::Char('j')));
        assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Reload);
        shell.load(store, actions);
        shell
    }

    #[test]
    fn enter_opens_the_selected_issue_as_the_cli_shows_it() {
        let store = store();
        let mut shell = opened(&store, &Fake);
        insta::assert_snapshot!(drawn(&mut shell));
    }

    #[test]
    fn q_esc_and_backspace_go_back_to_the_list_where_it_was() {
        let store = store();
        for back in [KeyCode::Char('q'), KeyCode::Esc, KeyCode::Backspace] {
            let mut shell = opened(&store, &Fake);
            assert_eq!(shell.key(press(back)), Flow::Reload, "{back:?}");
            shell.load(&store, &Fake);
            assert!(drawn(&mut shell).contains("open Issues"), "{back:?}");
            assert_eq!(list(&shell).selected_key().as_deref(), Some("TM-2"));
        }
        let mut shell = opened(&store, &Fake);
        shell.key(press(KeyCode::Esc));
        assert_eq!(shell.key(press(KeyCode::Char('q'))), Flow::Quit);
    }

    #[test]
    fn the_keys_shown_are_the_open_screens() {
        let store = store();
        let mut shell = opened(&store, &Fake);
        shell.key(press(KeyCode::Char('?')));
        let help = drawn_sized(&mut shell, 60, 16);
        assert!(
            help.contains("q Esc Bksp  back") && help.contains("scroll down and up"),
            "{help}"
        );
        assert!(!help.contains("type a filter"), "{help}");
        let footer = help.lines().last().unwrap_or_default();
        assert!(
            footer.starts_with("\"q back · j k scroll · ? keys "),
            "{footer}"
        );
    }

    #[test]
    fn a_modified_esc_hides_the_keys_and_ctrl_c_quits_with_any_modifier() {
        let store = store();
        let mut shell = opened(&store, &Fake);
        shell.key(press(KeyCode::Char('?')));
        let shift_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::SHIFT);
        assert_eq!(shell.key(shift_esc), Flow::Stay);
        assert!(!shell.help && shell.screens.depth() == 2);
        let ctrl_alt_c = KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert_eq!(shell.key(ctrl_alt_c), Flow::Quit);
    }

    #[test]
    fn esc_while_typing_drops_the_filter_whether_or_not_the_keys_show() {
        let mut shell = Shell::new("TM");
        shell.key(press(KeyCode::Char('?')));
        shell.key(press(KeyCode::Char('/')));
        shell.key(press(KeyCode::Char('x')));
        assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay);
        assert_eq!(list(&shell).prompt(), None);
        assert!(shell.help, "the keys stay until Esc again");
    }

    #[test]
    fn no_key_is_named_twice_on_a_screen() {
        let named = |table: Vec<(String, &'static str)>| -> Vec<String> {
            table
                .into_iter()
                .flat_map(|(keys, _)| keys.split(' ').map(str::to_owned).collect::<Vec<_>>())
                .collect()
        };
        let global = named(keys::help(&GLOBAL));
        for (screen, table) in [
            ("list", keys::help(&list::KEYS)),
            ("detail", keys::help(&detail::KEYS)),
        ] {
            let keys = named(table);
            let mut seen = std::collections::BTreeSet::new();
            for key in &keys {
                assert!(seen.insert(key), "{screen} names {key} twice");
                // Esc is the screen's own unless the keys show.
                assert!(
                    key == "Esc" || !global.contains(key),
                    "{screen} shadows {key}"
                );
            }
        }
    }

    #[test]
    fn esc_hides_the_keys_before_it_leaves_an_issue() {
        let store = store();
        let mut shell = opened(&store, &Fake);
        shell.key(press(KeyCode::Char('?')));
        assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay);
        assert!(!shell.help && shell.screens.depth() > 1);
    }

    #[test]
    fn an_issue_scrolls_no_further_than_its_last_line_and_keeps_its_place() {
        let store = store();
        let mut shell = opened(&store, &Fake);
        drawn(&mut shell);
        for _ in 0..100 {
            shell.key(press(KeyCode::Char('j')));
        }
        let bottom = drawn(&mut shell);
        assert!(
            bottom.contains("line 30") && !bottom.contains("line 22"),
            "{bottom}"
        );
        shell.key(press(KeyCode::Char('g')));
        assert!(drawn(&mut shell).contains("safe"));
        shell.key(press(KeyCode::Char('G')));
        assert_eq!(drawn(&mut shell), bottom);
        shell.key(press(KeyCode::Char('k')));
        shell.load(&store, &Fake);
        let reread = drawn(&mut shell);
        assert!(
            reread.contains("line 29") && !reread.contains("line 30"),
            "{reread}"
        );
    }

    #[test]
    fn an_issue_that_cannot_be_shown_says_why_in_place() {
        let store = store();
        let mut shell = opened(&store, &Gone);
        let screen = drawn(&mut shell);
        assert!(screen.contains(r"Issue TM-2 \u{1b}[2Jdoes not"), "{screen}");
        assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Reload);
    }

    #[test]
    fn r_opens_the_ready_queue_as_the_cli_prints_it_and_q_goes_back() {
        let store = store();
        let mut shell = Shell::new("TM");
        shell.load(&store, &Fake);
        shell.key(press(KeyCode::Char('j')));
        assert_eq!(shell.key(press(KeyCode::Char('r'))), Flow::Reload);
        shell.load(&store, &Fake);
        insta::assert_snapshot!(drawn(&mut shell));
        for back in [KeyCode::Char('q'), KeyCode::Esc, KeyCode::Backspace] {
            if shell.screens.depth() == 1 {
                shell.key(press(KeyCode::Char('r')));
                shell.load(&store, &Fake);
            }
            assert_eq!(shell.key(press(back)), Flow::Reload, "{back:?}");
            shell.load(&store, &Fake);
            assert!(drawn(&mut shell).contains("open Issues"), "{back:?}");
            assert_eq!(list(&shell).selected_key().as_deref(), Some("TM-2"));
        }
    }

    #[test]
    fn r_with_an_issue_open_leaves_it_open() {
        let store = store();
        let mut shell = opened(&store, &Fake);
        assert_eq!(shell.key(press(KeyCode::Char('r'))), Flow::Stay);
        assert!(drawn(&mut shell).contains("┌ TM-2 "));
    }

    #[test]
    fn the_ready_queue_reads_a_change_in_place() {
        let mut store = store();
        let mut shell = Shell::new("TM");
        shell.key(press(KeyCode::Char('r')));
        shell.load(&store, &Fake);
        assert!(drawn(&mut shell).contains("3 ready"));
        let jace = Actor::human("jace");
        let project = store.project_by_key("TM").unwrap().unwrap();
        store
            .create_issue(&jace, project, "task", "fourth")
            .unwrap();
        shell.load(&store, &Fake);
        assert!(drawn(&mut shell).contains("4 ready"));
    }

    #[test]
    fn r_while_typing_a_filter_is_text() {
        let mut shell = Shell::new("TM");
        shell.key(press(KeyCode::Char('/')));
        assert_eq!(shell.key(press(KeyCode::Char('r'))), Flow::Stay);
        assert_eq!(shell.screens.depth(), 1);
        assert_eq!(list(&shell).prompt().as_deref(), Some("/r"));
    }

    #[test]
    fn a_ready_queue_that_cannot_be_read_says_why_in_place() {
        let store = store();
        let mut shell = Shell::new("TM");
        shell.key(press(KeyCode::Char('r')));
        shell.load(&store, &Gone);
        let screen = drawn(&mut shell);
        assert!(
            screen.contains(r"the queue \u{1b}[2Jcannot be read"),
            "{screen}"
        );
    }

    #[test]
    fn enter_with_no_issue_listed_stays() {
        let store = store();
        let mut shell = Shell::new("TM");
        typed_after_slash(&mut shell, "title:nothing");
        shell.load(&store, &Fake);
        assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Stay);
        assert_eq!(shell.screens.depth(), 1);
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

        fn detail(&self, store: &Store, id: Uuid) -> Result<String, String> {
            Fake.detail(store, id)
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
            store
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

    #[test]
    fn t_opens_the_issues_in_triage_and_only_those() {
        let store = triage_store();
        let mut shell = triaging(&store, &Record::new());
        let screen = drawn_sized(&mut shell, 70, 10);
        assert!(
            screen.contains("Triage") && screen.contains("4 Issues"),
            "{screen}"
        );
        assert!(
            !screen.contains("first"),
            "an open Issue not in triage: {screen}"
        );
        assert!(
            screen.contains("s status") && screen.contains("u undo"),
            "{screen}"
        );
    }

    #[test]
    fn the_queue_shows_an_issues_text_made_safe() {
        let mut store = triage_store();
        set(&mut store, "TM-5", "title", "tabbed\there \u{1b}[31mred");
        let mut shell = triaging(&store, &Record::new());
        let screen = drawn_sized(&mut shell, 70, 10);
        assert!(screen.contains("tabbed here \\u{1b}[31mred"), "{screen}");
        assert!(!screen.contains(['\t', '\u{1b}']), "{screen}");
    }

    #[test]
    fn a_status_picked_moves_the_issue_as_read_and_goes_on_to_the_next() {
        let store = triage_store();
        let record = Record::new();
        let mut shell = triaging(&store, &record);
        let first = queue_key(&mut shell).unwrap();
        shell.key(press(KeyCode::Char('j')));
        let second = queue_key(&mut shell).unwrap();
        shell.key(press(KeyCode::Char('k')));
        assert_eq!(pick(&mut shell, 's', "todo"), Flow::Write);
        shell.write(&record);
        shell.load(&store, &record);
        let id = store.issue_by_key(&first).unwrap().unwrap();
        let version = store.issue_with_version(id).unwrap().unwrap().1;
        assert_eq!(
            *record.writes.borrow(),
            [Write::Move {
                key: first.clone(),
                status: "todo".into(),
                version,
            }]
        );
        assert_eq!(queue_key(&mut shell), Some(second), "the next Issue");
        let footer = drawn_sized(&mut shell, 70, 10);
        assert!(
            footer.contains(&format!("✓ {first}  written · u undo")),
            "{footer}"
        );
        shell.key(press(KeyCode::Char('j')));
        assert!(
            !drawn_sized(&mut shell, 70, 10).contains("✓"),
            "said until the next key"
        );
    }

    #[test]
    fn marked_issues_take_the_action_and_at_me_comes_first() {
        let store = triage_store();
        let record = Record::new();
        let mut shell = triaging(&store, &record);
        shell.key(press(KeyCode::Char(' ')));
        shell.key(press(KeyCode::Char(' ')));
        assert!(drawn_sized(&mut shell, 70, 10).contains("2 marked"));
        shell.key(press(KeyCode::Char('A')));
        let picker = drawn_sized(&mut shell, 70, 12);
        let lines: Vec<Vec<char>> = picker.lines().map(|line| line.chars().collect()).collect();
        let (top, left) = lines
            .iter()
            .enumerate()
            .find_map(|(row, line)| {
                let text: String = line.iter().collect();
                text.find("┌ Assign")
                    .map(|byte| (row, text[..byte].chars().count()))
            })
            .unwrap();
        let choices: Vec<String> = lines[top + 1..]
            .iter()
            .take_while(|line| line.get(left) == Some(&'│'))
            .map(|line| {
                let inside = line[left + 1..].iter().take_while(|c| **c != '│');
                inside.collect::<String>().trim().to_owned()
            })
            .collect();
        assert_eq!(choices, ["jace (@me)", "bob", "nobody"], "{picker}");
        assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Write);
        shell.write(&record);
        let writes = record.writes.borrow();
        assert_eq!(writes.len(), 2);
        assert!(writes.iter().all(|write| matches!(
            write,
            Write::Set { field, value: Some(value), .. } if field == "assignee" && value == "jace"
        )));
    }

    #[test]
    fn a_name_no_one_holds_yet_is_assigned_as_typed_and_nobody_unsets() {
        let store = triage_store();
        let record = Record::new();
        let mut shell = triaging(&store, &record);
        assert_eq!(pick(&mut shell, 'A', "kim"), Flow::Write);
        shell.write(&record);
        shell.load(&store, &record);
        assert_eq!(pick(&mut shell, 'A', "nobody"), Flow::Write);
        shell.write(&record);
        let writes = record.writes.borrow();
        assert!(matches!(&writes[0], Write::Set { value: Some(value), .. } if value == "kim"));
        assert!(matches!(&writes[1], Write::Set { value: None, .. }));
    }

    #[test]
    fn a_refused_write_says_why_and_the_cursor_stays() {
        let store = triage_store();
        let record = Record {
            refuse: true,
            ..Record::new()
        };
        let mut shell = triaging(&store, &record);
        let first = queue_key(&mut shell);
        pick(&mut shell, 'p', "high");
        shell.write(&record);
        shell.load(&store, &record);
        assert_eq!(queue_key(&mut shell), first);
        let screen = drawn_sized(&mut shell, 80, 10);
        assert!(screen.contains("✗ "), "{screen}");
        assert!(record.writes.borrow().is_empty());
    }

    #[test]
    fn u_undoes_this_sessions_writes_newest_first() {
        let store = triage_store();
        let record = Record::new();
        let mut shell = triaging(&store, &record);
        assert_eq!(shell.key(press(KeyCode::Char('u'))), Flow::Write);
        shell.write(&record);
        assert!(drawn_sized(&mut shell, 70, 10).contains("nothing written here to undo"));
        pick(&mut shell, 'p', "high");
        shell.write(&record);
        shell.load(&store, &record);
        pick(&mut shell, 'p', "low");
        shell.write(&record);
        shell.load(&store, &record);
        shell.key(press(KeyCode::Char('u')));
        shell.write(&record);
        assert_eq!(record.undone.borrow().len(), 1);
        assert!(drawn_sized(&mut shell, 70, 10).contains("✓ undid the write"));
    }

    #[test]
    fn esc_cancels_a_picker_and_its_keys_are_text() {
        let store = triage_store();
        let record = Record::new();
        let mut shell = triaging(&store, &record);
        shell.key(press(KeyCode::Char('s')));
        assert_eq!(shell.key(press(KeyCode::Char('?'))), Flow::Stay);
        assert!(!shell.help, "? is text in a picker");
        assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay);
        assert_eq!(
            shell.key(press(KeyCode::Char('q'))),
            Flow::Reload,
            "q goes back once the picker is gone"
        );
        assert!(record.writes.borrow().is_empty());
    }

    /// Moves `key` out of triage, as another process would.
    fn moved_elsewhere(store: &mut Store, key: &str) {
        let jace = Actor::human("jace");
        let id = store.issue_by_key(key).unwrap().unwrap();
        let project = store.project_by_key("TM").unwrap().unwrap();
        let change = Change::SetField {
            field: "status".into(),
            label: "status".into(),
            value: "todo".into(),
            value_label: None,
        };
        store
            .append(Draft {
                actor: jace,
                project,
                subject: id,
                change,
            })
            .unwrap();
    }

    fn written_keys(record: &Record) -> Vec<String> {
        let writes = record.writes.borrow();
        writes.iter().map(|write| write.key().to_owned()).collect()
    }

    #[test]
    fn a_picker_writes_the_issue_it_opened_on_whatever_a_read_moves() {
        let mut store = triage_store();
        let record = Record::new();
        let mut shell = triaging(&store, &record);
        let first = queue_key(&mut shell).unwrap();
        shell.key(press(KeyCode::Char('p')));
        moved_elsewhere(&mut store, &first);
        shell.load(&store, &record);
        for c in "high".chars() {
            shell.key(press(KeyCode::Char(c)));
        }
        assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Write);
        shell.write(&record);
        assert_eq!(written_keys(&record), [first]);
    }

    #[test]
    fn a_partly_refused_action_keeps_the_refused_marked_and_u_undoes_what_landed() {
        let store = triage_store();
        let mut record = Record::new();
        let mut shell = triaging(&store, &record);
        let first = queue_key(&mut shell).unwrap();
        shell.key(press(KeyCode::Char(' ')));
        shell.key(press(KeyCode::Char(' ')));
        record.refuse_key = Some(first);
        pick(&mut shell, 'p', "high");
        shell.write(&record);
        shell.load(&store, &record);
        let screen = drawn_sized(&mut shell, 110, 10);
        assert!(screen.contains("(1 written; u undoes them)"), "{screen}");
        assert!(screen.contains("1 marked"), "the refused one: {screen}");
        shell.key(press(KeyCode::Char('u')));
        shell.write(&record);
        assert_eq!(*record.undone.borrow(), *record.issued.borrow());
    }

    #[test]
    fn a_write_that_changes_nothing_offers_no_undo() {
        let store = triage_store();
        let mut record = Record::new();
        let mut shell = triaging(&store, &record);
        record.unchanged_key = queue_key(&mut shell);
        pick(&mut shell, 'p', "none");
        shell.write(&record);
        let screen = drawn_sized(&mut shell, 110, 10);
        assert!(
            screen.contains("nothing changed") && !screen.contains("✓"),
            "{screen}"
        );
        shell.key(press(KeyCode::Char('u')));
        shell.write(&record);
        assert!(drawn_sized(&mut shell, 110, 10).contains("nothing written here to undo"));
    }

    #[test]
    fn u_takes_the_newest_action_first_and_keeps_one_refused_for_another_try() {
        let store = triage_store();
        let record = Record::new();
        let mut shell = triaging(&store, &record);
        for level in ["high", "low"] {
            pick(&mut shell, 'p', level);
            shell.write(&record);
            shell.load(&store, &record);
        }
        let issued = record.issued.borrow().clone();
        record.refuse_undo.set(true);
        shell.key(press(KeyCode::Char('u')));
        shell.write(&record);
        assert!(drawn_sized(&mut shell, 110, 10).contains("✗ not undone"));
        for _ in 0..2 {
            shell.key(press(KeyCode::Char('u')));
            shell.write(&record);
        }
        assert_eq!(*record.undone.borrow(), [issued[1], issued[0]]);
    }

    #[test]
    fn after_a_write_from_the_middle_the_cursor_goes_down_not_up() {
        let store = triage_store();
        let record = Record::new();
        let mut shell = triaging(&store, &record);
        shell.key(press(KeyCode::Char('j')));
        shell.key(press(KeyCode::Char('j')));
        shell.key(press(KeyCode::Char('j')));
        let after = queue_key(&mut shell);
        shell.key(press(KeyCode::Char('k')));
        pick(&mut shell, 'p', "high");
        shell.write(&record);
        shell.load(&store, &record);
        assert_eq!(queue_key(&mut shell), after);
    }

    #[test]
    fn the_selected_issue_shows_beside_the_queue_and_the_crumbs_name_the_screens() {
        let store = triage_store();
        let record = Record::new();
        let mut shell = triaging(&store, &record);
        let screen = drawn_sized(&mut shell, 110, 10);
        assert!(screen.contains("tm · TM › Issues › Triage"), "{screen}");
        assert!(
            screen.contains("shown") && screen.contains("line 1"),
            "{screen}"
        );
        shell.key(press(KeyCode::Char('j')));
        shell.load(&store, &record);
        let selected = queue_key(&mut shell).unwrap();
        assert!(drawn_sized(&mut shell, 110, 10).contains(&format!("┌ {selected} ")));
    }

    #[test]
    fn a_warning_from_the_cli_shows_with_the_write() {
        let store = triage_store();
        let record = Record::new();
        let mut shell = triaging(&store, &record);
        pick(&mut shell, 'p', "high");
        shell.write(&record);
        let screen = drawn_sized(&mut shell, 140, 10);
        assert!(
            screen.contains("warning: the Snapshot was not updated"),
            "{screen}"
        );
    }

    #[test]
    fn a_refusal_with_a_warning_keeps_its_start_on_a_narrow_terminal() {
        let store = triage_store();
        let mut record = Record::new();
        let mut shell = triaging(&store, &record);
        shell.key(press(KeyCode::Char(' ')));
        shell.key(press(KeyCode::Char(' ')));
        record.refuse_key = Some("TM-8".into());
        pick(&mut shell, 'p', "high");
        shell.write(&record);
        let screen = drawn_sized(&mut shell, 80, 12);
        let said = screen.lines().rev().nth(1).unwrap_or_default();
        assert!(said.starts_with("\"✗ TM-8: "), "{screen}");
        assert!(said.ends_with("…\""), "cut at its end: {screen}");
        let footer = screen.lines().last().unwrap_or_default();
        assert!(
            footer.starts_with("\"q back"),
            "the keys keep their row: {screen}"
        );
    }

    #[test]
    fn an_undo_says_the_clis_warning() {
        let store = triage_store();
        let record = Record::new();
        let mut shell = triaging(&store, &record);
        pick(&mut shell, 'p', "high");
        shell.write(&record);
        shell.key(press(KeyCode::Char('u')));
        shell.write(&record);
        let screen = drawn_sized(&mut shell, 140, 10);
        assert!(
            screen.contains("✓ undid the write · warning: a backup was not made"),
            "{screen}"
        );
    }

    #[test]
    fn a_cut_counts_terminal_columns_and_keeps_its_ellipsis() {
        assert_eq!(cut("abc", 5), "abc");
        let wide = cut("名前名前名前", 5);
        assert!(columns(&wide) <= 5 && wide.ends_with('…'), "{wide}");
        assert_eq!(wide, "名前…");
    }

    #[test]
    fn the_crumbs_drop_the_oldest_screens_first() {
        let titles = ["Issues".to_owned(), "Triage".to_owned(), "TM-5".to_owned()];
        assert_eq!(
            crumbs("TM", &titles, 80),
            "tm · TM › Issues › Triage › TM-5"
        );
        assert_eq!(crumbs("TM", &titles, 28), "tm · TM › … › Triage › TM-5");
        assert_eq!(crumbs("TM", &titles, 20), "tm · TM › … › TM-5");
        assert_eq!(crumbs("TM", &titles, 8), "… › TM-5");
        let one = ["Issues".to_owned()];
        assert_eq!(crumbs("TM", &one, 12), "Issues");
        for width in 0..30 {
            for listed in [&titles[..], &one[..]] {
                let line = crumbs("TM", listed, width);
                assert!(columns(&line) <= width.max(1), "{width}: {line}");
            }
        }
    }

    #[test]
    fn a_mixed_action_names_what_landed_and_counts_what_did_not_change() {
        let store = triage_store();
        let mut record = Record::new();
        let mut shell = triaging(&store, &record);
        let first = queue_key(&mut shell).unwrap();
        shell.key(press(KeyCode::Char(' ')));
        let second = queue_key(&mut shell).unwrap();
        shell.key(press(KeyCode::Char(' ')));
        record.unchanged_key = Some(first);
        pick(&mut shell, 'p', "high");
        shell.write(&record);
        let screen = drawn_sized(&mut shell, 140, 10);
        let said = format!("✓ {second}  written · 1 unchanged · u undo");
        assert!(screen.contains(&said), "{screen}");
    }

    #[test]
    fn u_after_an_action_that_wrote_nothing_says_it_reaches_back() {
        let store = triage_store();
        let mut record = Record::new();
        let mut shell = triaging(&store, &record);
        pick(&mut shell, 'p', "high");
        shell.write(&record);
        shell.load(&store, &record);
        record.unchanged_key = queue_key(&mut shell);
        pick(&mut shell, 'p', "high");
        shell.write(&record);
        shell.key(press(KeyCode::Char('u')));
        shell.write(&record);
        let screen = drawn_sized(&mut shell, 140, 10);
        assert!(
            screen.contains("✓ the earlier action: undid the write"),
            "{screen}"
        );
    }

    #[test]
    fn every_refusal_and_warning_is_counted() {
        let store = triage_store();
        let record = Record {
            refuse: true,
            ..Record::new()
        };
        let mut shell = triaging(&store, &record);
        shell.key(press(KeyCode::Char(' ')));
        shell.key(press(KeyCode::Char(' ')));
        pick(&mut shell, 'p', "high");
        shell.write(&record);
        let screen = drawn_sized(&mut shell, 140, 10);
        assert!(screen.contains("(+1 more refused)"), "{screen}");
        let items = ["a", "b", "a", "c"].map(str::to_owned);
        assert_eq!(
            first_of(&items, "more warnings"),
            Some("a (+2 more warnings)".to_owned()),
            "a repeat counts once"
        );
    }

    #[test]
    fn a_refused_undo_of_an_earlier_action_says_so() {
        let store = triage_store();
        let mut record = Record::new();
        let mut shell = triaging(&store, &record);
        pick(&mut shell, 'p', "high");
        shell.write(&record);
        shell.load(&store, &record);
        record.unchanged_key = queue_key(&mut shell);
        pick(&mut shell, 'p', "high");
        shell.write(&record);
        record.refuse_undo.set(true);
        shell.key(press(KeyCode::Char('u')));
        shell.write(&record);
        let screen = drawn_sized(&mut shell, 140, 10);
        assert!(
            screen.contains("✗ the earlier action: not undone"),
            "{screen}"
        );
    }

    #[test]
    fn a_detail_that_cannot_be_read_says_so_beside_the_queue() {
        struct Unreadable;

        impl Actions for Unreadable {
            fn me(&self) -> Result<&str, &str> {
                NOBODY
            }

            fn detail(&self, _: &Store, _: Uuid) -> Result<String, String> {
                Err("database is locked".into())
            }

            fn ready(&self, store: &Store) -> Result<String, String> {
                Fake.ready(store)
            }

            fn write(&self, write: &Write) -> Result<Written, String> {
                Fake.write(write)
            }

            fn undo(&self, batch: Uuid) -> Result<Written, String> {
                Fake.undo(batch)
            }
        }

        let store = triage_store();
        let mut shell = triaging(&store, &Unreadable);
        let screen = drawn_sized(&mut shell, 140, 10);
        assert!(screen.contains("✗ cannot show TM-"), "{screen}");
        assert!(screen.contains("database is locked"), "{screen}");
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
        store
            .append(Draft {
                actor: Actor::human("jace"),
                project,
                subject: id,
                change,
            })
            .unwrap();
    }

    fn on_board(store: &Store, actions: &dyn Actions) -> Shell {
        let mut shell = Shell::new("TM");
        shell.load(store, actions);
        assert_eq!(shell.key(press(KeyCode::Char('b'))), Flow::Reload);
        shell.load(store, actions);
        shell
    }

    /// The cursor's column Status and Issue Key.
    fn cursor(shell: &mut Shell) -> (String, Option<String>) {
        match shell.shown() {
            Screen::Board(board) => board.cursor(),
            _ => unreachable!("the Board is shown"),
        }
    }

    fn version(store: &Store, key: &str) -> i64 {
        let id = store.issue_by_key(key).unwrap().unwrap();
        store.issue_with_version(id).unwrap().unwrap().1
    }

    #[test]
    fn b_opens_a_column_per_open_status_and_a_adds_done_and_cancelled() {
        let store = store();
        let record = Record::new();
        let mut shell = on_board(&store, &record);
        let screen = drawn_sized(&mut shell, 120, 10);
        insta::assert_snapshot!(screen);
        for column in ["Triage 0", "Todo 3", "In Progress 0", "In Review 0"] {
            assert!(screen.contains(column), "{column}: {screen}");
        }
        assert!(!screen.contains("Done"), "{screen}");
        assert_eq!(cursor(&mut shell).0, "todo", "opens on the first Issue");
        assert_eq!(shell.key(press(KeyCode::Char('a'))), Flow::Reload);
        shell.load(&store, &record);
        let screen = drawn_sized(&mut shell, 160, 10);
        assert!(
            screen.contains("Done 1") && screen.contains("Cancelled 0"),
            "{screen}"
        );
    }

    #[test]
    fn capital_l_and_h_move_the_issue_beside_and_the_cursor_follows_it() {
        let mut store = store();
        let record = Record::new();
        let mut shell = on_board(&store, &record);
        let (_, key) = cursor(&mut shell);
        let key = key.unwrap();
        assert_eq!(shell.key(press(KeyCode::Char('L'))), Flow::Write);
        shell.write(&record);
        assert_eq!(
            *record.writes.borrow(),
            [Write::Move {
                key: key.clone(),
                status: "in_progress".into(),
                version: version(&store, &key),
            }]
        );
        set(&mut store, &key, "status", "in_progress");
        shell.load(&store, &record);
        assert_eq!(
            cursor(&mut shell),
            ("in_progress".into(), Some(key.clone()))
        );
        assert_eq!(shell.key(press(KeyCode::Char('H'))), Flow::Write);
        shell.write(&record);
        let writes = record.writes.borrow();
        assert!(
            matches!(&writes[1], Write::Move { status, version: v, .. }
                if status == "todo" && *v == version(&store, &key)),
            "{writes:?}"
        );
    }

    #[test]
    fn a_move_past_either_end_of_the_workflow_writes_nothing() {
        let mut store = store();
        let record = Record::new();
        let mut shell = on_board(&store, &record);
        let key = cursor(&mut shell).1.unwrap();
        set(&mut store, &key, "status", "triage");
        shell.load(&store, &record);
        assert_eq!(cursor(&mut shell), ("triage".into(), Some(key.clone())));
        assert_eq!(shell.key(press(KeyCode::Char('H'))), Flow::Stay);
        set(&mut store, &key, "status", "cancelled");
        shell.key(press(KeyCode::Char('a')));
        shell.load(&store, &record);
        assert_eq!(cursor(&mut shell), ("cancelled".into(), Some(key)));
        assert_eq!(shell.key(press(KeyCode::Char('L'))), Flow::Stay);
        assert!(record.writes.borrow().is_empty());
    }

    #[test]
    fn a_refused_move_leaves_the_cursor_on_its_issue() {
        let store = store();
        let record = Record {
            refuse: true,
            ..Record::new()
        };
        let mut shell = on_board(&store, &record);
        shell.key(press(KeyCode::Char('j')));
        let before = cursor(&mut shell);
        shell.key(press(KeyCode::Char('L')));
        shell.write(&record);
        shell.load(&store, &record);
        assert_eq!(cursor(&mut shell), before);
        assert!(drawn_sized(&mut shell, 120, 10).contains("✗ "));
    }

    #[test]
    fn s_moves_the_issue_to_a_status_its_workflow_has() {
        let store = store();
        let record = Record::new();
        let mut shell = on_board(&store, &record);
        let key = cursor(&mut shell).1.unwrap();
        assert_eq!(pick(&mut shell, 's', "review"), Flow::Write);
        shell.write(&record);
        assert_eq!(
            *record.writes.borrow(),
            [Write::Move {
                key: key.clone(),
                status: "in_review".into(),
                version: version(&store, &key),
            }]
        );
    }

    #[test]
    fn columns_past_the_terminal_scroll_into_view_and_say_how_many_are_hidden() {
        let store = store();
        let record = Record::new();
        let mut shell = on_board(&store, &record);
        let screen = drawn_sized(&mut shell, 60, 10);
        assert!(
            screen.contains("Todo 3") && screen.contains(" 2› "),
            "{screen}"
        );
        for _ in 0..3 {
            shell.key(press(KeyCode::Char('l')));
        }
        assert_eq!(cursor(&mut shell), ("in_review".into(), None));
        let screen = drawn_sized(&mut shell, 60, 10);
        assert!(screen.contains("‹2 In Progress 0"), "{screen}");
        assert!(
            screen.contains("In Review 0") && !screen.contains("Todo"),
            "{screen}"
        );
        assert_eq!(shell.key(press(KeyCode::Char('L'))), Flow::Stay, "no Issue");
    }

    #[test]
    fn enter_opens_the_issue_under_the_cursor() {
        let store = store();
        let record = Record::new();
        let mut shell = on_board(&store, &record);
        shell.key(press(KeyCode::Char('G')));
        let key = cursor(&mut shell).1.unwrap();
        assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Reload);
        shell.load(&store, &record);
        assert!(drawn_sized(&mut shell, 120, 10).contains(&format!("Board › {key}")));
        assert_eq!(shell.key(press(KeyCode::Char('q'))), Flow::Reload);
        shell.load(&store, &record);
        assert_eq!(cursor(&mut shell).1, Some(key), "back where it was");
    }

    #[test]
    fn scrolled_columns_come_back_as_the_cursor_or_the_terminal_widens() {
        let store = store();
        let record = Record::new();
        let mut shell = on_board(&store, &record);
        for _ in 0..3 {
            shell.key(press(KeyCode::Char('l')));
        }
        drawn_sized(&mut shell, 60, 10);
        let wide = drawn_sized(&mut shell, 120, 10);
        assert!(wide.contains("Triage 0") && !wide.contains('‹'), "{wide}");
        drawn_sized(&mut shell, 60, 10);
        for _ in 0..3 {
            shell.key(press(KeyCode::Char('h')));
        }
        let back = drawn_sized(&mut shell, 60, 10);
        assert!(back.contains("Todo 3") && !back.contains('‹'), "{back}");
    }

    #[test]
    fn a_gone_issue_leaves_the_cursor_in_its_column_and_a_hidden_column_sends_it_to_work() {
        let mut store = store();
        let record = Record::new();
        let mut shell = on_board(&store, &record);
        shell.key(press(KeyCode::Char('G')));
        let gone = cursor(&mut shell).1.unwrap();
        set(&mut store, &gone, "status", "done");
        shell.load(&store, &record);
        let (column, now) = cursor(&mut shell);
        assert_eq!(column, "todo");
        assert!(now.is_some_and(|key| key != gone));
        shell.key(press(KeyCode::Char('a')));
        shell.load(&store, &record);
        for _ in 0..5 {
            shell.key(press(KeyCode::Char('l')));
        }
        assert_eq!(cursor(&mut shell).0, "cancelled");
        shell.key(press(KeyCode::Char('a')));
        shell.load(&store, &record);
        assert_eq!(
            cursor(&mut shell).0,
            "todo",
            "the first column with an Issue"
        );
    }

    #[test]
    fn the_cursor_follows_its_issue_when_an_issue_above_it_leaves() {
        let mut store = store();
        let record = Record::new();
        let mut shell = on_board(&store, &record);
        let first = cursor(&mut shell).1.unwrap();
        shell.key(press(KeyCode::Char('j')));
        let kept = cursor(&mut shell).1;
        set(&mut store, &first, "status", "done");
        shell.load(&store, &record);
        assert_eq!(cursor(&mut shell).1, kept);
    }

    #[test]
    fn a_column_keeps_its_row_while_the_cursor_is_elsewhere() {
        let store = store();
        let record = Record::new();
        let mut shell = on_board(&store, &record);
        shell.key(press(KeyCode::Char('j')));
        let left_on = cursor(&mut shell).1;
        shell.key(press(KeyCode::Char('l')));
        shell.load(&store, &record);
        shell.key(press(KeyCode::Char('h')));
        assert_eq!(cursor(&mut shell).1, left_on);
    }

    #[test]
    fn moves_follow_the_issues_own_workflow_not_the_columns() {
        let mut scheme = tm_core::scheme::Scheme::built_in();
        let default = scheme.workflows[0].clone();
        let lean = default
            .statuses
            .iter()
            .filter(|status| ["todo", "in_review", "done"].contains(&status.id.as_str()))
            .cloned()
            .collect();
        scheme.workflows.push(tm_core::scheme::Workflow {
            name: "lean".into(),
            start: "todo".into(),
            statuses: lean,
            ..default
        });
        let bug = scheme
            .types
            .iter_mut()
            .find(|issue| issue.id == "bug")
            .unwrap();
        bug.workflow = "lean".into();
        let mut store = store().with_scheme(scheme);
        let jace = Actor::human("jace");
        let project = store.project_by_key("TM").unwrap().unwrap();
        let (id, _) = store.create_issue(&jace, project, "bug", "lean").unwrap();
        let key = store
            .issue_with_version(id)
            .unwrap()
            .unwrap()
            .0
            .key
            .unwrap();
        let record = Record::new();
        let mut shell = on_board(&store, &record);
        for _ in 0..4 {
            if cursor(&mut shell).1.as_deref() == Some(key.as_str()) {
                break;
            }
            shell.key(press(KeyCode::Char('j')));
        }
        assert_eq!(cursor(&mut shell).1, Some(key));
        assert_eq!(
            shell.key(press(KeyCode::Char('H'))),
            Flow::Stay,
            "no triage"
        );
        assert_eq!(pick(&mut shell, 's', "progress"), Flow::Stay, "not offered");
        assert_eq!(shell.key(press(KeyCode::Char('L'))), Flow::Write);
        shell.write(&record);
        assert!(
            matches!(&record.writes.borrow()[..], [Write::Move { status, .. }] if status == "in_review"),
            "{:?}",
            record.writes.borrow()
        );
    }
}
