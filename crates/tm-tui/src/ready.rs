//! The ready queue as `tm ready` prints it, scrolled in place.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use tm_core::text::printable;
use tm_store::Store;

use crate::keys::{self, Binding, ch, plain};
use crate::scroll::{Move, Scroll};
use crate::{Actions, Outcome};

#[derive(Clone, Copy)]
pub(crate) enum Act {
    Back,
    Scroll(Move),
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
        action: Act::Scroll(Move::Down),
        does: "scroll down and up",
        hint: Some("scroll"),
    },
    Binding {
        keys: &[ch('k'), plain(KeyCode::Up)],
        action: Act::Scroll(Move::Up),
        does: "scroll down and up",
        hint: Some("scroll"),
    },
    Binding {
        keys: &[ch('g'), plain(KeyCode::Home)],
        action: Act::Scroll(Move::First),
        does: "top and bottom",
        hint: None,
    },
    Binding {
        keys: &[ch('G'), plain(KeyCode::End)],
        action: Act::Scroll(Move::Last),
        does: "top and bottom",
        hint: None,
    },
    Binding {
        keys: &[plain(KeyCode::PageDown)],
        action: Act::Scroll(Move::PageDown),
        does: "down a page and up",
        hint: None,
    },
    Binding {
        keys: &[plain(KeyCode::PageUp)],
        action: Act::Scroll(Move::PageUp),
        does: "down a page and up",
        hint: None,
    },
];

pub(crate) struct Ready {
    /// The text, or why it could not be read.
    text: Result<String, String>,
    scroll: Scroll,
}

impl Ready {
    pub(crate) fn new() -> Self {
        Self {
            text: Ok(String::new()),
            scroll: Scroll::default(),
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
        let read = actions.ready(store);
        self.text = read.map(|text| safe(&text)).map_err(|why| safe(&why));
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Outcome {
        match keys::action(&KEYS, key) {
            Some(Act::Back) => Outcome::Back,
            Some(Act::Scroll(to)) => {
                self.scroll.go(to);
                Outcome::Stay
            }
            None => Outcome::Stay,
        }
    }

    pub(crate) fn render(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered().title(Line::from(" Ready "));
        let text = match &self.text {
            Ok(text) | Err(text) => text.as_str(),
        };
        self.scroll.render(frame, Paragraph::new(text), block, area);
    }
}
