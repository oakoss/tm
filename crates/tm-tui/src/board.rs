//! The Board: a Project's Issues in a column per Workflow Status, each
//! moved between columns with the same writes as `tm move`.

use std::collections::BTreeSet;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize as _};
use ratatui::text::Line;
use ratatui::widgets::{Block, List as Cards, ListState, Paragraph};
use tm_core::filter::{Context, names_a_state, open_unless_named, parse};
use tm_core::scheme::{Category, PRIORITY_RANKS, Scheme};
use tm_core::text::printable;
use tm_store::Store;
use tm_store::view::View;
use uuid::Uuid;

use crate::cursor;
use crate::issue::IssueScreen;
use crate::keys::{self, Binding, ch, plain};
use crate::picker::{Choice, Field, Picker, statuses};
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
    /// Its value of the swimlane field, `-` when unset.
    lane: String,
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
    /// The View it opened as, whose filter it reads.
    view: Option<View>,
    /// Whether the done and cancelled columns show.
    all: bool,
    columns: Vec<Column>,
    /// The column the cursor is in.
    column: usize,
    /// The first column drawn, so the one with the cursor stays in view.
    left: usize,
    /// Statuses per Issue type, in Workflow order.
    statuses: Vec<(String, Vec<Choice>)>,
    problem: Option<String>,
    /// Each swimlane's value and band height, in order.
    lanes: Vec<(String, usize)>,
    /// The first row drawn, shared so the bands line up across columns.
    top: usize,
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

/// The Issues `filter` matches, as cards.
fn cards(
    store: &Store,
    filter: &str,
    lane: Option<&str>,
    actions: &dyn Actions,
) -> Result<Vec<Card>, String> {
    let me = actions.me().ok();
    let read = read::issues(store, filter, Context { me }).map_err(|err| err.to_string())?;
    Ok(read
        .into_iter()
        .map(|issue| Card {
            issue_type: issue.field("type"),
            status: issue.field("status"),
            title: issue.field("title"),
            lane: lane.map_or_else(String::new, |field| issue.value(field)),
            id: issue.id,
            key: issue.key,
            version: issue.version,
        })
        .collect())
}

/// Refuses a View whose filter or swimlanes' filter does not parse alone,
/// naming which, before they are joined, where a quote could run across.
fn parses(view: &View) -> Result<(), String> {
    parse(&view.filter).map_err(|err| format!("{}: {err}", view.name))?;
    if let Some(lanes) = &view.swimlanes {
        parse(&lanes.filter).map_err(|err| format!("{}, swimlanes: {err}", view.name))?;
    }
    Ok(())
}

/// `view`'s filter with its swimlanes' own, which narrows the Issues in
/// them.
fn narrowed(view: &View) -> String {
    match &view.swimlanes {
        Some(lanes) if !lanes.filter.is_empty() => format!("{} {}", view.filter, lanes.filter),
        _ => view.filter.clone(),
    }
}

fn lane_field(view: &View) -> Option<&str> {
    view.swimlanes.as_ref().map(|lanes| lanes.field.as_str())
}

/// Where `value` of `field` sorts among its swimlanes: the Scheme's order
/// for a Status, type, priority or select field, else none.
fn rank(scheme: &Scheme, field: &str, value: &str) -> Option<usize> {
    match field {
        "priority" => PRIORITY_RANKS.iter().position(|(id, _)| *id == value),
        "status" => scheme
            .workflows
            .iter()
            .flat_map(|workflow| &workflow.statuses)
            .position(|status| status.id == value),
        "type" => scheme.types.iter().position(|kind| kind.id == value),
        _ => scheme
            .field(field)?
            .options
            .iter()
            .position(|choice| choice.id == value),
    }
}

