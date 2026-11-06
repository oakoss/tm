//! The TUI's screens, opened by `tm tui` (ADR 0018).

mod list;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::flag;
use tm_store::Store;

use list::{Done, List};

/// How often a write elsewhere is looked for between keys (ADR 0018).
const POLL: Duration = Duration::from_millis(250);

const KEYS: [(&str, &str); 10] = [
    ("j k", "down and up; arrows too"),
    ("g G", "first and last"),
    ("PgDn", "down a page; PgUp up"),
    ("/", "type a filter, as tm list"),
    ("Enter", "apply the filter typed"),
    ("a", "open work or every status"),
    ("q", "quit"),
    ("Ctrl-C", "quit"),
    ("?", "show or hide these keys"),
    ("Esc", "hide these keys, or the filter"),
];

/// What a look for writes elsewhere found.
#[derive(Debug, PartialEq, Eq)]
pub enum Seen {
    Nothing,
    /// Another process wrote; the time it was seen.
    Changed(String),
    /// Another process holds the write lock, so nothing could be read.
    Busy,
}

/// The frame around the Issue list: where it reads from, the keys, and what
/// the last look for writes elsewhere found.
pub struct Shell {
    place: String,
    help: bool,
    changed: Option<String>,
    busy: bool,
    list: List,
}

