//! A command's text scrolled in place: one Issue as `tm show` and `tm
//! history` print it, or the ready queue as `tm ready` prints it.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Wrap};
use tm_core::text::printable;
use tm_store::Store;
use uuid::Uuid;

use crate::keys::{self, Binding, ch, plain};
use crate::{Actions, Outcome};

/// Rows a page key scrolls.
const PAGE: u16 = 10;

#[derive(Clone, Copy)]
pub(crate) enum Act {
    Back,
    Down,
    Up,
    First,
    Last,
    PageDown,
    PageUp,
}

pub(crate) const KEYS: [Binding<Act>; 7] = [
    Binding {
        keys: &[ch('q'), plain(KeyCode::Esc), plain(KeyCode::Backspace)],
        action: Act::Back,
        does: "back",
        hint: Some("back"),
    },
    Binding {
        keys: &[ch('j'), plain(KeyCode::Down)],
        action: Act::Down,
        does: "scroll down and up",
        hint: Some("scroll"),
    },
    Binding {
        keys: &[ch('k'), plain(KeyCode::Up)],
        action: Act::Up,
        does: "scroll down and up",
        hint: Some("scroll"),
    },
    Binding {
        keys: &[ch('g'), plain(KeyCode::Home)],
        action: Act::First,
        does: "top and bottom",
        hint: None,
    },
    Binding {
        keys: &[ch('G'), plain(KeyCode::End)],
        action: Act::Last,
        does: "top and bottom",
        hint: None,
    },
    Binding {
        keys: &[plain(KeyCode::PageDown)],
        action: Act::PageDown,
        does: "down a page and up",
        hint: None,
    },
    Binding {
        keys: &[plain(KeyCode::PageUp)],
        action: Act::PageUp,
        does: "down a page and up",
        hint: None,
    },
];

/// What the screen shows.
enum Shows {
    Issue { id: Uuid, key: String },
    Ready,
}

pub(crate) struct Detail {
    shows: Shows,
    /// The text, or why it could not be read.
    text: Result<String, String>,
    scroll: u16,
    /// The furthest `scroll` reaches at the last size drawn.
    bottom: u16,
}

impl Detail {
    pub(crate) const fn issue(id: Uuid, key: String) -> Self {
        Self::new(Shows::Issue { id, key })
    }

    pub(crate) const fn ready() -> Self {
        Self::new(Shows::Ready)
    }

    const fn new(shows: Shows) -> Self {
        Self {
            shows,
            text: Ok(String::new()),
            scroll: 0,
            bottom: 0,
        }
    }

    /// Reads the text again, keeping the place scrolled to.
    pub(crate) fn load(&mut self, store: &Store, actions: &dyn Actions) {
        // A tab passes `printable` but would move a screen cell's cursor.
        let safe = |text: &str| {
            let lines = text
                .lines()
                .map(|line| printable(line).replace('\t', "    "));
            lines.collect::<Vec<_>>().join("\n")
        };
        let read = match &self.shows {
            Shows::Issue { id, .. } => actions.detail(store, *id),
            Shows::Ready => actions.ready(store),
        };
        self.text = read.map(|text| safe(&text)).map_err(|why| safe(&why));
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Outcome {
        let Some(act) = keys::action(&KEYS, key) else {
            return Outcome::Stay;
        };
        self.scroll = match act {
            Act::Back => return Outcome::Back,
            Act::Down => self.scroll.saturating_add(1),
            Act::Up => self.scroll.saturating_sub(1),
            Act::First => 0,
            Act::Last => self.bottom,
            Act::PageDown => self.scroll.saturating_add(PAGE),
            Act::PageUp => self.scroll.saturating_sub(PAGE),
        }
        .min(self.bottom);
        Outcome::Stay
    }

    pub(crate) fn render(&mut self, frame: &mut Frame, area: Rect) {
        let title = match &self.shows {
            Shows::Issue { key, .. } => printable(key),
            Shows::Ready => "Ready".to_owned(),
        };
        let block = Block::bordered().title(Line::from(format!(" {title} ")));
        let text = match &self.text {
            Ok(text) | Err(text) => text.as_str(),
        };
        let inner = block.inner(area);
        let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
        // Counted without the block, whose side borders `line_count` ignores.
        let rows = paragraph.line_count(inner.width);
        self.bottom =
            u16::try_from(rows.saturating_sub(usize::from(inner.height))).unwrap_or(u16::MAX);
        self.scroll = self.scroll.min(self.bottom);
        frame.render_widget(paragraph.block(block).scroll((self.scroll, 0)), area);
    }
}
