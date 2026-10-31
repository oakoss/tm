//! Memories (ADR 0017): what a memory's key and body may be.

use serde::{Deserialize, Serialize};

/// The most characters a memory's body may hold; `tm prime` shows its first
/// line, so a longer note belongs in a comment or a doc.
pub const MAX_BODY: usize = 500;

/// How many words of a body an automatic key is made from.
const KEY_WORDS: usize = 4;

/// A memory's single-value fields, each merging on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Key,
    Body,
    /// The tombstone: a forgotten memory keeps its record, so an undo restores it.
    Forgotten,
}

impl Field {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Key => "key",
            Self::Body => "body",
            Self::Forgotten => "forgotten",
        }
    }
}

pub const KEY_RULE: &str =
    "must be 1 to 64 lowercase letters, digits or '-', starting with a letter or digit";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyError {
    Blank,
    /// Its length in characters.
    Long(usize),
}

impl std::fmt::Display for BodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blank => f.write_str("is blank"),
            Self::Long(count) => write!(
                f,
                "is {count} characters; the limit is {MAX_BODY}, so a longer note belongs in a comment (tm comment KEY) or a doc"
            ),
        }
    }
}

/// # Errors
///
/// [`KEY_RULE`] for a key [`is_key`] refuses.
pub fn check_key(key: &str) -> Result<(), &'static str> {
    if is_key(key) { Ok(()) } else { Err(KEY_RULE) }
}

/// # Errors
///
/// The [`BodyError`] the body breaks.
pub fn check_body(body: &str) -> Result<(), BodyError> {
    if body.trim().is_empty() {
        return Err(BodyError::Blank);
    }
    let count = body.chars().count();
    if count > MAX_BODY {
        return Err(BodyError::Long(count));
    }
    Ok(())
}

/// Whether `key` can name a memory: 1 to 64 lowercase letters, digits or
/// `-`, starting with a letter or digit.
#[must_use]
pub fn is_key(key: &str) -> bool {
    (1..=64).contains(&key.len())
        && key.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A key made from the first words of `body`, or `memory` when none of it is
/// a letter or digit.
#[must_use]
pub fn key_from(body: &str) -> String {
    let words: Vec<String> = body
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(KEY_WORDS)
        .map(str::to_ascii_lowercase)
        .collect();
    let mut key = words.join("-");
    key.truncate(64);
    let key = key.trim_end_matches('-').to_owned();
    if key.is_empty() { "memory".into() } else { key }
}

/// The words memories are compared by: lowercase, three characters or more.
#[must_use]
pub fn words(text: &str) -> std::collections::BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect()
}

/// How alike two bodies read: the share of their words they hold in common.
#[must_use]
pub fn likeness(a: &str, b: &str) -> f64 {
    let (a, b) = (words(a), words(b));
    let all = a.union(&b).count();
    if all == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let share = a.intersection(&b).count() as f64 / all as f64;
    share
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert!(is_key("tests-need-race"));
        assert!(is_key("2fa"));
        assert!(!is_key(""));
        assert!(!is_key("-race"));
        assert!(!is_key("Race"));
        assert!(!is_key("needs race"));
        assert!(!is_key(&"a".repeat(65)));
    }

    #[test]
    fn a_key_is_made_from_the_first_words() {
        assert_eq!(
            key_from("Run the tests with -race, always"),
            "run-the-tests-with"
        );
        assert_eq!(key_from("  SQLite's WAL  "), "sqlite-s-wal");
        assert_eq!(key_from("—"), "memory");
        assert!(is_key(&key_from(&"word ".repeat(40))));
    }

    #[test]
    fn likeness_counts_shared_words() {
        assert!(likeness("tests need the race flag", "the race flag tests need") > 0.99);
        assert!(likeness("tests need -race", "deploys go through staging") < 0.1);
        assert!(likeness("", "") < 0.1);
    }
}
