//! One Issue laid out from what `tm show` and `tm history` report: its
//! fields, description and comments as Markdown, its latest changes, and
//! the Issues it relates to, each opened with Enter.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List as Rows, ListState, Paragraph, Wrap};
use serde::Deserialize;
use serde_json::Value;
use tm_core::text::printable;
use tm_store::{Store, StoreError};
use uuid::Uuid;

use crate::keys::{self, Binding, ch, plain};
use crate::{Actions, Outcome, Screen, cursor, markdown, read};

/// Rows a page key scrolls.
const PAGE: u16 = 10;

/// Columns from which the related Issues sit beside the text, not below.
const WIDE: u16 = 100;

/// The latest writes shown and read; `tm history` has the rest.
pub const LATEST_WRITES: usize = 5;

// Every key below is one the CLI always sends, `null` when it has nothing:
// `deserialize_with` makes serde refuse a missing one rather than read it as
// `None`, so a key renamed in the CLI fails the read instead of blanking a
// line. Only a question's `options` may be absent.

/// What `tm show --json` and `tm history --json` report for an Issue, as
/// far as the screen lays it out (ADR 0018).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Issue {
    pub shown: Shown,
    pub history: History,
}

impl Issue {
    /// `tm show --json`'s and `tm history --json`'s `data`, read.
    ///
    /// # Errors
    ///
    /// Which command's data does not have the shape this reads, and why.
    pub fn read(shown: Value, history: Value) -> Result<Self, String> {
        let shown = serde_json::from_value(shown)
            .map_err(|err| format!("cannot read tm show's JSON: {err}"))?;
        let history = serde_json::from_value(history)
            .map_err(|err| format!("cannot read tm history's JSON: {err}"))?;
        Ok(Self { shown, history })
    }
}

/// `tm show --json`'s `data`.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Shown {
    pub id: Uuid,
    #[serde(deserialize_with = "Option::deserialize")]
    pub key: Option<String>,
    pub fields: BTreeMap<String, Value>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub claim: Option<Claim>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub parent: Option<Related>,
    pub links: Vec<Link>,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Claim {
    pub agent: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub on_behalf_of: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub branch: Option<String>,
    /// `null` for a Claim taken with no expiry.
    #[serde(deserialize_with = "Option::deserialize")]
    pub expires_in_ms: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Related {
    pub id: Uuid,
    #[serde(deserialize_with = "Option::deserialize")]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Link {
    /// The Link's name read from this Issue, such as `blocks` or `is
    /// blocked by`.
    pub name: String,
    pub other: Related,
}

#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Comment {
    pub actor: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub on_behalf_of: Option<String>,
    /// `null` once retracted.
    #[serde(deserialize_with = "Option::deserialize")]
    pub body: Option<String>,
    pub utc_ms: i64,
    pub retracted: bool,
    /// `question`, `answer` or `withdrawn` for a comment in a `tm ask`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub role: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub edited_utc_ms: Option<i64>,
    /// A question's choices; absent on any other comment.
    pub options: Option<Vec<String>>,
}

/// `tm history --json`'s `data`.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct History {
    pub total: usize,
    /// Oldest first.
    pub writes: Vec<Logged>,
}

#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Logged {
    pub actor: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub on_behalf_of: Option<String>,
    pub utc_ms: i64,
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Change {
    /// The change as `tm history` prints it.
    pub line: String,
}

#[derive(Clone, Copy)]
pub(crate) enum Act {
    Back,
    Down,
    Up,
    First,
    Last,
    PageDown,
    PageUp,
    Next,
    Previous,
    Open,
}