/// The swimlanes for `columns` by `field`: each value its cards hold, in
/// the Scheme's order where it has one, else by name, unset last, with the
/// most cards any one column has in it.
fn lanes(scheme: &Scheme, field: &str, columns: &mut [Column]) -> Vec<(String, usize)> {
    let mut values: Vec<String> = columns
        .iter()
        .flat_map(|column| column.cards.iter().map(|card| card.lane.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    values.sort_by_key(|value| {
        let unset = value == "-" || (field == "priority" && value == "none");
        (unset, rank(scheme, field, value).unwrap_or(usize::MAX))
    });
    let order = |lane: &str| values.iter().position(|value| value == lane);
    for column in columns.iter_mut() {
        column.cards.sort_by_key(|card| order(&card.lane));
    }
    values
        .iter()
        .map(|value| {
            let height = columns
                .iter()
                .map(|column| {
                    column
                        .cards
                        .iter()
                        .filter(|card| &card.lane == value)
                        .count()
                })
                .max()
                .unwrap_or(0);
            (value.clone(), height)
        })
        .collect()
}

/// The columns `view` lists, in its order, each holding the Issues in its
/// Status that the View's filter and its own both match; done and
/// cancelled ones too, since a View names its Statuses.
fn listed(store: &Store, view: &View, actions: &dyn Actions) -> Result<Vec<Column>, String> {
    // The View's filters are read without a column's first, so their error
    // is not named for a column; `parses` keeps a quote inside its filter.
    let typed = narrowed(view);
    cards(store, &read::in_project(&typed, actions), None, actions)
        .map_err(|why| format!("{}: {why}", view.name))?;
    let lane = lane_field(view);
    let scheme = store.scheme();
    let mut filled = Vec::with_capacity(view.columns.len());
    for column in &view.columns {
        let status = scheme
            .workflows
            .iter()
            .flat_map(|workflow| &workflow.statuses)
            .find(|status| status.id == column.status)
            .ok_or_else(|| format!("{}: no Status {:?}", view.name, column.status))?;
        let filter = format!("{typed} {} status:{}", column.filter, status.id);
        let cards = cards(store, &read::in_project(&filter, actions), lane, actions)
            .map_err(|why| format!("{}, column {}: {why}", view.name, status.id))?;
        filled.push(Column {
            status: status.id.clone(),
            label: printable(&status.label),
            category: status.category,
            cards,
            row: ListState::default(),
        });
    }
    Ok(filled)
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
    /// The Board a View with the board layout opens as.
    pub(crate) fn of_view(view: View) -> Self {
        Self {
            view: Some(view),
            ..Self::default()
        }
    }

    /// Whether its columns are the ones its View lists.
    fn listed(&self) -> bool {
        self.view
            .as_ref()
            .is_some_and(|view| !view.columns.is_empty())
    }

    /// The breadcrumb's name for it: its View's, else Board.
    pub(crate) fn title(&self) -> String {
        self.view
            .as_ref()
            .map_or_else(|| "Board".to_owned(), |view| printable(&view.name))
    }

    /// Whether the last read failed, its reason hiding the Issues, so none
    /// is moved unseen.
    pub(crate) fn unread(&self) -> bool {
        self.problem.is_some()
    }

    /// Reads the Issues again, the cursor staying on its Issue wherever it
    /// went.
    pub(crate) fn load(&mut self, store: &Store, actions: &dyn Actions) {
        let kept = self.selected().map(|card| card.key.clone());
        let view = self.view.as_ref();
        let lane = view.and_then(lane_field);
        let read = if let Some(Err(why)) = view.map(parses) {
            Err(why)
        } else if let Some(field) = lane.filter(|field| store.scheme().field(field).is_none()) {
            Err(format!(
                "{}: no field {field:?}",
                view.map_or("", |view| &view.name)
            ))
        } else if let Some(view) = view.filter(|view| !view.columns.is_empty()) {
            listed(store, view, actions)
        } else {
            let typed = view.map(narrowed).unwrap_or_default();
            let filter = read::in_project(&open_unless_named(&typed, self.all), actions);
            cards(store, &filter, lane, actions).map(|cards| {
                // A filter naming a state may admit done or cancelled
                // Issues, which need their own columns, not the triage one.
                let mut columns = columns(store.scheme(), self.all || names_a_state(&typed));
                for card in cards {
                    if let Some(at) = place(&columns, &card.status) {
                        columns[at].cards.push(card);
                    }
                }
                columns
            })
        };
        let mut filled = match read {
            Ok(filled) => filled,
            // The last Board read stays behind the reason, so the next read
            // finds the cursor's Issue again.
            Err(why) => {
                self.problem = Some(printable(&why));
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
        self.lanes = match self.view.as_ref().and_then(lane_field) {
            Some(field) => lanes(store.scheme(), field, &mut filled),
            None => Vec::new(),
        };
        self.columns = filled;
        let types = self.columns.iter().flat_map(|column| &column.cards);
        self.statuses = statuses(store, types.map(|card| card.issue_type.as_str()));
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
                Outcome::Pick(Picker::new(Field::Status, choices, target))
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
        let to = if self.listed() {
            // A View's columns set the order: the Status of the column drawn
            // beside this one, if the Issue's Workflow has it.
            let beside = if right {
                self.column.checked_add(1)
            } else {
                self.column.checked_sub(1)
            };
            beside
                .and_then(|at| self.columns.get(at))
                .and_then(|column| {
                    order
                        .iter()
                        .find(|choice| choice.value.as_deref() == Some(column.status.as_str()))
                })
        } else {
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
            next.and_then(|next| order.get(next))
        };
        match to {
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
        if !self.lanes.is_empty() {
            self.scroll_lanes(usize::from(whole.height.saturating_sub(2)));
        }
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
            if self.lanes.is_empty() {
                let lines: Vec<Line> = column.cards.iter().map(|card| line(column, card)).collect();
                let mut cards = Cards::new(lines).block(block);
                if here {
                    cards = cards.highlight_style(Style::new().reversed());
                }
                frame.render_stateful_widget(cards, *area, &mut column.row);
                continue;
            }
            let (mut rows, selected) = banded(&self.lanes, column);
            if let Some(at) = selected.filter(|_| here) {
                rows[at] = rows[at].clone().reversed();
            }
            let window: Vec<Line> = rows.into_iter().skip(self.top).collect();
            frame.render_widget(Paragraph::new(window).block(block), *area);
        }
    }

    /// Moves the rows every column shares so the cursor's card is in view.
    fn scroll_lanes(&mut self, height: usize) {
        let rows: usize = self.lanes.iter().map(|(_, tall)| tall + 1).sum();
        self.top = self.top.min(rows.saturating_sub(height));
        let Some(column) = self.columns.get(self.column) else {
            return;
        };
        let Some(at) = banded(&self.lanes, column).1 else {
            return;
        };
        if at < self.top {
            self.top = at;
        } else if at >= self.top + height {
            self.top = at + 1 - height;
        }
        // The cursor's band keeps its name in view while its card fits.
        let header = header_row(&self.lanes, at);
        if at - header < height {
            self.top = self.top.min(header);
        }
    }
}

fn header_row(lanes: &[(String, usize)], row: usize) -> usize {
    let mut start = 0;
    for (_, tall) in lanes {
        if row < start + 1 + tall {
            return start;
        }
        start += 1 + tall;
    }
    start
}

/// A card's line: its Key, its Status when it is not the column's, and its
/// title.
fn line<'a>(column: &Column, card: &Card) -> Line<'a> {
    let off = if card.status == column.status {
        String::new()
    } else {
        format!("[{}] ", card.status)
    };
    Line::from(format!("{} {off}{}", card.key, card.title))
}

/// `column`'s rows in swimlanes: each band's name, then its cards, then
/// blank rows to the band's height, so bands line up across columns; and
/// the row of the selected card.
fn banded<'a>(lanes: &[(String, usize)], column: &Column) -> (Vec<Line<'a>>, Option<usize>) {
    let selected = column.row.selected().and_then(|at| column.cards.get(at));
    let mut rows = Vec::new();
    let mut at = None;
    for (lane, height) in lanes {
        let name = if lane == "-" { "none" } else { lane };
        rows.push(Line::from(format!("── {name} ──")).dim());
        let start = rows.len();
        for card in column.cards.iter().filter(|card| &card.lane == lane) {
            if selected.is_some_and(|selected| std::ptr::eq(selected, card)) {
                at = Some(rows.len());
            }
            rows.push(line(column, card));
        }
        rows.resize(start + height, Line::default());
    }
    (rows, at)
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
                lane: String::new(),
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
