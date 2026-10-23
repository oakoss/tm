//! Short-lists (ADR 0016): what a list's name and size may be.

use serde::{Deserialize, Serialize};

/// The size a list is given when none is set.
pub const DEFAULT_SIZE: u64 = 10;

/// The largest size a list may be set to.
pub const MAX_SIZE: u64 = 1000;

/// The set field a list's Actors are filed under.
pub const ACTORS: &str = "actors";

/// A list's single-value fields, each merging on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Name,
    Size,
    /// The tombstone: a deleted list keeps its record, so an undo restores it.
    Deleted,
}

impl Field {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Size => "size",
            Self::Deleted => "deleted",
        }
    }
}

/// The one spelling an Actor's name is filed under, as Actors compare.
#[must_use]
pub fn actor_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Whether `name` can name a list: 1 to 64 letters, digits, `.`, `-` or `_`,
/// starting with a letter or digit, so it reads the same in a filter such as
/// `shortlist:release-0.2`.
#[must_use]
pub fn is_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(is_name("release-0.2"));
        assert!(is_name("docs"));
        assert!(!is_name(""));
        assert!(!is_name("-docs"));
        assert!(!is_name("my list"));
        assert!(!is_name(&"a".repeat(65)));
    }
}
