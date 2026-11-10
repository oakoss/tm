//! The Board: a Project's Issues in a column per Workflow Status, each
//! moved between columns with the same writes as `tm move`.

use std::collections::BTreeSet;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, List as Cards, ListState, Paragraph};
use tm_core::filter::{Context, open_unless_named};
use tm_core::scheme::{Category, Scheme};
use tm_core::text::printable;
use tm_store::Store;
use uuid::Uuid;

use crate::cursor;
use crate::issue::IssueScreen;
use crate::keys::{self, Binding, ch, plain};
use crate::picker::{Choice, Field, Picked, Picker, statuses, writes};
use crate::read;
use crate::{Actions, Outcome, Screen, Write};

/// Rows a page key moves.
const PAGE: usize = 10;

/// The fewest columns of the terminal a Board column takes.
const COLUMN: u16 = 24;

#[derive(Clone, Copy)]
pub(crate) enum Act {
    Back,
    Down,
    Up,
    Left,
    Right,
    First,
    Last,
    PageDown,
    PageUp,
    Open,
    MoveLeft,
    MoveRight,
    Status,
    All,
    Undo,
}

pub(crate) const KEYS: [Binding<Act>; 15] = [
    Binding {
        keys: &[ch('q'), plain(KeyCode::Esc), plain(KeyCode::Backspace)],
        action: Act::Back,
        does: "back",
        hint: Some("back"),
    },
    Binding {
        keys: &[ch('j'), plain(KeyCode::Down)],
        action: Act::Down,
        does: "down and up a column",
        hint: None,
    },
    Binding {
        keys: &[ch('k'), plain(KeyCode::Up)],
        action: Act::Up,
        does: "down and up a column",
        hint: None,
    },
    Binding {
        keys: &[ch('h'), plain(KeyCode::Left)],
        action: Act::Left,
        does: "the column left and right",
        hint: None,
    },
    Binding {
        keys: &[ch('l'), plain(KeyCode::Right)],
        action: Act::Right,
        does: "the column left and right",
        hint: None,
    },
    Binding {
        keys: &[ch('g'), plain(KeyCode::Home)],
        action: Act::First,
        does: "first and last in the column",
        hint: None,
    },
    Binding {
        keys: &[ch('G'), plain(KeyCode::End)],
        action: Act::Last,
        does: "first and last in the column",
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
        keys: &[ch('H')],
        action: Act::MoveLeft,
        does: "move the Issue to the Status left or right, as tm move",
        hint: Some("move"),
    },
    Binding {
        keys: &[ch('L')],
        action: Act::MoveRight,
        does: "move the Issue to the Status left or right, as tm move",
        hint: Some("move"),
    },
    Binding {
        keys: &[ch('s')],
        action: Act::Status,
        does: "move to a Status, as tm move",
        hint: Some("status"),
    },
    Binding {
        keys: &[ch('a')],
        action: Act::All,
        does: "show or hide the done and cancelled columns",
        hint: None,
    },
    Binding {
        keys: &[ch('u')],
        action: Act::Undo,
        does: "undo this session's last write, as tm undo",
        hint: Some("undo"),
    },
];

/// One Issue as a card shows it, its text already safe for the terminal.
struct Card {
    id: Uuid,
    key: String,
    version: i64,
    issue_type: String,
    status: String,
    title: String,
}

struct Column {
    status: String,
    label: String,
    category: Category,
    cards: Vec<Card>,
    row: ListState,
}

#[derive(Default)]
pub(crate) struct Board {
    /// Whether the done and cancelled columns show.
    all: bool,
    columns: Vec<Column>,
    /// The column the cursor is in.
    column: usize,
    /// The first column drawn, so the one with the cursor stays in view.
    left: usize,
    picker: Option<Picker>,
    /// Statuses per Issue type, in Workflow order.
    statuses: Vec<(String, Vec<Choice>)>,
    problem: Option<String>,
}

/// The Statuses of every Workflow, in order, each once; done and cancelled
/// ones only when `all`.
fn columns(scheme: &Scheme, all: bool) -> Vec<Column> {
    let mut seen = BTreeSet::new();
    scheme
        .workflows
        .iter()
        .flat_map(|workflow| &workflow.statuses)
        .filter(|status| all || !matches!(status.category, Category::Done | Category::Cancelled))
        .filter(|status| seen.insert(status.id.clone()))
        .map(|status| Column {
            status: status.id.clone(),
            label: printable(&status.label),
            category: status.category,
            cards: Vec::new(),
            row: ListState::default(),
        })
        .collect()
}

