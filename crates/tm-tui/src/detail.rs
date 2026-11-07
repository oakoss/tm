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

use crate::Actions;

/// Rows a page key scrolls.
const PAGE: u16 = 10;

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

    /// Scrolls for a scroll key; false for any other key.
    pub(crate) fn key(&mut self, key: KeyEvent) -> bool {
        self.scroll = match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => 0,
            KeyCode::Char('G') | KeyCode::End => self.bottom,
            KeyCode::PageDown => self.scroll.saturating_add(PAGE),
            KeyCode::PageUp => self.scroll.saturating_sub(PAGE),
            _ => return false,
        }
        .min(self.bottom);
        true
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
