//! The TUI's screens, opened by `tm tui` (ADR 0018).

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

/// How often a write elsewhere is looked for between keys (ADR 0018).
const POLL: Duration = Duration::from_millis(250);

const KEYS: [(&str, &str); 4] = [
    ("q", "quit"),
    ("Ctrl-C", "quit"),
    ("?", "show or hide these keys"),
    ("Esc", "hide these keys"),
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

/// The frame around every screen: where it reads from, the keys, and what
/// the last look for writes elsewhere found.
pub struct Shell {
    place: String,
    help: bool,
    changed: Option<String>,
    busy: bool,
}

/// Whether the TUI goes on after a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Flow {
    Stay,
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
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Flow {
        if key.kind != KeyEventKind::Press {
            return Flow::Stay;
        }
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Flow::Quit,
            KeyCode::Char('q') => Flow::Quit,
            KeyCode::Char('?') => {
                self.help = !self.help;
                Flow::Stay
            }
            KeyCode::Esc => {
                self.help = false;
                Flow::Stay
            }
            _ => Flow::Stay,
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

    pub fn render(&self, frame: &mut Frame) {
        let [top, body, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        frame.render_widget(Line::from(format!("tm · {}", self.place)), top);
        frame.render_widget(
            Paragraph::new("Nothing to show yet.").block(Block::bordered()),
            body,
        );
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
        frame.render_widget(Line::from("q quit · ? keys"), keys);
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
                .centered(Constraint::Length(36), Constraint::Length(height));
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(lines).block(Block::bordered().title("Keys")),
                area,
            );
        }
    }
}

/// Runs the TUI until a key quits it. Between keys `look` says what it found
/// of writes elsewhere; its error ends the TUI. The terminal is restored on
/// return, and on panic even where a panic aborts.
///
/// # Errors
///
/// A terminal failure, starting it included, or `look`'s error.
pub fn run<E: From<io::Error>>(
    place: impl Into<String>,
    look: impl FnMut() -> Result<Seen, E>,
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
    let ran = match ratatui::try_init() {
        Ok(mut terminal) => {
            let ran = drive(&mut terminal, Shell::new(place), look, &signal);
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
    mut look: impl FnMut() -> Result<Seen, E>,
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
            && shell.key(key) == Flow::Quit
        {
            return Ok(Ended::Quit);
        }
        shell.seen(look()?);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyEventState;

    use super::*;

    fn drawn_at(shell: &Shell, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 10)).unwrap();
        terminal.draw(|frame| shell.render(frame)).unwrap();
        terminal.backend().to_string()
    }

    fn drawn(shell: &Shell) -> String {
        drawn_at(shell, 44)
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn the_frame_names_its_place_and_keys() {
        let mut shell = Shell::new("TM");
        insta::assert_snapshot!(drawn(&shell));
        shell.seen(Seen::Changed("10:15:03".into()));
        assert!(drawn(&shell).contains("changed elsewhere 10:15:03"));
    }

    #[test]
    fn a_busy_store_shows_until_a_look_gets_through() {
        let mut shell = Shell::new("TM");
        shell.seen(Seen::Changed("10:15:03".into()));
        shell.seen(Seen::Busy);
        assert!(drawn(&shell).contains("store busy, retrying"));
        shell.seen(Seen::Nothing);
        let after = drawn(&shell);
        assert!(!after.contains("busy"), "{after}");
        assert!(after.contains("changed elsewhere 10:15:03"), "{after}");
    }

    #[test]
    fn a_narrow_footer_keeps_the_status_whole_beside_the_keys() {
        let mut shell = Shell::new("TM");
        shell.seen(Seen::Changed("10:15:03".into()));
        let footer = drawn_at(&shell, 34).lines().last().unwrap().to_owned();
        assert!(footer.contains("changed elsewhere 10:15:03"), "{footer}");
        assert!(!footer.contains("quchanged"), "{footer}");
    }

    #[test]
    fn question_mark_shows_the_keys_and_esc_hides_them() {
        let mut shell = Shell::new("TM");
        assert_eq!(shell.key(press(KeyCode::Char('?'))), Flow::Stay);
        insta::assert_snapshot!(drawn(&shell));
        assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay);
        assert!(!drawn(&shell).contains("Keys"));
        shell.key(press(KeyCode::Char('?')));
        shell.key(press(KeyCode::Char('?')));
        assert!(!drawn(&shell).contains("Keys"));
    }

    #[test]
    fn q_and_ctrl_c_quit_and_a_release_does_nothing() {
        let mut shell = Shell::new("TM");
        assert_eq!(shell.key(press(KeyCode::Char('q'))), Flow::Quit);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(shell.key(ctrl_c), Flow::Quit);
        assert_eq!(shell.key(press(KeyCode::Char('c'))), Flow::Stay);
        let release = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
            KeyEventState::NONE,
        );
        assert_eq!(shell.key(release), Flow::Stay);
    }

    #[test]
    fn no_size_panics() {
        let mut shell = Shell::new("TM");
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