/// The column a card in `status` goes in: its own, else the first triage
/// one, as a Status the Scheme does not define reads as triage, else the
/// first.
fn place(columns: &[Column], status: &str) -> Option<usize> {
    columns
        .iter()
        .position(|column| column.status == status)
        .or_else(|| {
            columns
                .iter()
                .position(|column| column.category == Category::Triage)
        })
        .or_else(|| (!columns.is_empty()).then_some(0))
}

/// Puts each column's row back from `rows`, as read before, as near as its
/// Issues now allow, and returns the cursor's column: the one holding
/// `kept`, at its row; else `here`, the column it was in; else the first
/// column with an Issue.
fn arrange(
    columns: &mut [Column],
    rows: &[(String, Option<usize>)],
    kept: Option<&str>,
    here: Option<&str>,
) -> usize {
    for column in columns.iter_mut() {
        let row = rows
            .iter()
            .find(|(status, _)| *status == column.status)
            .and_then(|(_, row)| *row)
            .unwrap_or(0);
        column.row.select(cursor::held(column.cards.len(), row));
    }
    let found = kept.and_then(|key| {
        columns.iter().enumerate().find_map(|(at, column)| {
            let row = column.cards.iter().position(|card| card.key == key)?;
            Some((at, row))
        })
    });
    let same = here.and_then(|status| columns.iter().position(|column| column.status == status));
    match (found, same) {
        (Some((column, row)), _) => {
            columns[column].row.select(Some(row));
            column
        }
        (None, Some(column)) => column,
        (None, None) => columns
            .iter()
            .position(|column| !column.cards.is_empty())
            .unwrap_or(0),
    }
}

impl Board {
    pub(crate) fn prompt(&self) -> Option<String> {
        self.picker.as_ref().map(Picker::prompt)
    }

    /// Reads the Issues again, the cursor staying on its Issue wherever it
    /// went.
    pub(crate) fn load(&mut self, store: &Store, actions: &dyn Actions) {
        let kept = self.selected().map(|card| card.key.clone());
        let filter = read::in_project(&open_unless_named("", self.all), actions);
        let me = actions.me().ok();
        let read = read::issues(store, &filter, Context { me }).map(|issues| {
            issues
                .into_iter()
                .map(|issue| Card {
                    issue_type: issue.field("type"),
                    status: issue.field("status"),
                    title: issue.field("title"),
                    id: issue.id,
                    key: issue.key,
                    version: issue.version,
                })
                .collect::<Vec<_>>()
        });
        let cards = match read {
            Ok(cards) => cards,
            // The last Board read stays behind the reason, so the next read
            // finds the cursor's Issue again.
            Err(err) => {
                self.picker = None;
                self.problem = Some(printable(&err.to_string()));
                return;
            }
        };
        let rows: Vec<(String, Option<usize>)> = self
            .columns
            .iter()
            .map(|column| (column.status.clone(), column.row.selected()))
            .collect();
        let here = self
            .columns
            .get(self.column)
            .map(|column| column.status.clone());
        self.columns = columns(store.scheme(), self.all);
        self.statuses = statuses(store, cards.iter().map(|card| card.issue_type.as_str()));
        for card in cards {
            if let Some(at) = place(&self.columns, &card.status) {
                self.columns[at].cards.push(card);
            }
        }
        self.problem = None;
        self.column = arrange(&mut self.columns, &rows, kept.as_deref(), here.as_deref());
    }

    /// The cursor's column Status and Issue Key.
    #[cfg(test)]
    pub(crate) fn cursor(&self) -> (String, Option<String>) {
        let status = self
            .columns
            .get(self.column)
            .map(|column| column.status.clone());
        (
            status.unwrap_or_default(),
            self.selected().map(|card| card.key.clone()),
        )
    }

