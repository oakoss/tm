//! The contract between the CLI and the Issue screen.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

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
    /// The Claim that lapsed with no other taken since, as `tm show` reports
    /// it.
    #[serde(deserialize_with = "Option::deserialize")]
    pub lapsed_claim: Option<Claim>,
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
    pub expires_ms: Option<i64>,
    /// `null` for a Claim taken with no expiry; 0 once lapsed.
    #[serde(deserialize_with = "Option::deserialize")]
    pub expires_in_ms: Option<i64>,
    /// Handed on by `tm handoff` and not yet taken by the next session.
    pub handed: bool,
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
