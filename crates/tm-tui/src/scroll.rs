//! Text scrolled in place, no further than its last line shows.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Paragraph, Wrap};

/// Rows a page key scrolls.
const PAGE: u16 = 10;

#[derive(Clone, Copy)]
pub(crate) enum Move {
    Down,
    Up,
    First,
    Last,
    PageDown,
    PageUp,
}

#[derive(Default)]
pub(crate) struct Scroll {
    at: u16,
    /// The furthest `at` reaches at the last size drawn.
    bottom: u16,
}

impl Scroll {
    pub(crate) fn go(&mut self, to: Move) {
        self.at = match to {
            Move::Down => self.at.saturating_add(1),
            Move::Up => self.at.saturating_sub(1),
            Move::First => 0,
            Move::Last => self.bottom,
            Move::PageDown => self.at.saturating_add(PAGE),
            Move::PageUp => self.at.saturating_sub(PAGE),
        }
        .min(self.bottom);
    }

    /// Draws `paragraph`, wrapped, inside `block` over `area`, scrolled to
    /// where it was as far as this size allows.
    pub(crate) fn render(
        &mut self,
        frame: &mut Frame,
        paragraph: Paragraph,
        block: Block,
        area: Rect,
    ) {
        let inner = block.inner(area);
        let paragraph = paragraph.wrap(Wrap { trim: false });
        // Counted without the block, whose side borders `line_count` ignores.
        let rows = paragraph.line_count(inner.width);
        self.bottom =
            u16::try_from(rows.saturating_sub(usize::from(inner.height))).unwrap_or(u16::MAX);
        self.at = self.at.min(self.bottom);
        frame.render_widget(paragraph.block(block).scroll((self.at, 0)), area);
    }
}