pub(crate) const KEYS: [Binding<Act>; 10] = [
    Binding {
        keys: &[ch('q'), plain(KeyCode::Esc), plain(KeyCode::Backspace)],
        action: Act::Back,
        does: "back",
        hint: Some("back"),
    },
    Binding {
        keys: &[ch('j'), plain(KeyCode::Down)],
        action: Act::Down,
        does: "scroll down and up",
        hint: Some("scroll"),
    },
    Binding {
        keys: &[ch('k'), plain(KeyCode::Up)],
        action: Act::Up,
        does: "scroll down and up",
        hint: Some("scroll"),
    },
    Binding {
        keys: &[ch('g'), plain(KeyCode::Home)],
        action: Act::First,
        does: "top and bottom",
        hint: None,
    },
    Binding {
        keys: &[ch('G'), plain(KeyCode::End)],
        action: Act::Last,
        does: "top and bottom",
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
    scroll: u16,
    /// The furthest `scroll` reaches at the last size drawn.
    bottom: u16,
    /// Now, as of the last read, for the times shown as ages.
    now_ms: i64,
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
            scroll: 0,
            bottom: 0,
            now_ms: 0,
        }
    }

    pub(crate) fn title(&self) -> String {
        printable(&self.key)
    }

    /// Reads the Issue again, keeping the place scrolled to and the related
    /// Issue selected.
    pub(crate) fn load(&mut self, store: &Store, actions: &dyn Actions) {
        self.now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
            });
        self.read = actions.issue(store, self.id);
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
        let Some(act) = keys::action(&KEYS, key) else {
            return Outcome::Stay;
        };
        let last = self.rows.len().saturating_sub(1);
        let row = self.at.selected().unwrap_or(0);
        match act {
            Act::Back => return Outcome::Back,
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
            Act::Down => self.scroll = self.scroll.saturating_add(1),
            Act::Up => self.scroll = self.scroll.saturating_sub(1),
            Act::First => self.scroll = 0,
            Act::Last => self.scroll = self.bottom,
            Act::PageDown => self.scroll = self.scroll.saturating_add(PAGE),
            Act::PageUp => self.scroll = self.scroll.saturating_sub(PAGE),
        }
        self.scroll = self.scroll.min(self.bottom);
        Outcome::Stay
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
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        let rows = paragraph.line_count(inner.width);
        let rows = u16::try_from(rows).unwrap_or(u16::MAX);
        self.bottom = rows.saturating_sub(inner.height);
        self.scroll = self.scroll.min(self.bottom);
        frame.render_widget(paragraph.block(block).scroll((self.scroll, 0)), text_area);
        if let Some(area) = rows_area {
            self.render_rows(frame, area);
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

const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const DIM: Style = Style::new().add_modifier(Modifier::DIM);

/// The text beside the related Issues, wrapped to `width`.
fn body(issue: &Issue, width: u16, now_ms: i64) -> Vec<Line<'static>> {
    let shown = &issue.shown;
    let title = field(shown, "title").unwrap_or_default();
    let mut lines = vec![Line::styled(title, BOLD), Line::default()];
    lines.extend(facts(shown, now_ms));
    lines.push(Line::default());
    lines.push(Line::styled("Description", BOLD));
    match shown.fields.get("description").and_then(Value::as_str) {
        Some(text) if !text.trim().is_empty() => lines.extend(markdown::lines(text, width)),
        _ => lines.push(Line::styled("none", DIM)),
    }
    lines.push(Line::default());
    lines.extend(comments(shown, width, now_ms));
    lines.push(Line::default());
    lines.extend(history(issue, now_ms));
    lines
}

/// `shown`'s text field `name`, safe for a screen cell.
fn field(shown: &Shown, name: &str) -> Option<String> {
    let text = match shown.fields.get(name)? {
        Value::Null => return None,
        Value::String(text) => text.clone(),
        // Shown as the CLI sent it rather than dropped.
        other => other.to_string(),
    };
    Some(printable(&text).replace('\t', " "))
}

/// Who acted, and for whom.
fn by(actor: &str, on_behalf_of: Option<&str>) -> String {
    match on_behalf_of {
        Some(person) => format!("{} for {}", printable(actor), printable(person)),
        None => printable(actor),
    }
}

