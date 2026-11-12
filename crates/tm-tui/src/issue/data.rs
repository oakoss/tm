//! The contract between the CLI and the Issue screen.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;
use tm_core::mention;
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
        let mut shown: Shown = serde_json::from_value(shown)
            .map_err(|err| format!("cannot read tm show's JSON: {err}"))?;
        let history: History = serde_json::from_value(history)
            .map_err(|err| format!("cannot read tm history's JSON: {err}"))?;
        // The text is stored with tokens, which the screen shows by name as
        // `tm show` lists them; `tm history` names its own lines.
        let names: BTreeMap<String, String> = shown
            .mentions
            .iter()
            .map(|mention| (mention.token.clone(), mention.shown.clone()))
            .collect();
        let by_name = |text: &mut String| {
            *text = mention::shown(text, |actor| names.get(&actor.token()).cloned());
        };
        if let Some(Value::String(description)) = shown.fields.get_mut("description") {
            by_name(description);
        }
        for body in shown.comments.iter_mut().filter_map(|c| c.body.as_mut()) {
            by_name(body);
        }
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
    /// The version read, so a write from the screen is refused if the
    /// Issue changed since.
    pub version: i64,
    pub fields: BTreeMap<String, Value>,
    /// The Statuses its Workflow lets it move to, as `tm show` lists them.
    pub moves_to: Vec<Status>,
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
    /// Each mention in the description and comments, as `tm show` shows it.
    pub mentions: Vec<Mention>,
}

#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Mention {
    pub token: String,
    pub shown: String,
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
pub struct Status {
    pub id: String,
    pub label: String,
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn mentions_tm_show_lists_read_by_name_and_others_as_written() {
        let shown = json!({
            "id": Uuid::nil(),
            "key": "TM-1",
            "version": 1,
            "moves_to": [],
            "fields": { "description": "for <@kim@example.com>" },
            "claim": null,
            "lapsed_claim": null,
            "parent": null,
            "links": [],
            "comments": [{
                "actor": "jace",
                "on_behalf_of": null,
                "body": "ask <@kim@example.com> and <@bo@example.com>",
                "utc_ms": 0,
                "retracted": false,
                "role": null,
                "edited_utc_ms": null,
            }],
            "mentions": [{ "token": "<@kim@example.com>", "shown": "@Kim Lee" }],
        });
        let history = json!({
            "total": 1,
            "writes": [{
                "actor": "jace",
                "on_behalf_of": null,
                "utc_ms": 0,
                "changes": [{ "line": "commented: ask <@kim@example.com>" }],
            }],
        });
        let issue = Issue::read(shown, history).unwrap();
        assert_eq!(issue.shown.fields["description"], "for @Kim Lee");
        assert_eq!(
            issue.shown.comments[0].body.as_deref(),
            Some("ask @Kim Lee and <@bo@example.com>")
        );
    }
}
