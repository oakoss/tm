//! A popup of values for one field, narrowed as text is typed, whose choice
//! becomes a write to the Issues it was opened on.

use std::collections::BTreeSet;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, List as Choices, ListState, Paragraph};
use tm_core::text::printable;
use tm_store::Store;

use crate::Write;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Status,
    Priority,
    Assignee,
}

impl Field {
    const fn name(self) -> &'static str {
        match self {
            Self::Status => "Status",
            Self::Priority => "Priority",
            Self::Assignee => "Assign",
        }
    }
}

/// A value a picker offers: `None` clears the field.
#[derive(Clone)]
pub(crate) struct Choice {
    pub(crate) value: Option<String>,
    pub(crate) label: String,
}

/// What a key in a picker did.
pub(crate) enum Picked {
    Open,
    Cancelled,
    /// Enter took this choice for these Issues and versions.
    Chosen(Field, Choice, Vec<(String, i64)>),
}

pub(crate) struct Picker {
    field: Field,
    choices: Vec<Choice>,
    /// The Issues it was opened on and the versions read, so a read while
    /// it is open cannot move the write to another Issue.
    targets: Vec<(String, i64)>,
    typed: String,
    at: ListState,
}

impl Picker {
    pub(crate) fn new(field: Field, choices: Vec<Choice>, targets: Vec<(String, i64)>) -> Self {
        Self {
            field,
            choices,
            targets,
            typed: String::new(),
            at: ListState::default().with_selected(Some(0)),
        }
    }

    /// The choices the typed text matches, ignoring case.
    fn shown(&self) -> Vec<&Choice> {
        let typed = self.typed.to_lowercase();
        self.choices
            .iter()
            .filter(|choice| {
                choice.label.to_lowercase().contains(&typed)
                    || choice
                        .value
                        .as_deref()
                        .is_some_and(|value| value.to_lowercase().contains(&typed))
            })
            .collect()
    }

    /// The choice Enter takes: the one selected, or for an assignee a name
    /// typed that no choice matches.
    fn chosen(&self) -> Option<Choice> {
        let shown = self.shown();
        let at = self.at.selected().unwrap_or(0);
        if let Some(choice) = shown.get(at) {
            return Some((*choice).clone());
        }
        let typed = self.typed.trim();
        (self.field == Field::Assignee && !typed.is_empty()).then(|| Choice {
            value: Some(typed.to_owned()),
            label: typed.to_owned(),
        })
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Picked {
        let count = self.shown().len();
        match key.code {
            KeyCode::Esc => return Picked::Cancelled,
            KeyCode::Enter => {
                let targets = std::mem::take(&mut self.targets);
                return match self.chosen() {
                    Some(chosen) => Picked::Chosen(self.field, chosen, targets),
                    None => Picked::Cancelled,
                };
            }
            KeyCode::Down => self.at.select(Some(
                self.at
                    .selected()
                    .unwrap_or(0)
                    .saturating_add(1)
                    .min(count.saturating_sub(1)),
            )),
            KeyCode::Up => self
                .at
                .select(Some(self.at.selected().unwrap_or(0).saturating_sub(1))),
            KeyCode::Backspace => {
                self.typed.pop();
                self.at.select(Some(0));
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.typed.push(c);
                self.at.select(Some(0));
            }
            _ => {}
        }
        Picked::Open
    }

    /// The footer's text while the picker is open.
    pub(crate) fn prompt(&self) -> String {
        format!(
            "{} {} · Enter choose · Esc cancel",
            self.field.name(),
            if self.typed.is_empty() {
                "type to narrow".to_owned()
            } else {
                format!("/{}", printable(&self.typed))
            }
        )
    }

    /// Draws the picker centred on `whole`.
    pub(crate) fn render(&mut self, frame: &mut Frame, whole: Rect) {
        let shown: Vec<String> = self
            .shown()
            .iter()
            .map(|choice| choice.label.clone())
            .collect();
        let height = u16::try_from(shown.len().max(1)).unwrap_or(u16::MAX) + 2;
        let width = shown
            .iter()
            .map(|label| label.chars().count())
            .max()
            .unwrap_or(0)
            + 4;
        let width = u16::try_from(width.max(20)).unwrap_or(u16::MAX);
        let popup = whole.centered(Constraint::Length(width), Constraint::Length(height));
        frame.render_widget(Clear, popup);
        let block = Block::bordered().title(Line::from(format!(" {} ", self.field.name())));
        if shown.is_empty() {
            frame.render_widget(Paragraph::new("no match; Esc cancels").block(block), popup);
        } else {
            let list = Choices::new(shown)
                .block(block)
                .highlight_style(Style::new().reversed());
            frame.render_stateful_widget(list, popup, &mut self.at);
        }
    }
}

/// The writes that set `field` to `chosen` on each of `targets`.
pub(crate) fn writes(field: Field, chosen: &Choice, targets: Vec<(String, i64)>) -> Vec<Write> {
    targets
        .into_iter()
        .map(|(key, version)| match field {
            Field::Status => Write::Move {
                key,
                status: chosen.value.clone().unwrap_or_default(),
                version,
            },
            Field::Priority => Write::Set {
                key,
                field: "priority".to_owned(),
                value: chosen.value.clone(),
                version,
            },
            Field::Assignee => Write::Set {
                key,
                field: "assignee".to_owned(),
                value: chosen.value.clone(),
                version,
            },
        })
        .collect()
}

/// Each of `types`' Statuses, as its Workflow lists them.
pub(crate) fn statuses<'a>(
    store: &Store,
    types: impl IntoIterator<Item = &'a str>,
) -> Vec<(String, Vec<Choice>)> {
    let types: BTreeSet<&str> = types.into_iter().collect();
    types
        .into_iter()
        .filter_map(|issue_type| {
            let workflow = store.scheme().workflow(issue_type)?;
            let choices = workflow
                .statuses
                .iter()
                .map(|status| Choice {
                    value: Some(status.id.clone()),
                    label: printable(&status.label),
                })
                .collect();
            Some((issue_type.to_owned(), choices))
        })
        .collect()
}

