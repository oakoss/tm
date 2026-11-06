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

/// Rows a page key moves.
const PAGE: usize = 10;

/// What a key did to the list.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Done {
    /// The filter changed, so the list must be read again.
    Reload,
    Handled,
    /// Not a list key.
    Passed,
}

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
        let field = |name: &str| {
            let text = view
                .fields
                .get(name)
                .and_then(|value| value.as_str())
                .unwrap_or("-");
            // A tab passes `printable` but would move a screen cell's cursor.
            printable(text).replace('\t', " ")
        };
        Self {
            id: view.id,
            key: view.key.clone().unwrap_or_else(|| view.id.to_string()),
            status: field("status"),
            priority: field("priority"),
            title: field("title"),
        }
    }
}

impl List {
    /// Whether a filter is being typed, so keys are text, not commands.
    #[must_use]
    pub(crate) const fn typing(&self) -> bool {
        self.typing.is_some()
    }

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
        let at = kept
            .and_then(|key| self.rows.iter().position(|row| row.key == key))
            .unwrap_or_else(|| self.table.selected().unwrap_or(0));
        self.table
            .select((!self.rows.is_empty()).then(|| at.min(self.rows.len() - 1)));
    }

    pub(crate) fn selected_key(&self) -> Option<String> {
        self.selected().map(|(_, key)| key)
    }

    /// The selected Issue's id and Key.
    pub(crate) fn selected(&self) -> Option<(Uuid, String)> {
        self.table
            .selected()
            .and_then(|at| self.rows.get(at))
            .map(|row| (row.id, row.key.clone()))
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Done {
        if let Some(typed) = &mut self.typing {
            match key.code {
                KeyCode::Enter => {
                    self.filter = std::mem::take(typed);
                    self.typing = None;
                    return Done::Reload;
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
            return Done::Handled;
        }
        let last = self.rows.len().saturating_sub(1);
        let at = self.table.selected().unwrap_or(0);
        let to = match key.code {
            KeyCode::Char('/') => {
                self.typing = Some(self.filter.clone());
                return Done::Handled;
            }
            KeyCode::Char('a') => {
                self.all = !self.all;
                return Done::Reload;
            }
            KeyCode::Char('j') | KeyCode::Down => at.saturating_add(1).min(last),
            KeyCode::Char('k') | KeyCode::Up => at.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => 0,
            KeyCode::Char('G') | KeyCode::End => last,
            KeyCode::PageDown => at.saturating_add(PAGE).min(last),
            KeyCode::PageUp => at.saturating_sub(PAGE),
            _ => return Done::Passed,
        };
        if !self.rows.is_empty() {
            self.table.select(Some(to));
        }
        Done::Handled
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
