//! One Issue laid out from what `tm show` and `tm history` report: its
//! fields, description and comments as Markdown, its latest changes, and
//! the Issues it relates to, each opened with Enter.

mod data;
mod layout;

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List as Rows, ListState, Paragraph, Wrap};
use serde_json::Value;
use tm_core::text::printable;
use tm_store::{Store, StoreError};
use uuid::Uuid;

pub use data::{
    Change, Claim, Comment, History, Issue, LATEST_WRITES, Link, Logged, Related, Shown, Status,
};
pub(crate) use layout::body;

use crate::keys::{self, Binding, ch, plain};
use crate::picker::{Choice, Field, Picked, Picker, assignees, people, priorities, writes};
use crate::scroll::{Move, Scroll};
use crate::{Actions, Outcome, Screen, cursor, read};

/// Columns from which the related Issues sit beside the text, not below.
const WIDE: u16 = 100;

#[derive(Clone, Copy)]
pub(crate) enum Act {
    Back,
    Scroll(Move),
    Next,
    Previous,
    Open,
    Status,
    Priority,
    Assign,
    Undo,
}

pub(crate) const KEYS: [Binding<Act>; 14] = [
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
    Binding {
        keys: &[plain(KeyCode::Tab), ch('n')],
        action: Act::Next,
        does: "the next and previous related Issue",
        hint: Some("related"),
    },
    Binding {
        keys: &[plain(KeyCode::BackTab), ch('N')],
        action: Act::Previous,
        does: "the next and previous related Issue",
        hint: None,
    },
    Binding {
        keys: &[plain(KeyCode::Enter)],
        action: Act::Open,
        does: "open the related Issue",
        hint: Some("open"),
    },
    Binding {
        keys: &[ch('s')],
        action: Act::Status,
        does: "move to a Status, as tm move",
        hint: Some("status"),
    },
    Binding {
        keys: &[ch('p')],
        action: Act::Priority,
        does: "set the priority, as tm edit",
        hint: Some("priority"),
    },
    Binding {
        keys: &[ch('A')],
        action: Act::Assign,
        does: "assign, as tm edit",
        hint: Some("assign"),
    },
    Binding {
        keys: &[ch('u')],
        action: Act::Undo,
        does: "undo this session's last write, as tm undo",
        hint: Some("undo"),
    },
];

/// An Issue this one relates to, as a row shows it.
struct Row {
    id: Uuid,
    key: String,
    /// How it relates: `parent`, `child`, or a Link's name.
    how: String,
    status: String,
    title: String,
}

pub(crate) struct IssueScreen {
    id: Uuid,
    key: String,
    /// The Issue as read, or why it could not be.
    read: Result<Issue, String>,
    rows: Vec<Row>,
    /// Why the related Issues could not be read.
    trouble: Option<String>,
    at: ListState,
    scroll: Scroll,
    /// Now, as of the last read, for the times shown as ages.
    now_ms: i64,
    picker: Option<Picker>,
    /// Who `@me` is, offered first when assigning.
    me: Option<String>,
    /// People assigned open work, offered when assigning.
    people: Vec<String>,
}

impl IssueScreen {
    pub(crate) fn new(id: Uuid, key: String) -> Self {
        Self {
            id,
            key,
            read: Err("not read yet".to_owned()),
            rows: Vec::new(),
            trouble: None,
            at: ListState::default(),
            scroll: Scroll::default(),
            now_ms: 0,
            picker: None,
            me: None,
            people: Vec::new(),
        }
    }

    pub(crate) fn prompt(&self) -> Option<String> {
        self.picker.as_ref().map(Picker::prompt)
    }

    pub(crate) fn title(&self) -> String {
        printable(&self.key)
    }

    /// Reads the Issue again, keeping the place scrolled to and the related
    /// Issue selected.
    pub(crate) fn load(&mut self, store: &Store, actions: &dyn Actions) {
        self.now_ms = now_ms();
        self.read = actions.issue(store, self.id);
        self.me = actions.me().ok().map(str::to_owned);
        // Only the assignee picker offers them, and it takes a name typed.
        self.people = store.read(people).unwrap_or_default();
        let kept = self.selected().map(|row| row.key.clone());
        let rows = match &self.read {
            Ok(issue) => related(store, &issue.shown),
            Err(_) => Ok(Vec::new()),
        };
        (self.rows, self.trouble) = match rows {
            Ok(rows) => (rows, None),
            Err(err) => (Vec::new(), Some(printable(&err.to_string()))),
        };
        let keys = self.rows.iter().map(|row| row.key.as_str());
        let was = self.at.selected().unwrap_or(0);
        self.at.select(cursor::reselect(keys, kept.as_deref(), was));
    }

    #[cfg(test)]
    pub(crate) fn selected_key(&self) -> Option<String> {
        self.selected().map(|row| row.key.clone())
    }