    fn selected(&self) -> Option<&Card> {
        let column = self.columns.get(self.column)?;
        column.row.selected().and_then(|row| column.cards.get(row))
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
        match act {
            Act::Back => Outcome::Back,
            Act::Undo => Outcome::Undo,
            Act::All => {
                self.all = !self.all;
                Outcome::Reload
            }
            // The reason hides the Issues, so none is moved or opened unseen.
            _ if self.problem.is_some() => Outcome::Stay,
            Act::Open => self.selected().map_or(Outcome::Stay, |card| {
                Outcome::Open(Box::new(Screen::Issue(IssueScreen::new(
                    card.id,
                    card.key.clone(),
                ))))
            }),
            Act::MoveLeft => self.step(false),
            Act::MoveRight => self.step(true),
            Act::Status => {
                let Some(card) = self.selected() else {
                    return Outcome::Stay;
                };
                let choices = self.workflow(&card.issue_type).to_vec();
                let target = vec![(card.key.clone(), card.version)];
                self.picker = Some(Picker::new(Field::Status, choices, target));
                Outcome::Stay
            }
            Act::Left => {
                self.column = self.column.saturating_sub(1);
                Outcome::Stay
            }
            Act::Right => {
                self.column = self
                    .column
                    .saturating_add(1)
                    .min(self.columns.len().saturating_sub(1));
                Outcome::Stay
            }
            Act::Down => self.go(|row, _| row.saturating_add(1)),
            Act::Up => self.go(|row, _| row.saturating_sub(1)),
            Act::First => self.go(|_, _| 0),
            Act::Last => self.go(|_, last| last),
            Act::PageDown => self.go(|row, _| row.saturating_add(PAGE)),
            Act::PageUp => self.go(|row, _| row.saturating_sub(PAGE)),
        }
    }

    /// Moves the cursor within its column to `to(row, last)`.
    fn go(&mut self, to: impl Fn(usize, usize) -> usize) -> Outcome {
        if let Some(column) = self.columns.get_mut(self.column)
            && let Some(last) = column.cards.len().checked_sub(1)
        {
            let row = column.row.selected().unwrap_or(0);
            column.row.select(Some(to(row, last).min(last)));
        }
        Outcome::Stay
    }

    /// `issue_type`'s Statuses, in its Workflow's order.
    fn workflow(&self, issue_type: &str) -> &[Choice] {
        self.statuses
            .iter()
            .find(|(listed, _)| listed == issue_type)
            .map(|(_, choices)| choices.as_slice())
            .unwrap_or_default()
    }

    /// Moves the selected Issue to the Status beside its own in its
    /// Workflow; nothing past either end.
    fn step(&self, right: bool) -> Outcome {
        let Some(card) = self.selected() else {
            return Outcome::Stay;
        };
        let order = self.workflow(&card.issue_type);
        let Some(now) = order
            .iter()
            .position(|choice| choice.value.as_deref() == Some(card.status.as_str()))
        else {
            return Outcome::Stay;
        };
        let next = if right {
            now.checked_add(1)
        } else {
            now.checked_sub(1)
        };
        match next.and_then(|next| order.get(next)) {
            Some(to) => Outcome::Write(vec![Write::Move {
                key: card.key.clone(),
                status: to.value.clone().unwrap_or_default(),
                version: card.version,
            }]),
            None => Outcome::Stay,
        }
    }

    pub(crate) fn render(&mut self, frame: &mut Frame, whole: Rect) {
        if let Some(problem) = &self.problem {
            let block = Block::bordered().title(Line::from(" Board "));
            frame.render_widget(Paragraph::new(problem.as_str()).block(block), whole);
            return;
        }
        if self.columns.is_empty() {
            let block = Block::bordered().title(Line::from(" Board "));
            frame.render_widget(Paragraph::new("No Statuses to show.").block(block), whole);
            return;
        }
        let fit = usize::from((whole.width / COLUMN).max(1)).min(self.columns.len());
        if self.column < self.left {
            self.left = self.column;
        } else if self.column >= self.left + fit {
            self.left = self.column + 1 - fit;
        }
        self.left = self.left.min(self.columns.len() - fit);
        let areas = Layout::horizontal(vec![Constraint::Fill(1); fit]).split(whole);
        let shown = self.left..self.left + fit;
        let (before, after) = (self.left, self.columns.len() - shown.end);
        for ((at, column), area) in self
            .columns
            .iter_mut()
            .enumerate()
            .skip(self.left)
            .zip(areas.iter())
        {
            let here = at == self.column;
            let mut title = format!(" {} {} ", column.label, column.cards.len());
            if at == shown.start && before > 0 {
                title = format!(" ‹{before}{title}");
            }
            let mut block = Block::bordered().title(Line::from(title));
            if at + 1 == shown.end && after > 0 {
                block = block.title(Line::from(format!(" {after}› ")).right_aligned());
            }
            if here {
                block = block.border_style(Style::new().bold());
            }
            let lines: Vec<Line> = column
                .cards
                .iter()
                .map(|card| {
                    let off = if card.status == column.status {
                        String::new()
                    } else {
                        format!("[{}] ", card.status)
                    };
                    Line::from(format!("{} {off}{}", card.key, card.title))
                })
                .collect();
            let mut cards = Cards::new(lines).block(block);
            if here {
                cards = cards.highlight_style(Style::new().reversed());
            }
            frame.render_stateful_widget(cards, *area, &mut column.row);
        }
        if let Some(picker) = &mut self.picker {
            picker.render(frame, whole);
        }
    }
}

