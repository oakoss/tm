//! The Issue list: `tm list` on a screen, with the filter typed in place.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};
use tm_core::filter::{Context, FilterError, open_unless_named};
use tm_core::text::printable;
use tm_store::{IssueView, Store, StoreError};
use uuid::Uuid;

use crate::board::Board;
use crate::cursor;
use crate::issue::IssueScreen;
use crate::keys::{self, Binding, ch, plain};
use crate::read;
use crate::ready::Ready;
use crate::triage::Triage;
use crate::{Outcome, Screen};

/// Rows a page key moves.
const PAGE: usize = 10;

#[derive(Clone, Copy)]
pub(crate) enum Act {
    Quit,
    Down,
    Up,
    First,
    Last,
    PageDown,
    PageUp,
    Open,
    Filter,
    All,
    Ready,
    Triage,
    Board,
}

pub(crate) const KEYS: [Binding<Act>; 13] = [
    Binding {
        keys: &[ch('q')],
        action: Act::Quit,
        does: "quit",
        hint: Some("quit"),
    },
    Binding {
        keys: &[ch('j'), plain(KeyCode::Down)],
        action: Act::Down,
        does: "down and up",
        hint: None,
    },
    Binding {
        keys: &[ch('k'), plain(KeyCode::Up)],
        action: Act::Up,
        does: "down and up",
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
    Binding {
        keys: &[plain(KeyCode::Enter)],
        action: Act::Open,
        does: "open the Issue",
        hint: None,
    },
    Binding {
        keys: &[ch('/')],
        action: Act::Filter,
        does: "type a filter, as tm list; Enter applies",
        hint: Some("filter"),
    },
    Binding {
        keys: &[ch('a')],
        action: Act::All,
        does: "open work or every status",
        hint: None,
    },
    Binding {
        keys: &[ch('r')],
        action: Act::Ready,
        does: "the ready queue, as tm ready",
        hint: Some("ready"),
    },
    Binding {
        keys: &[ch('t')],
        action: Act::Triage,
        does: "triage the Issues in a triage Status",
        hint: Some("triage"),
    },
    Binding {
        keys: &[ch('b')],
        action: Act::Board,
        does: "the Board: a column per Status",
        hint: Some("board"),
    },
];

#[derive(Default)]
pub(crate) struct List {
    filter: String,
    all: bool,
    /// The filter being typed, while it is.
    typing: Option<String>,
    rows: Vec<Shown>,
    total: usize,
    table: TableState,
    /// Why the last read showed nothing, such as a filter that does not parse.
    problem: Option<String>,
}

/// One Issue as a row shows it, its text already safe for the terminal.
struct Shown {
    id: Uuid,
    key: String,
    status: String,
    priority: String,
    title: String,
}

impl Shown {
    fn of(view: &IssueView) -> Self {
        Self {
            id: view.id,
            key: read::key(view),
            status: read::field(view, "status"),
            priority: read::field(view, "priority"),
            title: read::field(view, "title"),
        }
    }
}

impl List {
    /// Reads the Issues the filter matches, as `tm list` does, `me` standing
    /// for `@me`, or why no one can. The selected Issue stays selected while
    /// it still matches.
    pub(crate) fn load(&mut self, store: &Store, me: Result<&str, &str>) {
        let filter = open_unless_named(&self.filter, self.all);
        let read = store.read(|store| {
            let ids = store.find(&filter, Context { me: me.ok() })?;
            let views = store.issues(&ids)?;
            Ok::<_, StoreError>((ids.len(), views))
        });
        let kept = self.selected_key();
        match read {
            Ok((total, views)) => {
                self.rows = views.iter().map(Shown::of).collect();
                self.total = total;
                self.problem = None;
            }
            Err(err) => {
                self.rows.clear();
                self.total = 0;
                let said = match (&err, me) {
                    (StoreError::Filter(FilterError::NoIdentity { .. }), Err(why)) => {
                        format!("@me needs to know who you are: {why}")
                    }
                    _ => err.to_string(),
                };
                self.problem = Some(printable(&said));
            }
        }
        let keys = self.rows.iter().map(|row| row.key.as_str());
        let was = self.table.selected().unwrap_or(0);
        self.table
            .select(cursor::reselect(keys, kept.as_deref(), was));
    }

    pub(crate) fn selected_key(&self) -> Option<String> {
        self.selected().map(|(_, key)| key)
    }

    /// The selected Issue's id and Key.
    fn selected(&self) -> Option<(Uuid, String)> {
        self.table
            .selected()
            .and_then(|at| self.rows.get(at))
            .map(|row| (row.id, row.key.clone()))
    }

    /// Acts on `key`: while a filter is typed it is text, else as [`KEYS`]
    /// says.
    pub(crate) fn key(&mut self, key: KeyEvent) -> Outcome {
        if let Some(typed) = &mut self.typing {
            match key.code {
                KeyCode::Enter => {
                    self.filter = std::mem::take(typed);
                    self.typing = None;
                    return Outcome::Reload;
                }
                KeyCode::Esc => self.typing = None,
                KeyCode::Backspace => {
                    typed.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    typed.push(c);
                }
                _ => {}
            }
            return Outcome::Stay;
        }
        let Some(act) = keys::action(&KEYS, key) else {
            return Outcome::Stay;
        };
        let last = self.rows.len().saturating_sub(1);
        let at = self.table.selected().unwrap_or(0);
        let to = match act {
            Act::Quit => return Outcome::Quit,
            Act::Filter => {
                self.typing = Some(self.filter.clone());
                return Outcome::Stay;
            }
            Act::All => {
                self.all = !self.all;
                return Outcome::Reload;
            }
            Act::Ready => return Outcome::Open(Box::new(Screen::Ready(Ready::new()))),
            Act::Triage => return Outcome::Open(Box::new(Screen::Triage(Triage::default()))),
            Act::Board => return Outcome::Open(Box::new(Screen::Board(Board::default()))),
            Act::Open => {
                return self.selected().map_or(Outcome::Stay, |(id, key)| {
                    Outcome::Open(Box::new(Screen::Issue(IssueScreen::new(id, key))))
                });
            }
            Act::Down => at.saturating_add(1).min(last),
            Act::Up => at.saturating_sub(1),
            Act::First => 0,
            Act::Last => last,
            Act::PageDown => at.saturating_add(PAGE).min(last),
            Act::PageUp => at.saturating_sub(PAGE),
        };
        if !self.rows.is_empty() {
            self.table.select(Some(to));
        }
        Outcome::Stay
    }

    /// The footer's text while a filter is typed.
    #[must_use]
    pub(crate) fn prompt(&self) -> Option<String> {
        self.typing
            .as_ref()
            .map(|typed| format!("/{}", printable(typed)))
    }

    pub(crate) fn render(&mut self, frame: &mut Frame, area: Rect) {
        let mut title = if self.filter.is_empty() {
            String::from("open Issues")
        } else {
            format!("filter {}", printable(&self.filter))
        };
        if self.all {
            title.push_str(", all statuses");
        }
        let block = Block::bordered()
            .title(Line::from(format!(" {title} ")))
            .title(Line::from(format!(" {} ", self.count())).right_aligned());
        if let Some(problem) = &self.problem {
            frame.render_widget(Paragraph::new(problem.as_str()).block(block), area);
            return;
        }
        if self.rows.is_empty() {
            frame.render_widget(Paragraph::new("No Issues match.").block(block), area);
            return;
        }
        let width = |of: fn(&Shown) -> &str| {
            let widest = self.rows.iter().map(|row| of(row).chars().count()).max();
            u16::try_from(widest.unwrap_or(0)).unwrap_or(u16::MAX)
        };
        let widths = [
            Constraint::Length(width(|row| &row.key)),
            Constraint::Length(width(|row| &row.status)),
            Constraint::Length(width(|row| &row.priority)),
            Constraint::Fill(1),
        ];
        let rows = self.rows.iter().map(|row| {
            Row::new([
                Cell::from(row.key.as_str()),
                Cell::from(row.status.as_str()),
                Cell::from(row.priority.as_str()),
                Cell::from(row.title.as_str()),
            ])
        });
        let table = Table::new(rows, widths)
            .column_spacing(2)
            .block(block)
            .row_highlight_style(Style::new().reversed());
        frame.render_stateful_widget(table, area, &mut self.table);
    }

    fn count(&self) -> String {
        match self.total {
            1 => "1 Issue".to_owned(),
            total => format!("{total} Issues"),
        }
    }
}