    fn selected(&self) -> Option<&Row> {
        self.at.selected().and_then(|at| self.rows.get(at))
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Outcome {
        if let Some(picker) = &mut self.picker {
            return match picker.key(key) {
                Picked::Open => Outcome::Stay,
                Picked::Cancelled => {
                    self.picker = None;
                    Outcome::Stay
                }
                Picked::Chosen(field, chosen, targets) => {
                    self.picker = None;
                    Outcome::Write(writes(field, &chosen, targets))
                }
            };
        }
        let Some(act) = keys::action(&KEYS, key) else {
            return Outcome::Stay;
        };
        let last = self.rows.len().saturating_sub(1);
        let row = self.at.selected().unwrap_or(0);
        match act {
            Act::Back => return Outcome::Back,
            Act::Undo => return Outcome::Undo,
            Act::Status => self.open(Field::Status),
            Act::Priority => self.open(Field::Priority),
            Act::Assign => self.open(Field::Assignee),
            Act::Open => {
                return self.selected().map_or(Outcome::Stay, |row| {
                    Outcome::Open(Box::new(Screen::Issue(Self::new(row.id, row.key.clone()))))
                });
            }
            Act::Next if !self.rows.is_empty() => {
                self.at.select(Some(if row >= last { 0 } else { row + 1 }));
            }
            Act::Previous if !self.rows.is_empty() => {
                self.at.select(Some(if row == 0 { last } else { row - 1 }));
            }
            Act::Next | Act::Previous => {}
            Act::Scroll(to) => self.scroll.go(to),
        }
        Outcome::Stay
    }

    fn open(&mut self, field: Field) {
        let Ok(issue) = &self.read else {
            return;
        };
        let shown = &issue.shown;
        let choices = match field {
            Field::Status => shown
                .moves_to
                .iter()
                .map(|status| Choice {
                    value: Some(status.id.clone()),
                    label: printable(&status.label),
                })
                .collect(),
            Field::Priority => priorities(),
            Field::Assignee => assignees(self.me.as_deref(), &self.people),
        };
        let key = shown.key.clone().unwrap_or_else(|| shown.id.to_string());
        self.picker = Some(Picker::new(field, choices, vec![(key, shown.version)]));
    }

    pub(crate) fn render(&mut self, frame: &mut Frame, whole: Rect) {
        let (text_area, rows_area) = if self.rows.is_empty() && self.trouble.is_none() {
            (whole, None)
        } else if whole.width >= WIDE {
            let [text, rows] =
                Layout::horizontal([Constraint::Fill(2), Constraint::Fill(1)]).areas(whole);
            (text, Some(rows))
        } else {
            let height = u16::try_from(self.rows.len())
                .unwrap_or(u16::MAX)
                .saturating_add(2);
            let [text, rows] =
                Layout::vertical([Constraint::Fill(1), Constraint::Max(height.min(8))])
                    .areas(whole);
            (text, Some(rows))
        };
        let block = Block::bordered().title(Line::from(format!(" {} ", self.title())));
        let inner = block.inner(text_area);
        let lines = match &self.read {
            Ok(issue) => body(issue, inner.width, self.now_ms),
            Err(why) => vec![Line::from(format!(
                "✗ cannot show {}: {}",
                self.title(),
                printable(why)
            ))],
        };
        // Lines laid out above still wrap here if one is too wide, as a long
        // title or a code line can be, rather than being cut at the edge.
        self.scroll
            .render(frame, Paragraph::new(lines), block, text_area);
        if let Some(area) = rows_area {
            self.render_rows(frame, area);
        }
        if let Some(picker) = &mut self.picker {
            picker.render(frame, whole);
        }
    }

    fn render_rows(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered().title(Line::from(" Related "));
        if let Some(trouble) = &self.trouble {
            let said = format!("✗ cannot read the related Issues: {trouble}");
            let paragraph = Paragraph::new(said).wrap(Wrap { trim: false });
            frame.render_widget(paragraph.block(block), area);
            return;
        }
        let how = self
            .rows
            .iter()
            .map(|row| row.how.chars().count())
            .max()
            .unwrap_or(0);
        let shown: Vec<Line> = self
            .rows
            .iter()
            .map(|row| {
                Line::from(vec![
                    Span::styled(
                        format!("{:<how$}  ", row.how),
                        Style::new().add_modifier(Modifier::DIM),
                    ),
                    Span::raw(format!("{}  {}  {}", row.key, row.status, row.title)),
                ])
            })
            .collect();
        let rows = Rows::new(shown)
            .block(block)
            .highlight_style(Style::new().reversed());
        frame.render_stateful_widget(rows, area, &mut self.at);
    }
}

/// Now, in Unix milliseconds, for the times a screen shows as ages.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}

/// The Issues `shown` relates to: its parent, its children, then its Links,
/// each with its Status and title read from `store`.
fn related(store: &Store, shown: &Shown) -> Result<Vec<Row>, StoreError> {
    store.read(|store| {
        // With the Key `tm show` reported, for a row the store cannot find.
        let mut wanted: Vec<(String, Uuid, Option<String>)> = Vec::new();
        if let Some(parent) = &shown.parent {
            wanted.push(("parent".to_owned(), parent.id, parent.key.clone()));
        }
        // Archived ones too, as `tm show` counts them.
        let children = store.children(shown.id)?;
        wanted.extend(
            children
                .iter()
                .map(|child| ("child".to_owned(), child.id, child.key.clone())),
        );
        for link in &shown.links {
            wanted.push((link.name.clone(), link.other.id, link.other.key.clone()));
        }
        let ids: Vec<Uuid> = wanted.iter().map(|(_, id, _)| *id).collect();
        let views = store.issues(&ids)?;
        let rows = wanted.into_iter().map(|(how, id, key)| {
            let how = printable(&how);
            let Some(view) = views.iter().find(|view| view.id == id) else {
                let key = printable(&key.unwrap_or_else(|| id.to_string()));
                return Row {
                    id,
                    key,
                    how,
                    status: "missing".to_owned(),
                    title: String::new(),
                };
            };
            let mut status = read::field(view, "status");
            if view.fields.get("archived").and_then(Value::as_bool) == Some(true) {
                status.push_str(", archived");
            }
            Row {
                id,
                key: read::key(view),
                how,
                status,
                title: read::field(view, "title"),
            }
        });
        Ok(rows.collect())
    })
}
