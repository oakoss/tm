//! The TUI's screens, opened by `tm tui` (ADR 0018).

mod detail;
mod keys;
mod list;
mod stack;

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

use detail::Detail;
use keys::{Binding, ch, ctrl, plain};
use list::List;
use stack::Stack;

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
}

/// What a key on a screen asks of the Shell.
pub(crate) enum Outcome {
    Stay,
    /// Read the screen again, as its filter changed.
    Reload,
    /// Show this screen over the current one.
    Open(Screen),
    /// Go back to the screen below.
    Back,
    Quit,
}

impl Screen {
    fn load(&mut self, store: &Store, actions: &dyn Actions) {
        match self {
            Self::List(list) => list.load(store, actions.me()),
            Self::Detail(detail) => detail.load(store, actions),
        }
    }

    fn key(&mut self, key: KeyEvent) -> Outcome {
        match self {
            Self::List(list) => list.key(key),
            Self::Detail(detail) => detail.key(key),
        }
    }

    /// Whether keys are text being typed, which takes every key but Ctrl-C.
    const fn typing(&self) -> bool {
        matches!(self, Self::List(list) if list.typing())
    }

    fn help(&self) -> Vec<(String, &'static str)> {
        match self {
            Self::List(_) => keys::help(&list::KEYS),
            Self::Detail(_) => keys::help(&detail::KEYS),
        }
    }

    /// The footer: the text typed, else the screen's hints.
    fn footer(&self) -> String {
        if let Self::List(list) = self
            && let Some(prompt) = list.prompt()
        {
            return prompt;
        }
        let mut hints = match self {
            Self::List(_) => keys::hints(&list::KEYS),
            Self::Detail(_) => keys::hints(&detail::KEYS),
        };
        hints.extend(keys::hints(&GLOBAL));
        hints.join(" · ")
    }

    fn render(&mut self, frame: &mut Frame, area: ratatui::layout::Rect) {
        match self {
            Self::List(list) => list.render(frame, area),
            Self::Detail(detail) => detail.render(frame, area),
        }
    }
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
}

/// What the TUI does after a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Flow {
    Stay,
    /// Read the screen shown again, as its filter or the screen changed.
    Reload,
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
                self.screens.push(screen);
                Flow::Reload
            }
            Outcome::Back if self.screens.pop() => Flow::Reload,
            Outcome::Stay | Outcome::Back => Flow::Stay,
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
        let [top, body, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        frame.render_widget(Line::from(format!("tm · {}", self.place)), top);
        self.shown().render(frame, body);
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
            Screen::Detail(_) => unreachable!("the list is first"),
        }
    }

    struct Gone;

    impl Actions for Gone {
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
}