/// What the TUI does after a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Flow {
    Stay,
    /// Read the list again: its filter changed.
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
            list: List::default(),
        }
    }

    /// Reads the list from `store`, `me` standing for `@me`, or why no one can.
    pub fn load(&mut self, store: &Store, me: Result<&str, &str>) {
        self.list.load(store, me);
    }

    pub fn key(&mut self, key: KeyEvent) -> Flow {
        if key.kind != KeyEventKind::Press {
            return Flow::Stay;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Flow::Quit;
        }
        // While a filter is typed, every other key is text.
        if !self.list.typing() {
            match key.code {
                KeyCode::Char('q') => return Flow::Quit,
                KeyCode::Char('?') => {
                    self.help = !self.help;
                    return Flow::Stay;
                }
                KeyCode::Esc if self.help => {
                    self.help = false;
                    return Flow::Stay;
                }
                _ => {}
            }
        }
        match self.list.key(key) {
            Done::Reload => Flow::Reload,
            Done::Handled | Done::Passed => Flow::Stay,
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
        self.list.render(frame, body);
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
        let left = self
            .list
            .prompt()
            .unwrap_or_else(|| "q quit · / filter · ? keys".to_owned());
        frame.render_widget(Line::from(left), keys);
        if let Some(status) = status {
            frame.render_widget(Line::from(status).right_aligned(), right);
        }
        if self.help {
            let lines: Vec<Line> = KEYS
                .iter()
                .map(|(key, does)| Line::from(format!("{key:>6}  {does}")))
                .collect();
            let height = u16::try_from(lines.len()).unwrap_or(u16::MAX) + 2;
            let area = frame
                .area()
                .centered(Constraint::Length(42), Constraint::Length(height));
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
    me: Result<&str, &str>,
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
    shell.load(&store, me);
    let ran = match ratatui::try_init() {
        Ok(mut terminal) => {
            let ran = drive(&mut terminal, shell, (store, me), look, &signal);
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
    (mut store, me): (Store, Result<&str, &str>),
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
                Flow::Reload => shell.load(&store, me),
                Flow::Stay => {}
            }
        }
        let seen = look(&mut store)?;
        if matches!(seen, Seen::Changed(_)) {
            shell.load(&store, me);
        }
        shell.seen(seen);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyEventState;
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
        shell.load(&store, NOBODY);
        let screen = drawn(&mut shell);
        insta::assert_snapshot!(screen);
        assert!(!screen.contains("finished"), "done is not open work");
        assert!(!screen.contains('\u{1b}'), "{screen}");
    }

    #[test]
    fn a_typed_filter_and_every_status_change_what_is_read() {
        let store = store();
        let mut shell = Shell::new("TM");
        shell.load(&store, NOBODY);
        assert_eq!(shell.key(press(KeyCode::Char('/'))), Flow::Stay);
        assert_eq!(
            shell.key(press(KeyCode::Char('q'))),
            Flow::Stay,
            "q is text"
        );
        shell.key(press(KeyCode::Backspace));
        assert_eq!(typed(&mut shell, "title:first"), Flow::Reload);
        shell.load(&store, NOBODY);
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
        shell.load(&store, NOBODY);
        assert!(drawn(&mut shell).contains("finished"));
    }

    #[test]
    fn esc_drops_a_typed_filter_and_a_bad_one_says_why() {
        let store = store();
        let mut shell = Shell::new("TM");
        shell.load(&store, NOBODY);
        shell.key(press(KeyCode::Char('/')));
        shell.key(press(KeyCode::Char('x')));
        assert!(drawn(&mut shell).contains("/x"));
        assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay);
        shell.load(&store, NOBODY);
        let dropped = drawn(&mut shell);
        assert!(
            !dropped.contains("/x") && dropped.contains("open Issues"),
            "{dropped}"
        );
        assert!(dropped.contains("3 Issues"), "{dropped}");
        assert_eq!(typed_after_slash(&mut shell, "nonsense:1"), Flow::Reload);
        shell.load(&store, NOBODY);
        assert!(drawn(&mut shell).contains("unknown field"));
    }

    #[test]
    fn at_me_reads_the_person_or_says_why_no_one_can() {
        let store = store();
        let mut shell = Shell::new("TM");
        typed_after_slash(&mut shell, "assignee:@me");
        shell.load(&store, Ok("jace"));
        let mine = drawn(&mut shell);
        assert!(
            mine.contains("third") && mine.contains("1 Issue "),
            "{mine}"
        );
        shell.load(&store, NOBODY);
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
        shell.load(&store, NOBODY);
        shell.key(press(KeyCode::Char('G')));
        assert_eq!(shell.list.selected_key().as_deref(), Some("TM-1"));
        typed_after_slash(&mut shell, "title:third");
        shell.load(&store, NOBODY);
        assert_eq!(shell.list.selected_key().as_deref(), Some("TM-3"));
    }

    #[test]
    fn while_typing_question_mark_is_text_and_a_control_key_is_not() {
        let mut shell = Shell::new("TM");
        shell.key(press(KeyCode::Char('/')));
        assert_eq!(shell.key(press(KeyCode::Char('?'))), Flow::Stay);
        let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(shell.key(ctrl_w), Flow::Stay);
        assert_eq!(shell.list.prompt().as_deref(), Some("/?"));
        assert!(!drawn_sized(&mut shell, 60, 16).contains("Keys"));
    }

    fn typed_after_slash(shell: &mut Shell, text: &str) -> Flow {
        shell.key(press(KeyCode::Char('/')));
        while shell.list.prompt().is_some_and(|prompt| prompt.len() > 1) {
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
        shell.load(&store, NOBODY);
        let selected = |shell: &Shell| shell.list.selected_key();
        assert_eq!(selected(&shell).as_deref(), Some("TM-3"), "the first row");
        shell.key(press(KeyCode::Char('G')));
        shell.key(press(KeyCode::Up));
        assert_eq!(selected(&shell).as_deref(), Some("TM-2"));
        let jace = Actor::human("jace");
        let project = store.project_by_key("TM").unwrap().unwrap();
        store
            .create_issue(&jace, project, "task", "zeroth")
            .unwrap();
        shell.load(&store, NOBODY);
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
        insta::assert_snapshot!(drawn_sized(&mut shell, 60, 16));
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
        shell.load(&store, NOBODY);
        shell.seen(Seen::Changed("10:15:03".into()));
        for help in [false, true] {
            if help {
                shell.key(press(KeyCode::Char('?')));
            }
            for width in 0..50 {
                for height in 0..12 {
                    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                    terminal.draw(|frame| shell.render(frame)).unwrap();
                }
            }
        }
    }
}
