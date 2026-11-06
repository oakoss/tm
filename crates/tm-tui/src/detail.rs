//! One Issue as `tm show` and `tm history` print it, scrolled in place.

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

pub(crate) struct Detail {
    id: Uuid,
    key: String,
    /// The text, or why it could not be read.
    text: Result<String, String>,
    scroll: u16,
    /// The furthest `scroll` reaches at the last size drawn.
    bottom: u16,
}

impl Detail {
    pub(crate) const fn new(id: Uuid, key: String) -> Self {
        Self {
            id,
            key,
            text: Ok(String::new()),
            scroll: 0,
            bottom: 0,
        }
    }

    /// Reads the Issue again, keeping the place scrolled to.
    pub(crate) fn load(&mut self, store: &Store, actions: &dyn Actions) {
        // A tab passes `printable` but would move a screen cell's cursor.
        let safe = |text: &str| {
            let lines = text
                .lines()
                .map(|line| printable(line).replace('\t', "    "));
            lines.collect::<Vec<_>>().join("\n")
        };
        self.text = actions
            .detail(store, self.id)
            .map(|text| safe(&text))
            .map_err(|why| safe(&why));
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
        let block = Block::bordered().title(Line::from(format!(" {} ", printable(&self.key))));
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
