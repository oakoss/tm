//! Triage: the Issues in a triage Status, each moved, prioritized or
//! assigned with one key, the cursor going on to the next.

use std::collections::BTreeSet;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState, Wrap};
use tm_core::filter::Context;
use tm_core::text::printable;
use tm_store::{Store, StoreError};
use uuid::Uuid;

use crate::cursor;
use crate::issue::{Issue, IssueScreen, body, now_ms};
use crate::keys::{self, Binding, ch, plain};
use crate::picker::{
    Choice, Field, Picked, Picker, assignees, people, priorities, shared, statuses,
};
use crate::read;
use crate::{Actions, Outcome, Screen};

/// Rows a page key moves.
const PAGE: usize = 10;

const QUEUE: &str = "category:triage";

/// Columns from which the selected Issue sits beside the queue, not below.
const WIDE: u16 = 100;

#[derive(Clone, Copy)]
pub(crate) enum Act {
    Back,
    Down,
    Up,
    First,
    Last,
    PageDown,
    PageUp,
    Open,
    Mark,
    Status,
    Priority,
    Assign,
    Undo,
}

pub(crate) const KEYS: [Binding<Act>; 13] = [
    Binding {
        keys: &[ch('q'), plain(KeyCode::Esc), plain(KeyCode::Backspace)],
        action: Act::Back,
        does: "back",
        hint: Some("back"),
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
        keys: &[ch(' ')],
        action: Act::Mark,
        does: "mark and go down; s p A act on the marked",
        hint: Some("mark"),
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

/// One Issue as a row shows it, its text already safe for the terminal.
struct Shown {
    id: Uuid,
    key: String,
    version: i64,
    issue_type: String,
    priority: String,
    assignee: String,
    title: String,
}

#[derive(Default)]
pub(crate) struct Triage {
    rows: Vec<Shown>,
    table: TableState,
    /// Keys marked for the next action.
    marked: BTreeSet<String>,
    picker: Option<Picker>,
    /// The Key to select once a write lands and the queue is read again.
    then: Option<String>,
    /// Who `@me` is, offered first when assigning.
    me: Option<String>,
    /// People assigned open work, offered when assigning.
    people: Vec<String>,
    /// Statuses per Issue type, offered when moving.
    statuses: Vec<(String, Vec<Choice>)>,
    problem: Option<String>,
    /// The selected Issue as the Issue screen lays it out, or why it cannot
    /// be read.
    detail: Option<Result<Issue, String>>,
    /// Now, as of the last read, for the times shown as ages.
    now_ms: i64,
}

impl Triage {
    pub(crate) fn prompt(&self) -> Option<String> {
        self.picker.as_ref().map(Picker::prompt)
    }

    pub(crate) fn load(&mut self, store: &Store, actions: &dyn Actions) {
        let me = actions.me();
        self.me = me.ok().map(str::to_owned);
        let queue = read::in_project(QUEUE, actions);
        let read = store.read(|store| {
            let rows: Vec<Shown> = read::issues(store, &queue, Context { me: me.ok() })?
                .into_iter()
                .map(|issue| Shown {
                    issue_type: issue.field("type"),
                    priority: issue.field("priority"),
                    assignee: issue.field("assignee"),
                    title: issue.field("title"),
                    id: issue.id,
                    key: issue.key,
                    version: issue.version,
                })
                .collect();
            Ok::<_, StoreError>((rows, people(store)?))
        });
        let kept = self.then.take().or_else(|| self.selected_key());
        match read {
            Ok((rows, people)) => {
                self.statuses = statuses(store, rows.iter().map(|row| row.issue_type.as_str()));
                self.rows = rows;
                self.people = people;
                self.problem = None;
            }
            Err(err) => {
                self.rows.clear();
                self.problem = Some(printable(&err.to_string()));
            }
        }
        self.marked
            .retain(|key| self.rows.iter().any(|row| &row.key == key));
        let keys = self.rows.iter().map(|row| row.key.as_str());
        let was = self.table.selected().unwrap_or(0);
        self.table
            .select(cursor::reselect(keys, kept.as_deref(), was));
        self.now_ms = now_ms();
        self.detail = self.selected().map(|row| actions.issue(store, row.id));
    }

    /// The writes to `attempted` are done: the Issues `landed` are unmarked,
    /// and the cursor goes on past them unless one was `refused`.
    pub(crate) fn wrote(&mut self, attempted: &[String], landed: &[String], refused: bool) {
        self.marked.retain(|key| !landed.contains(key));
        self.then = if refused || attempted.is_empty() {
            None
        } else {
            self.after(attempted)
        };
    }

    pub(crate) fn selected_key(&self) -> Option<String> {
        self.selected().map(|row| row.key.clone())
    }

    fn selected(&self) -> Option<&Shown> {
        self.table.selected().and_then(|at| self.rows.get(at))
    }

    /// The rows an action takes: the marked ones, else the selected one.
    fn targets(&self) -> Vec<&Shown> {
        if self.marked.is_empty() {
            self.selected().into_iter().collect()
        } else {
            self.rows
                .iter()
                .filter(|row| self.marked.contains(&row.key))
                .collect()
        }
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Outcome {
        if let Some(picker) = &mut self.picker {
            return match picker.key(key) {
                Picked::Open => Outcome::Stay,
                Picked::Cancelled => {
                    self.picker = None;
                    Outcome::Stay
                }
                Picked::Chosen(writes) => {
                    self.picker = None;
                    Outcome::Write(writes)
                }
            };
        }
        let Some(act) = keys::action(&KEYS, key) else {
            return Outcome::Stay;
        };
        let last = self.rows.len().saturating_sub(1);
        let at = self.table.selected().unwrap_or(0);
        let to = match act {
            Act::Back => return Outcome::Back,
            Act::Undo => return Outcome::Undo,
            Act::Open => {
                return self.selected().map_or(Outcome::Stay, |row| {
                    Outcome::Open(Box::new(Screen::Issue(IssueScreen::new(
                        row.id,
                        row.key.clone(),
                    ))))
                });
            }
            Act::Mark => {
                if let Some(key) = self.selected_key()
                    && !self.marked.remove(&key)
                {
                    self.marked.insert(key);
                }
                at.saturating_add(1).min(last)
            }
            Act::Status => return self.open(Field::Status),
            Act::Priority => return self.open(Field::Priority),
            Act::Assign => return self.open(Field::Assignee),
            Act::Down => at.saturating_add(1).min(last),
            Act::Up => at.saturating_sub(1),
            Act::First => 0,
            Act::Last => last,
            Act::PageDown => at.saturating_add(PAGE).min(last),
            Act::PageUp => at.saturating_sub(PAGE),
        };
        if self.rows.is_empty() || Some(to) == self.table.selected() {
            return Outcome::Stay;
        }
        self.table.select(Some(to));
        // The detail beside the queue follows the cursor.
        Outcome::Reload
    }

    fn open(&mut self, field: Field) -> Outcome {
        let targets = self.targets();
        if targets.is_empty() {
            return Outcome::Stay;
        }
        let types: BTreeSet<&str> = targets.iter().map(|row| row.issue_type.as_str()).collect();
        let captured = targets
            .iter()
            .map(|row| (row.key.clone(), row.version))
            .collect();
        let choices = match field {
            Field::Status => shared(&self.statuses, &types),
            Field::Priority => priorities(),
            Field::Assignee => assignees(self.me.as_deref(), &self.people),
        };
        self.picker = Some(Picker::new(field, choices, captured));
        Outcome::Stay
    }

    /// The row the cursor goes on to once `attempted` are written: the first
    /// other at or below it, else the nearest above.
    fn after(&self, attempted: &[String]) -> Option<String> {
        let written: BTreeSet<&str> = attempted.iter().map(String::as_str).collect();
        let at = self.table.selected().unwrap_or(0).min(self.rows.len());
        let free = |row: &&Shown| !written.contains(row.key.as_str());
        self.rows[at..]
            .iter()
            .find(free)
            .or_else(|| self.rows[..at].iter().rev().find(free))
            .map(|row| row.key.clone())
    }

    /// Draws the selected Issue beside the queue on a wide terminal and
    /// below it on a narrow one, and returns the queue's area.
    fn render_detail(&self, frame: &mut Frame, whole: Rect) -> Rect {
        let Some(detail) = &self.detail else {
            return whole;
        };
        let split = if whole.width >= WIDE {
            Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)])
        } else {
            Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)])
        };
        let [area, beside] = split.areas(whole);
        let title = self
            .selected_key()
            .map(|key| printable(&key))
            .unwrap_or_default();
        let block = Block::bordered().title(Line::from(format!(" {title} ")));
        let lines = match detail {
            Ok(issue) => body(issue, block.inner(beside).width, self.now_ms),
            Err(why) => vec![
                Line::from(format!("✗ cannot show {title}: {}", printable(why))),
                Line::from("Move the cursor to read it again."),
            ],
        };
        let shown = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block);
        frame.render_widget(shown, beside);
        area
    }

    pub(crate) fn render(&mut self, frame: &mut Frame, whole: Rect) {
        let area = self.render_detail(frame, whole);
        let count = match self.rows.len() {
            1 => "1 Issue".to_owned(),
            n => format!("{n} Issues"),
        };
        let marked = if self.marked.is_empty() {
            String::new()
        } else {
            format!(", {} marked", self.marked.len())
        };
        let block = Block::bordered()
            .title(Line::from(" Triage "))
            .title(Line::from(format!(" {count}{marked} ")).right_aligned());
        if let Some(problem) = &self.problem {
            frame.render_widget(Paragraph::new(problem.as_str()).block(block), area);
        } else if self.rows.is_empty() {
            frame.render_widget(Paragraph::new("Nothing to triage.").block(block), area);
        } else {
            let width = |of: fn(&Shown) -> &str| {
                let widest = self.rows.iter().map(|row| of(row).chars().count()).max();
                u16::try_from(widest.unwrap_or(0)).unwrap_or(u16::MAX)
            };
            let widths = [
                Constraint::Length(1),
                Constraint::Length(width(|row| &row.key)),
                Constraint::Length(width(|row| &row.priority)),
                Constraint::Length(width(|row| &row.assignee)),
                Constraint::Fill(1),
            ];
            let rows = self.rows.iter().map(|row| {
                let mark = if self.marked.contains(&row.key) {
                    "*"
                } else {
                    " "
                };
                Row::new([
                    Cell::from(mark),
                    Cell::from(row.key.as_str()),
                    Cell::from(row.priority.as_str()),
                    Cell::from(row.assignee.as_str()),
                    Cell::from(row.title.as_str()),
                ])
            });
            let table = Table::new(rows, widths)
                .column_spacing(2)
                .block(block)
                .row_highlight_style(Style::new().reversed());
            frame.render_stateful_widget(table, area, &mut self.table);
        }
        if let Some(picker) = &mut self.picker {
            picker.render(frame, whole);
        }
    }
}
