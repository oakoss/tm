//! The person's Notifications as `tm notifications --all` lists them, newest
//! first, each opening its Issue or marked read in place.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize as _};
use ratatui::text::Line;
use ratatui::widgets::{Block, List as Rows, ListItem, ListState, Paragraph};
use tm_core::text::printable;
use tm_store::Store;
use uuid::Uuid;

use crate::issue::IssueScreen;
use crate::keys::{self, Binding, ch, plain};
use crate::{Actions, Notice, Outcome, Screen, Write};

#[derive(Clone, Copy)]
pub(crate) enum Act {
    Back,
    Down,
    Up,
    First,
    Last,
    Open,
    Read,
    ReadAll,
    Undo,
}

pub(crate) const KEYS: [Binding<Act>; 9] = [
    Binding {
        keys: &[ch('q'), plain(KeyCode::Esc), plain(KeyCode::Backspace)],
        action: Act::Back,
        does: "back",
        hint: Some("back"),
    },
    Binding {
        keys: &[ch('j'), plain(KeyCode::Down)],
        action: Act::Down,
        does: "move down and up",
        hint: None,
    },
    Binding {
        keys: &[ch('k'), plain(KeyCode::Up)],
        action: Act::Up,
        does: "move down and up",
        hint: None,
    },
    Binding {
        keys: &[ch('g'), plain(KeyCode::Home)],
        action: Act::First,
        does: "first and last",
        hint: None,
    },
    Binding {
        keys: &[ch('G'), plain(KeyCode::End)],
        action: Act::Last,
        does: "first and last",
        hint: None,
    },
    Binding {
        keys: &[plain(KeyCode::Enter)],
        action: Act::Open,
        does: "open its Issue",
        hint: Some("open"),
    },
    Binding {
        keys: &[ch('r')],
        action: Act::Read,
        does: "mark it read; u marks it unread",
        hint: Some("read"),
    },
    Binding {
        keys: &[ch('R')],
        action: Act::ReadAll,
        does: "mark every one read",
        hint: Some("all read"),
    },
    Binding {
        keys: &[ch('u')],
        action: Act::Undo,
        does: "undo this session's last write, as tm undo",
        hint: Some("undo"),
    },
];

#[derive(Default)]
pub(crate) struct Notices {
    rows: Vec<Notice>,
    at: ListState,
    /// Why the last read showed nothing.
    problem: Option<String>,
}

impl Notices {
    /// Reads them again, staying on the one selected while it is listed.
    pub(crate) fn load(&mut self, store: &Store, actions: &dyn Actions) {
        let kept: Option<Uuid> = self.selected().map(|row| row.id);
        match actions.notifications(store) {
            Ok(rows) => {
                self.problem = None;
                // A tab passes `printable` but would move a cell's cursor.
                self.rows = rows
                    .into_iter()
                    .map(|row| Notice {
                        line: printable(&row.line).replace('\t', "    "),
                        key: printable(&row.key),
                        ..row
                    })
                    .collect();
            }
            Err(why) => {
                self.problem = Some(printable(&why));
                self.rows.clear();
            }
        }
        let at = kept
            .and_then(|id| self.rows.iter().position(|row| row.id == id))
            .or_else(|| (!self.rows.is_empty()).then_some(0));
        self.at
            .select(at.map(|at| at.min(self.rows.len().saturating_sub(1))));
    }

    fn selected(&self) -> Option<&Notice> {
        self.at.selected().and_then(|at| self.rows.get(at))
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Outcome {
        let last = self.rows.len().saturating_sub(1);
        match keys::action(&KEYS, key) {
            Some(Act::Back) => Outcome::Back,
            Some(Act::Down) => {
                let at = self.at.selected().map_or(0, |at| (at + 1).min(last));
                self.at.select(Some(at));
                Outcome::Stay
            }
            Some(Act::Up) => {
                let at = self.at.selected().map_or(0, |at| at.saturating_sub(1));
                self.at.select(Some(at));
                Outcome::Stay
            }
            Some(Act::First) => {
                self.at.select(Some(0));
                Outcome::Stay
            }
            Some(Act::Last) => {
                self.at.select(Some(last));
                Outcome::Stay
            }
            Some(Act::Open) => self.selected().map_or(Outcome::Stay, |row| {
                let screen = IssueScreen::new(row.issue, row.key.clone());
                Outcome::Open(Box::new(Screen::Issue(screen)))
            }),
            Some(Act::Read) => match self.selected() {
                Some(row) if !row.read => Outcome::Write(vec![Write::Read {
                    id: row.id.to_string(),
                }]),
                _ => Outcome::Stay,
            },
            Some(Act::ReadAll) if self.rows.iter().any(|row| !row.read) => {
                Outcome::Write(vec![Write::Read { id: "all".into() }])
            }
            Some(Act::Undo) => Outcome::Undo,
            Some(Act::ReadAll) | None => Outcome::Stay,
        }
    }

    pub(crate) fn render(&mut self, frame: &mut Frame, area: Rect) {
        let unread = self.rows.iter().filter(|row| !row.read).count();
        let block = Block::bordered()
            .title(Line::from(" Notifications "))
            .title(Line::from(format!(" {unread} unread ")).right_aligned());
        if let Some(problem) = &self.problem {
            frame.render_widget(Paragraph::new(problem.as_str()).block(block), area);
            return;
        }
        if self.rows.is_empty() {
            let none =
                "No Notifications: a mention of you, or a write past your Claim, lands here.";
            frame.render_widget(Paragraph::new(none).block(block), area);
            return;
        }
        let rows = self.rows.iter().map(|row| {
            let item = ListItem::new(row.line.as_str());
            if row.read { item } else { item.bold() }
        });
        let rows = Rows::new(rows)
            .block(block)
            .highlight_style(Style::new().reversed());
        frame.render_stateful_widget(rows, area, &mut self.at);
    }
}
