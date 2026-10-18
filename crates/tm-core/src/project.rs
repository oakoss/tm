//! Projects. Operations name a Project by its permanent id; its key (`CPL`)
//! and name are labels that can change (ADR 0008).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Deserializes through [`FromStr`], so one id has one spelling everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct ProjectId(Uuid);

impl From<ProjectId> for String {
    fn from(id: ProjectId) -> Self {
        id.to_string()
    }
}

impl TryFrom<String> for ProjectId {
    type Error = ParseProjectIdError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl ProjectId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn uuid(self) -> Uuid {
        self.0
    }
}

impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}

/// The canonical text the operation envelope hashes.
impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.hyphenated().fmt(f)
    }
}

/// A Project key as stored: `^[A-Z][A-Z0-9]{1,9}$`, so `CPL-42` splits at the
/// last dash and reads well in a Key.
#[must_use]
pub fn is_key(key: &str) -> bool {
    let mut chars = key.chars();
    (2..=10).contains(&key.len())
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// The Project key and number of an Issue Key written the one way tm writes
/// it (`CPL-42`), so each number has a single spelling.
#[must_use]
pub fn parse_issue_key(text: &str) -> Option<(&str, i64)> {
    let (prefix, number) = text.rsplit_once('-')?;
    let number: i64 = number.parse().ok().filter(|n| *n > 0)?;
    (is_key(prefix) && format!("{prefix}-{number}") == text).then_some((prefix, number))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a Project id")]
pub struct ParseProjectIdError(String);

impl FromStr for ProjectId {
    type Err = ParseProjectIdError;

    /// Only the canonical hyphenated form, so one id has one spelling.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Uuid::try_parse(text)
            .ok()
            .filter(|id| id.hyphenated().to_string() == text)
            .map(Self)
            .ok_or_else(|| ParseProjectIdError(text.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_through_their_one_spelling() {
        let id = ProjectId::new();
        assert_eq!(id.to_string().parse::<ProjectId>().unwrap(), id);
        let upper = id.to_string().to_uppercase();
        assert!(upper.parse::<ProjectId>().is_err());
        assert!(id.uuid().simple().to_string().parse::<ProjectId>().is_err());
        assert!("CPL".parse::<ProjectId>().is_err());
    }

    #[test]
    fn project_keys_are_short_uppercase_words() {
        for good in ["CP", "CPL", "TM2", "ABCDEFGHIJ"] {
            assert!(is_key(good), "{good}");
        }
        for bad in ["C", "cpl", "2TM", "ABCDEFGHIJK", "CP-L", "CPÉ", ""] {
            assert!(!is_key(bad), "{bad}");
        }
    }

    #[test]
    fn issue_keys_have_one_spelling() {
        assert_eq!(parse_issue_key("CPL-42"), Some(("CPL", 42)));
        for bad in [
            "CPL-007", "CPL-+3", "cpl-1", "CP-L-3", "CPL-0", "CPL-", "CPL", " CPL-1", "CPL-1 ",
            "CPL--1",
        ] {
            assert_eq!(parse_issue_key(bad), None, "{bad}");
        }
    }

    #[test]
    fn serde_accepts_only_the_canonical_spelling() {
        let id = ProjectId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        assert_eq!(serde_json::from_str::<ProjectId>(&json).unwrap(), id);
        let upper = format!("\"{}\"", id.to_string().to_uppercase());
        assert!(serde_json::from_str::<ProjectId>(&upper).is_err());
    }
}
