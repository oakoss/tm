//! The Views sidebar: the saved Views the repo file holds for this Project,
//! one picked to open as a list or a Board.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize as _};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, List as Rows, ListState, Paragraph, Wrap};
use tm_core::text::printable;
use tm_store::view::{Layout, View};

use std::path::PathBuf;

use crate::board::Board;
use crate::list::List;
use crate::scroll::{Move, Scroll};
use crate::{Actions, Proposal, Screen};

/// What a key in the sidebar did.
pub(crate) enum Picked {
    Stay,
    Closed,
    Open(View),
    /// Write a View in the editor: this one, or with none a new one.
    Edit(Option<View>),
    Remove(View),
    /// Why a change cannot be made here.
    Refused(String),
}

#[derive(Default)]
pub(crate) struct Sidebar {
    /// The Views, or why they cannot be read; read once, when it opens.
    views: Option<Result<Vec<View>, String>>,
    /// Whether the person here may change them, read with them.
    editable: Option<Result<(), String>>,
    at: ListState,
}

/// The sidebar's widest, in columns, so the screen beside it keeps room.
const WIDTH: u16 = 32;

impl Sidebar {
    pub(crate) fn read(&mut self, actions: &dyn Actions) {
        if self.views.is_none() {
            self.views = Some(actions.views());
            self.editable = Some(actions.may_edit_views());
            self.at.select(Some(0));
        }
    }

    /// `change`, or why the person here may not make it, said before any
    /// editor opens.
    fn change(&self, change: impl FnOnce() -> Picked) -> Picked {
        match &self.editable {
            Some(Ok(())) => change(),
            Some(Err(why)) => Picked::Refused(why.clone()),
            None => Picked::Stay,
        }
    }

    fn listed(&self) -> &[View] {
        match &self.views {
            Some(Ok(views)) => views,
            _ => &[],
        }
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Picked {
        let count = self.listed().len();
        let at = self.at.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('v') => return Picked::Closed,
            KeyCode::Enter => {
                return self
                    .listed()
                    .get(at)
                    .map_or(Picked::Stay, |view| Picked::Open(view.clone()));
            }
            KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                self.at.select(Some((at + 1).min(count - 1)));
            }
            KeyCode::Up | KeyCode::Char('k') => self.at.select(Some(at.saturating_sub(1))),
            KeyCode::Char('n') => return self.change(|| Picked::Edit(None)),
            KeyCode::Char('e') => {
                let Some(view) = self.listed().get(at).cloned() else {
                    return Picked::Stay;
                };
                return self.change(|| Picked::Edit(Some(view)));
            }
            KeyCode::Char('d') => {
                let Some(view) = self.listed().get(at).cloned() else {
                    return Picked::Stay;
                };
                return self.change(|| Picked::Remove(view));
            }
            _ => {}
        }
        Picked::Stay
    }

    /// The footer's text while the sidebar is open.
    pub(crate) fn prompt() -> String {
        "Views · Enter open · e edit · n new · d delete · Esc close".to_owned()
    }

    /// Draws the sidebar down the left of `body`.
    pub(crate) fn render(&mut self, frame: &mut Frame, body: Rect) {
        let area = Rect {
            width: body.width.min(WIDTH),
            ..body
        };
        frame.render_widget(Clear, area);
        let block = Block::bordered().title(Line::from(" Views "));
        let Some(views) = &self.views else {
            frame.render_widget(Paragraph::new("reading…").block(block), area);
            return;
        };
        match views {
            Err(why) => {
                let said = format!("✗ cannot read the Views: {}", printable(why));
                frame.render_widget(
                    Paragraph::new(said).wrap(Wrap { trim: false }).block(block),
                    area,
                );
            }
            Ok(views) if views.is_empty() => {
                let said = "No Views yet; add [[views]] to .config/tm.toml.";
                frame.render_widget(
                    Paragraph::new(said).wrap(Wrap { trim: false }).block(block),
                    area,
                );
            }
            Ok(views) => {
                let names: Vec<Line> = views
                    .iter()
                    .map(|view| {
                        let mut line = Line::from(printable(&view.name));
                        if view.layout == Layout::Board {
                            line.push_span(" · board".dim());
                        }
                        line
                    })
                    .collect();
                let rows = Rows::new(names)
                    .block(block)
                    .highlight_style(Style::new().reversed());
                frame.render_stateful_widget(rows, area, &mut self.at);
            }
        }
    }
}

/// What a key on the diff said.
#[derive(Clone, Copy)]
pub(crate) enum Answer {
    Yes,
    No,
    Scrolled,
}

/// A View change shown as the diff it would make, written only on `y`, as
/// a file outside the Workspace is written only after a confirmed diff.
pub(crate) struct Confirm {
    proposal: Proposal,
    draft: Option<PathBuf>,
    scroll: Scroll,
}

impl Confirm {
    pub(crate) fn new(proposal: Proposal, draft: Option<PathBuf>) -> Self {
        Self {
            proposal,
            draft,
            scroll: Scroll::default(),
        }
    }

    pub(crate) fn into_parts(self) -> (Proposal, Option<PathBuf>) {
        (self.proposal, self.draft)
    }

    pub(crate) fn draft(&self) -> Option<&PathBuf> {
        self.draft.as_ref()
    }

    /// A plain `y` says yes; the keys that scroll the diff scroll it; any
    /// other says no.
    pub(crate) fn key(&mut self, key: KeyEvent) -> Answer {
        let plain = (key.modifiers - KeyModifiers::SHIFT).is_empty();
        let to = match key.code {
            KeyCode::Char('y') if plain => return Answer::Yes,
            KeyCode::Char('j') | KeyCode::Down => Move::Down,
            KeyCode::Char('k') | KeyCode::Up => Move::Up,
            KeyCode::PageDown => Move::PageDown,
            KeyCode::PageUp => Move::PageUp,
            _ => return Answer::No,
        };
        self.scroll.go(to);
        Answer::Scrolled
    }

    pub(crate) fn prompt() -> String {
        "y save this · j/k scroll · any other key leaves it unsaved".to_owned()
    }

    pub(crate) fn render(&mut self, frame: &mut Frame, body: Rect) {
        frame.render_widget(Clear, body);
        let lines: Vec<Line> = self
            .proposal
            .diff
            .lines()
            .map(|line| {
                let shown = Line::from(printable(line).replace('\t', " "));
                match line.chars().next() {
                    Some('+') if !line.starts_with("+++") => shown.green(),
                    Some('-') if !line.starts_with("---") => shown.red(),
                    _ => shown,
                }
            })
            .collect();
        let block = Block::bordered().title(Line::from(" Save this change? "));
        self.scroll
            .render(frame, Paragraph::new(lines), block, body);
    }
}

/// The screen `view` opens as.
pub(crate) fn screen(view: View) -> Screen {
    match view.layout {
        Layout::List => Screen::List(List::of_view(view)),
        Layout::Board => Screen::Board(Board::of_view(view)),
    }
}
