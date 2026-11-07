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
use tm_core::scheme::PRIORITY_RANKS;
use tm_core::text::printable;
use tm_store::{Store, StoreError};
use uuid::Uuid;

use crate::detail::Detail;
use crate::keys::{self, Binding, ch, plain};
use crate::picker::{Choice, Field, Picked, Picker, shared, statuses, writes};
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
    /// The selected Issue as `tm show` and `tm history` print it.
    detail: Option<Result<String, String>>,
}

impl Triage {
    /// Whether a picker takes the keys as text.
    pub(crate) const fn typing(&self) -> bool {
        self.picker.is_some()
    }

    pub(crate) fn prompt(&self) -> Option<String> {
        self.picker.as_ref().map(Picker::prompt)
    }

    pub(crate) fn load(&mut self, store: &Store, actions: &dyn Actions) {
        let me = actions.me();
        self.me = me.ok().map(str::to_owned);
        let read = store.read(|store| {
            let ids = store.find(QUEUE, Context { me: me.ok() })?;
            let mut rows = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some((view, version)) = store.issue_with_version(id)? {
                    let field = |name: &str| {
                        let text = view.fields.get(name).and_then(|value| value.as_str());
                        printable(text.unwrap_or("-")).replace('\t', " ")
                    };
                    rows.push(Shown {
                        id,
                        key: view.key.clone().unwrap_or_else(|| id.to_string()),
                        version,
                        issue_type: field("type"),
                        priority: field("priority"),
                        assignee: field("assignee"),
                        title: field("title"),
                    });
                }
            }
            let assigned =
                store.find("has:assignee -category:done,cancelled", Context::default())?;
            let mut people = BTreeSet::new();
            for view in store.issues(&assigned)? {
                if let Some(person) = view.fields.get("assignee").and_then(|value| value.as_str()) {
                    people.insert(person.to_owned());
                }
            }
            Ok::<_, StoreError>((rows, people))
        });
        let kept = self.then.take().or_else(|| self.selected_key());
        match read {
            Ok((rows, people)) => {
                self.statuses = statuses(store, rows.iter().map(|row| row.issue_type.as_str()));
                self.rows = rows;
                self.people = people.into_iter().collect();
                self.problem = None;
            }
            Err(err) => {
                self.rows.clear();
                self.problem = Some(printable(&err.to_string()));
            }
        }
        self.marked
            .retain(|key| self.rows.iter().any(|row| &row.key == key));
        let at = kept
            .and_then(|key| self.rows.iter().position(|row| row.key == key))
            .unwrap_or_else(|| self.table.selected().unwrap_or(0));
        self.table
            .select((!self.rows.is_empty()).then(|| at.min(self.rows.len() - 1)));
        let safe = |text: &str| {
            let lines = text
                .lines()
                .map(|line| printable(line).replace('\t', "    "));
            lines.collect::<Vec<_>>().join("\n")
        };
        self.detail = self.selected().map(|row| {
            actions
                .detail(store, row.id)
                .map(|text| safe(&text))
                .map_err(|why| safe(&why))
        });
    }

    /// The writes asked for are done: the Issues `landed` are unmarked, and
    /// when one was `refused` the cursor stays where it was.
    pub(crate) fn wrote(&mut self, landed: &[String], refused: bool) {
        self.marked.retain(|key| !landed.contains(key));
        if refused {
            self.then = None;
        }
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
                Picked::Chosen(field, chosen, targets) => {
                    self.picker = None;
                    self.write(field, &chosen, targets)
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
                    Outcome::Open(Box::new(Screen::Detail(Detail::issue(
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
            Field::Priority => PRIORITY_RANKS
                .iter()
                .map(|(id, _)| Choice {
                    value: Some((*id).to_owned()),
                    label: (*id).to_owned(),
                })
                .collect(),
            Field::Assignee => {
                let mut choices: Vec<Choice> = Vec::new();
                if let Some(me) = &self.me {
                    choices.push(Choice {
                        value: Some(me.clone()),
                        label: format!("{} (@me)", printable(me)),
                    });
                }
                for person in &self.people {
                    if self.me.as_ref() != Some(person) {
                        choices.push(Choice {
                            value: Some(person.clone()),
                            label: printable(person),
                        });
                    }
                }
                choices.push(Choice {
                    value: None,
                    label: "nobody".to_owned(),
                });
                choices
            }
        };
        self.picker = Some(Picker::new(field, choices, captured));
        Outcome::Stay
    }

    /// The writes `chosen` asks of `targets`, and the row to go on to once
    /// they land: the next one not written, else the one before.
    fn write(&mut self, field: Field, chosen: &Choice, targets: Vec<(String, i64)>) -> Outcome {
        if targets.is_empty() {
            return Outcome::Stay;
        }
        let written: BTreeSet<&str> = targets.iter().map(|(key, _)| key.as_str()).collect();
        let at = self.table.selected().unwrap_or(0);
        let free = |row: &&Shown| !written.contains(row.key.as_str());
        self.then = self.rows[at.min(self.rows.len())..]
            .iter()
            .find(free)
            .or_else(|| self.rows[..at.min(self.rows.len())].iter().rev().find(free))
            .map(|row| row.key.clone());
        Outcome::Write(writes(field, chosen, targets))
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
        let text = match detail {
            Ok(text) => text.clone(),
            Err(why) => format!("✗ cannot show {title}: {why}\nMove the cursor to read it again."),
        };
        let shown = Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(Line::from(format!(" {title} "))));
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