#[cfg(test)]
mod tests {
    use tm_core::scheme::{Status, Transitions, Workflow};

    use super::*;

    fn ids(columns: &[Column]) -> Vec<&str> {
        columns
            .iter()
            .map(|column| column.status.as_str())
            .collect()
    }

    #[test]
    fn every_workflows_statuses_are_columns_once_in_order() {
        let mut scheme = Scheme::built_in();
        let status = |id: &str, category| Status {
            id: id.into(),
            label: id.into(),
            category,
        };
        scheme.workflows.push(Workflow {
            name: "other".into(),
            start: "todo".into(),
            statuses: vec![
                status("todo", Category::Todo),
                status("blocked", Category::Active),
                status("shipped", Category::Done),
            ],
            transitions: Transitions::Any,
        });
        let open = ["triage", "todo", "in_progress", "in_review", "blocked"];
        assert_eq!(ids(&columns(&scheme, false)), open);
        let every = columns(&scheme, true);
        assert_eq!(
            ids(&every),
            [&open[..4], &["done", "cancelled", "blocked", "shipped"]].concat()
        );
    }

    #[test]
    fn a_status_no_column_has_goes_in_the_first_triage_column() {
        let scheme = Scheme::built_in();
        let shown = columns(&scheme, false);
        assert_eq!(place(&shown, "in_review"), Some(3));
        assert_eq!(place(&shown, "gone"), Some(0));
        let mut later: Vec<Column> = columns(&scheme, false);
        later.rotate_left(1);
        assert_eq!(place(&later, "gone"), Some(3), "triage, wherever it is");
        let no_triage: Vec<Column> = columns(&scheme, false).into_iter().skip(1).collect();
        assert_eq!(place(&no_triage, "gone"), Some(0));
        assert_eq!(place(&[], "todo"), None);
    }

    /// The open columns, Todo holding TM-1 to TM-3 and In Review TM-4.
    fn filled() -> Vec<Column> {
        let mut shown = columns(&Scheme::built_in(), false);
        for (key, status) in [
            ("TM-1", "todo"),
            ("TM-2", "todo"),
            ("TM-3", "todo"),
            ("TM-4", "in_review"),
        ] {
            let at = place(&shown, status).unwrap();
            shown[at].cards.push(Card {
                id: Uuid::nil(),
                key: key.into(),
                version: 1,
                issue_type: "task".into(),
                status: status.into(),
                title: key.into(),
            });
        }
        shown
    }

    fn rows(columns: &[Column]) -> Vec<Option<usize>> {
        columns.iter().map(|column| column.row.selected()).collect()
    }

    #[test]
    fn the_cursor_goes_to_its_issue_wherever_it_moved() {
        let mut shown = filled();
        let was = [("todo".to_owned(), Some(0))];
        assert_eq!(arrange(&mut shown, &was, Some("TM-4"), Some("todo")), 3);
        assert_eq!(shown[3].row.selected(), Some(0));
        assert_eq!(shown[1].row.selected(), Some(0), "todo keeps its row");
    }

    #[test]
    fn a_gone_issue_leaves_the_cursor_in_its_column_at_its_row_held() {
        let mut shown = filled();
        let was = [("todo".to_owned(), Some(7))];
        assert_eq!(arrange(&mut shown, &was, Some("TM-9"), Some("todo")), 1);
        assert_eq!(shown[1].row.selected(), Some(2), "held to the last");
    }

    #[test]
    fn with_its_column_gone_the_cursor_goes_to_the_first_with_an_issue() {
        let mut shown = filled();
        assert_eq!(arrange(&mut shown, &[], None, Some("done")), 1);
        assert_eq!(arrange(&mut shown, &[], None, None), 1);
        assert_eq!(rows(&shown), [None, Some(0), None, Some(0)]);
        let mut empty = columns(&Scheme::built_in(), false);
        assert_eq!(arrange(&mut empty, &[], None, None), 0);
    }
}
