//! Key tables: a screen's keys, which also write its footer hints and help,
//! so neither can name a key the screen does not act on.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A key as a table names it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Key {
    code: KeyCode,
    ctrl: bool,
}

pub(crate) const fn plain(code: KeyCode) -> Key {
    Key { code, ctrl: false }
}

pub(crate) const fn ch(c: char) -> Key {
    plain(KeyCode::Char(c))
}

/// Ctrl and `c`, which a terminal reports in lower case.
pub(crate) const fn ctrl(c: char) -> Key {
    assert!(c.is_ascii_lowercase());
    Key {
        code: KeyCode::Char(c),
        ctrl: true,
    }
}

impl Key {
    /// A plain key matches whatever modifiers are held with it; a Ctrl key
    /// needs Ctrl among them.
    fn matches(self, key: KeyEvent) -> bool {
        self.code == key.code && (!self.ctrl || key.modifiers.contains(KeyModifiers::CONTROL))
    }

    fn name(self) -> String {
        let name = match self.code {
            KeyCode::Char(' ') => "Space".to_owned(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Up => "↑".to_owned(),
            KeyCode::Down => "↓".to_owned(),
            KeyCode::PageUp => "PgUp".to_owned(),
            KeyCode::PageDown => "PgDn".to_owned(),
            KeyCode::Backspace => "Bksp".to_owned(),
            KeyCode::Enter => "Enter".to_owned(),
            KeyCode::Esc => "Esc".to_owned(),
            other => other.to_string(),
        };
        if self.ctrl {
            format!("Ctrl-{}", name.to_uppercase())
        } else {
            name
        }
    }
}

/// One line of a screen's key table.
pub(crate) struct Binding<A> {
    pub(crate) keys: &'static [Key],
    pub(crate) action: A,
    /// The help line's words; neighbours saying the same share a line.
    pub(crate) does: &'static str,
    /// The footer's word for it, when the footer names it; neighbours with
    /// the same word share a hint.
    pub(crate) hint: Option<&'static str>,
}

/// The action `key` names in `table`, if any.
pub(crate) fn action<A: Copy>(table: &[Binding<A>], key: KeyEvent) -> Option<A> {
    table
        .iter()
        .find(|binding| binding.keys.iter().any(|named| named.matches(key)))
        .map(|binding| binding.action)
}

/// The help lines `table` gives: its keys' names and what they do.
pub(crate) fn help<A>(table: &[Binding<A>]) -> Vec<(String, &'static str)> {
    let mut lines: Vec<(Vec<String>, &'static str)> = Vec::new();
    for binding in table {
        let names = binding.keys.iter().map(|key| key.name());
        match lines.last_mut() {
            Some((keys, does)) if *does == binding.does => keys.extend(names),
            _ => lines.push((names.collect(), binding.does)),
        }
    }
    lines
        .into_iter()
        .map(|(keys, does)| (keys.join(" "), does))
        .collect()
}

/// The footer hints `table` gives, each its keys' first names then its word.
pub(crate) fn hints<A>(table: &[Binding<A>]) -> Vec<String> {
    let mut hints: Vec<(Vec<String>, &'static str)> = Vec::new();
    for binding in table {
        let (Some(hint), Some(first)) = (binding.hint, binding.keys.first()) else {
            continue;
        };
        match hints.last_mut() {
            Some((keys, word)) if *word == hint => keys.push(first.name()),
            _ => hints.push((vec![first.name()], hint)),
        }
    }
    hints
        .into_iter()
        .map(|(keys, word)| format!("{} {word}", keys.join(" ")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Act {
        Down,
        Up,
        Quit,
    }

    const TABLE: [Binding<Act>; 3] = [
        Binding {
            keys: &[ch('j'), plain(KeyCode::Down)],
            action: Act::Down,
            does: "down and up",
            hint: Some("move"),
        },
        Binding {
            keys: &[ch('k'), plain(KeyCode::Up)],
            action: Act::Up,
            does: "down and up",
            hint: Some("move"),
        },
        Binding {
            keys: &[ctrl('c')],
            action: Act::Quit,
            does: "quit",
            hint: None,
        },
    ];

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn a_ctrl_key_needs_ctrl_and_a_plain_key_takes_any_modifiers() {
        let none = KeyModifiers::NONE;
        assert_eq!(action(&TABLE, press(KeyCode::Down, none)), Some(Act::Down));
        let c = KeyCode::Char('c');
        let ctrl_alt = KeyModifiers::CONTROL | KeyModifiers::ALT;
        assert_eq!(action(&TABLE, press(c, ctrl_alt)), Some(Act::Quit));
        assert_eq!(action(&TABLE, press(c, none)), None);
        for held in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SHIFT,
        ] {
            let k = press(KeyCode::Char('k'), held);
            assert_eq!(action(&TABLE, k), Some(Act::Up), "{held:?}");
        }
    }

    #[test]
    fn help_and_hints_come_from_the_table() {
        assert_eq!(
            help(&TABLE),
            [
                ("j ↓ k ↑".to_owned(), "down and up"),
                ("Ctrl-C".to_owned(), "quit")
            ]
        );
        assert_eq!(hints(&TABLE), ["j k move"]);
    }
}
