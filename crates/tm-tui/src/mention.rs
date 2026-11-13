//! Picks the Actor each `@word` of a saved comment means when it names
//! several, before the comment is written (ADR 0019).

use std::path::PathBuf;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, List as Choices, ListState};
use tm_core::mention::Mentioned;
use tm_core::text::printable;

use crate::{Ambiguous, Write};

/// The last choice, after the candidates.
const AS_TEXT: &str = "none: leave it as text";

/// What a key in the picker did.
pub(crate) enum Picked {
    Open,
    /// Esc: nothing is written and the draft is kept.
    Cancelled(PathBuf),
    /// Every mention is answered: the comment to write, and its draft.
    Done(Write, PathBuf),
}

pub(crate) struct MentionPicker {
    key: String,
    body: String,
    /// The editor's file, kept until the comment is written.
    draft: PathBuf,
    ambiguous: Vec<Ambiguous>,
    picks: Vec<Option<Mentioned>>,
    at: ListState,
}

impl MentionPicker {
    pub(crate) fn new(
        key: String,
        body: String,
        draft: PathBuf,
        ambiguous: Vec<Ambiguous>,
    ) -> Self {
        Self {
            key,
            body,
            draft,
            ambiguous,
            picks: Vec::new(),
            at: ListState::default().with_selected(Some(0)),
        }
    }

    pub(crate) const fn draft(&self) -> &PathBuf {
        &self.draft
    }

    fn asking(&self) -> Option<&Ambiguous> {
        self.ambiguous.get(self.picks.len())
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Picked {
        let Some(asking) = self.asking() else {
            return Picked::Open;
        };
        let last = asking.candidates.len();
        match key.code {
            KeyCode::Esc => Picked::Cancelled(self.draft.clone()),
            KeyCode::Down | KeyCode::Char('j') => {
                let at = self.at.selected().map_or(0, |at| (at + 1).min(last));
                self.at.select(Some(at));
                Picked::Open
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let at = self.at.selected().map_or(0, |at| at.saturating_sub(1));
                self.at.select(Some(at));
                Picked::Open
            }
            KeyCode::Enter => {
                let at = self.at.selected().unwrap_or(0);
                let pick = asking.candidates.get(at).map(|(actor, _)| actor.clone());
                self.picks.push(pick);
                self.at.select(Some(0));
                if self.asking().is_some() {
                    return Picked::Open;
                }
                let write = Write::Comment {
                    key: self.key.clone(),
                    body: self.body.clone(),
                    picks: std::mem::take(&mut self.picks),
                };
                Picked::Done(write, self.draft.clone())
            }
            _ => Picked::Open,
        }
    }

    pub(crate) fn prompt() -> String {
        "↑↓ choose · Enter takes it · Esc writes nothing, keeping the comment".to_owned()
    }

    pub(crate) fn render(&mut self, frame: &mut Frame, area: Rect) {
        let Some(asking) = self.asking() else { return };
        // A tab passes `printable` but would move a cell's cursor.
        let safe = |text: &str| printable(text).replace('\t', " ");
        let title = format!(" @{} names more than one ", safe(&asking.typed));
        let mut items: Vec<String> = asking
            .candidates
            .iter()
            .map(|(_, label)| safe(label))
            .collect();
        items.push(AS_TEXT.to_owned());
        let wide = items
            .iter()
            .map(|item| item.chars().count())
            .chain([title.chars().count()])
            .max()
            .unwrap_or(0);
        let width = u16::try_from(wide + 4).unwrap_or(u16::MAX).min(area.width);
        let height = u16::try_from(items.len() + 2)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let popup = area.centered(Constraint::Length(width), Constraint::Length(height));
        let choices = Choices::new(items)
            .block(Block::bordered().title(Line::from(title)))
            .highlight_style(Style::new().reversed());
        frame.render_widget(Clear, popup);
        frame.render_stateful_widget(choices, popup, &mut self.at);
    }
}