/// The fields beside their labels: Status, the people, the Claim, the dates.
fn facts(shown: &Shown, now_ms: i64) -> Vec<Line<'static>> {
    let mut facts: Vec<(&str, String)> = Vec::new();
    for (label, name) in [
        ("Status", "status"),
        ("Type", "type"),
        ("Priority", "priority"),
        ("Assignee", "assignee"),
        ("Reporter", "reporter"),
    ] {
        if let Some(value) = field(shown, name) {
            facts.push((label, value));
        }
    }
    if let Some(claim) = &shown.claim {
        let mut holder = by(&claim.agent, claim.on_behalf_of.as_deref());
        if let Some(branch) = &claim.branch {
            holder = format!("{holder} on {}", printable(branch));
        }
        if let Some(left) = claim.expires_in_ms {
            holder = format!("{holder}, {} left", span(left));
        }
        facts.push(("Claim", holder));
    }
    for (label, name) in [("Created", "created"), ("Updated", "updated")] {
        if let Some(at) = shown.fields.get(name).and_then(Value::as_i64) {
            facts.push((label, ago(now_ms, at)));
        }
    }
    let wide = facts
        .iter()
        .map(|(label, _)| label.len())
        .max()
        .unwrap_or(0);
    facts
        .into_iter()
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(format!("{label:<wide$}  "), DIM),
                Span::raw(value),
            ])
        })
        .collect()
}

/// Each comment under who wrote it and when, its body as Markdown.
fn comments(shown: &Shown, width: u16, now_ms: i64) -> Vec<Line<'static>> {
    let heading = format!("Comments ({})", shown.comments.len());
    let mut lines = vec![Line::styled(heading, BOLD)];
    for comment in &shown.comments {
        let mut who = by(&comment.actor, comment.on_behalf_of.as_deref());
        // As `tm show` names a comment's part in a `tm ask`.
        match comment.role.as_deref() {
            Some("question") => who.push_str(" asked"),
            Some("answer") => who.push_str(" answered"),
            Some("withdrawn") => who.push_str(" withdrew the question"),
            _ => {}
        }
        let edited = if comment.edited_utc_ms.is_some() {
            " (edited)"
        } else {
            ""
        };
        let when = ago(now_ms, comment.utc_ms);
        lines.push(Line::styled(format!("{who} · {when}{edited}"), DIM));
        let body = comment.body.as_deref().filter(|_| !comment.retracted);
        let Some(body) = body else {
            lines.push(Line::styled("  retracted", DIM));
            continue;
        };
        for line in markdown::lines(body, width.saturating_sub(2)) {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
        if let Some(options) = comment
            .options
            .as_ref()
            .filter(|options| !options.is_empty())
        {
            let numbered: Vec<String> = options
                .iter()
                .enumerate()
                .map(|(at, option)| format!("{}. {}", at + 1, printable(option)))
                .collect();
            lines.push(Line::from(format!("  options: {}", numbered.join("  "))));
        }
    }
    lines
}

/// The latest writes, newest first, and how many more `tm history` has.
fn history(issue: &Issue, now_ms: i64) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled("History", BOLD)];
    let history = &issue.history;
    let shown = history.writes.len().min(LATEST_WRITES);
    for logged in history.writes.iter().rev().take(shown) {
        let who = by(&logged.actor, logged.on_behalf_of.as_deref());
        lines.push(Line::styled(
            format!("{} · {who}", ago(now_ms, logged.utc_ms)),
            DIM,
        ));
        for change in &logged.changes {
            lines.push(Line::from(format!("  {}", printable(&change.line))));
        }
    }
    let earlier = history.total.saturating_sub(shown);
    if earlier > 0 {
        let key = printable(issue.shown.key.as_deref().unwrap_or_default());
        lines.push(Line::styled(
            format!("{earlier} earlier: tm history {key}"),
            DIM,
        ));
    }
    lines
}

/// How long ago `at` was, as of `now`, both in Unix milliseconds.
fn ago(now: i64, at: i64) -> String {
    format!("{} ago", span(now.saturating_sub(at)))
}

/// `ms` as its largest whole unit.
fn span(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_age_takes_its_largest_whole_unit() {
        assert_eq!(ago(10_000, 10_000), "0s ago");
        assert_eq!(ago(3_600_000, 0), "1h ago");
        assert_eq!(ago(90_000_000, 0), "1d ago");
        assert_eq!(span(-5), "0s");
    }
}