/// The Statuses every one of `types` has, in the first one's order.
pub(crate) fn shared(statuses: &[(String, Vec<Choice>)], types: &BTreeSet<&str>) -> Vec<Choice> {
    let of = |issue_type: &str| {
        statuses
            .iter()
            .find(|(listed, _)| listed == issue_type)
            .map(|(_, choices)| choices.as_slice())
            .unwrap_or_default()
    };
    let first = types.first().copied().unwrap_or_default();
    of(first)
        .iter()
        .filter(|choice| {
            types.iter().all(|issue_type| {
                of(issue_type)
                    .iter()
                    .any(|other| other.value == choice.value)
            })
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choices(values: &[&str]) -> Vec<Choice> {
        values
            .iter()
            .map(|value| Choice {
                value: Some((*value).to_owned()),
                label: (*value).to_owned(),
            })
            .collect()
    }

    fn press(picker: &mut Picker, code: KeyCode) -> Picked {
        picker.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn chosen(picked: Picked) -> Option<String> {
        match picked {
            Picked::Chosen(_, choice, _) => Some(choice.label),
            _ => None,
        }
    }

    fn opened(field: Field) -> Picker {
        Picker::new(
            field,
            choices(&["low", "lower", "high"]),
            vec![("TM-1".into(), 4)],
        )
    }

    #[test]
    fn up_and_down_stop_at_the_ends() {
        let mut picker = opened(Field::Priority);
        for _ in 0..5 {
            press(&mut picker, KeyCode::Down);
        }
        assert_eq!(
            chosen(press(&mut picker, KeyCode::Enter)),
            Some("high".into())
        );
        let mut picker = opened(Field::Priority);
        press(&mut picker, KeyCode::Down);
        for _ in 0..5 {
            press(&mut picker, KeyCode::Up);
        }
        assert_eq!(
            chosen(press(&mut picker, KeyCode::Enter)),
            Some("low".into())
        );
    }

    #[test]
    fn typing_and_backspace_go_back_to_the_first_choice_shown() {
        let mut picker = opened(Field::Priority);
        press(&mut picker, KeyCode::Down);
        press(&mut picker, KeyCode::Down);
        press(&mut picker, KeyCode::Char('l'));
        assert_eq!(
            chosen(press(&mut picker, KeyCode::Enter)),
            Some("low".into())
        );
        let mut picker = opened(Field::Priority);
        press(&mut picker, KeyCode::Char('w'));
        press(&mut picker, KeyCode::Down);
        press(&mut picker, KeyCode::Backspace);
        assert_eq!(
            chosen(press(&mut picker, KeyCode::Enter)),
            Some("low".into())
        );
    }

    #[test]
    fn enter_with_no_match_cancels_unless_a_name_is_assigned() {
        let mut picker = opened(Field::Status);
        press(&mut picker, KeyCode::Char('z'));
        assert!(matches!(
            press(&mut picker, KeyCode::Enter),
            Picked::Cancelled
        ));
        let mut picker = opened(Field::Assignee);
        press(&mut picker, KeyCode::Char('z'));
        assert_eq!(chosen(press(&mut picker, KeyCode::Enter)), Some("z".into()));
    }

    #[test]
    fn a_choice_writes_its_field_at_the_version_read() {
        let high = &choices(&["high"])[0];
        let nobody = Choice {
            value: None,
            label: "nobody".into(),
        };
        let at = || vec![("TM-1".to_owned(), 4)];
        assert_eq!(
            writes(Field::Priority, high, at()),
            [Write::Set {
                key: "TM-1".into(),
                field: "priority".into(),
                value: Some("high".into()),
                version: 4,
            }]
        );
        assert_eq!(
            writes(Field::Assignee, &nobody, at()),
            [Write::Set {
                key: "TM-1".into(),
                field: "assignee".into(),
                value: None,
                version: 4,
            }]
        );
        assert_eq!(
            writes(Field::Status, high, at()),
            [Write::Move {
                key: "TM-1".into(),
                status: "high".into(),
                version: 4,
            }]
        );
    }

    #[test]
    fn shared_keeps_the_statuses_every_type_has_in_the_first_ones_order() {
        let statuses = vec![
            (
                "bug".to_owned(),
                choices(&["triage", "todo", "fixed", "done"]),
            ),
            ("task".to_owned(), choices(&["todo", "triage", "done"])),
        ];
        let types: BTreeSet<&str> = ["bug", "task"].into();
        let kept: Vec<String> = shared(&statuses, &types)
            .into_iter()
            .map(|choice| choice.label)
            .collect();
        assert_eq!(kept, ["triage", "todo", "done"]);
    }
}
