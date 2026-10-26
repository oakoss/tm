//! Operations: the typed, field-level changes the log records (ADR 0006).

use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::project::ProjectId;
use crate::{Hlc, shortlist};

/// The operation format this binary writes and fully understands. Until tm
/// v1 is released the format changes in place (ADR 0010).
pub const CURRENT_VERSION: u16 = 1;

/// Version of the binary envelope `op_hash` is computed over.
const ENVELOPE_FORMAT: u8 = 1;

/// Domain prefix for `op_hash`; chain hashes use a different one.
const OP_HASH_PREFIX: u8 = 0x00;

/// Bounds every envelope field, so hashing never meets an unbounded one.
pub const MAX_PAYLOAD_BYTES: usize = 1 << 20;

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("operation payload is not valid: {0}")]
    Payload(#[from] serde_json::Error),
    #[error("operation format v{0} is newer than this tm understands; upgrade tm")]
    UnsupportedVersion(u16),
    #[error("operation format v0 does not exist")]
    InvalidVersion,
    #[error("{field} {reason}")]
    InvalidName {
        field: &'static str,
        reason: &'static str,
    },
    #[error("operation kind {stored:?} does not match its payload, which is {payload:?}")]
    KindMismatch {
        stored: String,
        payload: &'static str,
    },
    #[error("operation payload is {0} bytes; the limit is {MAX_PAYLOAD_BYTES}")]
    TooLarge(usize),
    #[error(
        "field name {0:?} must be lowercase letters, digits and underscores, starting with a letter"
    )]
    InvalidField(String),
    #[error("field {0:?} belongs to another operation kind")]
    ReservedField(&'static str),
    #[error(
        "Link type {0:?} must be lowercase letters, digits and underscores, starting with a letter"
    )]
    InvalidLinkType(String),
    #[error(
        "Project key {0:?} must be an uppercase letter then 1 to 9 uppercase letters or digits"
    )]
    InvalidProjectKey(String),
    #[error("Short-list {field} {reason}")]
    InvalidShortlist {
        field: &'static str,
        reason: &'static str,
    },
}

/// One change to one record, whose kind [`Change::subject`] names. Field
/// changes carry the display labels of the field and of an enumerated value
/// at the time, so history stays readable after a rename.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    Create,
    SetField {
        field: String,
        label: String,
        value: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value_label: Option<String>,
    },
    Add {
        field: String,
        label: String,
        elem: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elem_label: Option<String>,
    },
    Remove {
        field: String,
        label: String,
        elem: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elem_label: Option<String>,
    },
    Comment {
        body: String,
    },
    /// Hides a comment, or shows it again; the comment stays in the log.
    Retract {
        /// The id of the operation that added the comment.
        comment: Uuid,
        retracted: bool,
    },
    /// Adds or removes a Link, filed under its canonical source Issue.
    Link {
        link_type: String,
        target: Uuid,
        present: bool,
    },
    /// Takes or releases the Issue's Claim (ADR 0009).
    Claim {
        action: ClaimAction,
    },
    /// A Project; the operation's subject and Project are both its id.
    CreateProject {
        key: String,
        name: String,
    },
    /// A new key, name or both; the old key keeps resolving.
    RenameProject {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// A Short-list; its name, size and Actors are written beside it (ADR 0016).
    CreateShortlist,
    SetShortlist {
        field: shortlist::Field,
        value: serde_json::Value,
    },
    /// Adds or removes an Actor who works through a Short-list.
    ShortlistActor {
        actor: String,
        present: bool,
    },
    /// Carries a source an import matched a Short-list to by name, so a later
    /// import from it finds the list (ADR 0016).
    ShortlistSource,
}

/// The kind of record an operation's subject id names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Subject {
    Issue,
    Project,
    Shortlist,
}

/// The field a Claim is filed under; no other kind may write it.
pub const CLAIM_FIELD: &str = "claim";

/// The field a retraction is filed under, one element per comment.
pub const COMMENTS_FIELD: &str = "comments";

/// A take applies only while the Issue's current Claim is still `prev`, and
/// a release only while it is still `claim`, so server order decides between
/// concurrent ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ClaimAction {
    Take {
        agent: String,
        /// How long the Claim lasts; none holds it until released.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl_ms: Option<u64>,
        /// The Claim the taker saw as current; none when the Issue was unclaimed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prev: Option<Uuid>,
        /// The harness session that took it, telling apart sessions that
        /// share an Agent name; none when the harness names no session.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
    /// Extends the Claim `claim`, lapsed or not, to `ttl_ms` from when it is
    /// written; it applies only while that Claim still holds the Issue's row.
    Renew {
        claim: Uuid,
        ttl_ms: u64,
    },
    Release {
        claim: Uuid,
    },
    /// Replaces the Claim `claim`, lapsed or not, with one held for `agent`
    /// in no session yet: the first of its sessions to take the Issue binds
    /// it. It lasts `ttl_ms` from when it is written.
    Hand {
        claim: Uuid,
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl_ms: Option<u64>,
        /// What the receiver needs to carry on, also written as a comment;
        /// here it travels with the Claim to every Replica.
        note: String,
    },
}

impl Change {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::SetField { .. } => "set_field",
            Self::Add { .. } => "add",
            Self::Remove { .. } => "remove",
            Self::Comment { .. } => "comment",
            Self::Retract { .. } => "retract",
            Self::Link { .. } => "link",
            Self::Claim { .. } => "claim",
            Self::CreateProject { .. } => "create_project",
            Self::RenameProject { .. } => "rename_project",
            Self::CreateShortlist => "create_shortlist",
            Self::SetShortlist { .. } => "set_shortlist",
            Self::ShortlistActor { .. } => "shortlist_actor",
            Self::ShortlistSource => "shortlist_source",
        }
    }

    /// What kind of record the operation's subject is.
    #[must_use]
    pub const fn subject(&self) -> Subject {
        match self {
            Self::CreateProject { .. } | Self::RenameProject { .. } => Subject::Project,
            Self::CreateShortlist
            | Self::SetShortlist { .. }
            | Self::ShortlistActor { .. }
            | Self::ShortlistSource => Subject::Shortlist,
            Self::Create
            | Self::SetField { .. }
            | Self::Add { .. }
            | Self::Remove { .. }
            | Self::Comment { .. }
            | Self::Retract { .. }
            | Self::Link { .. }
            | Self::Claim { .. } => Subject::Issue,
        }
    }

    /// The field this change targets, if any.
    #[must_use]
    pub fn field(&self) -> Option<&str> {
        match self {
            Self::SetField { field, .. } | Self::Add { field, .. } | Self::Remove { field, .. } => {
                Some(field)
            }
            Self::Link { .. } => Some(crate::link::FIELD),
            Self::Claim { .. } => Some(CLAIM_FIELD),
            Self::Retract { .. } => Some(COMMENTS_FIELD),
            Self::SetShortlist { field, .. } => Some(field.as_str()),
            Self::ShortlistActor { .. } => Some(shortlist::ACTORS),
            Self::Create
            | Self::Comment { .. }
            | Self::CreateProject { .. }
            | Self::RenameProject { .. }
            | Self::CreateShortlist
            | Self::ShortlistSource => None,
        }
    }

    /// The set element this change targets, if any.
    #[must_use]
    pub fn elem(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::Add { elem, .. } | Self::Remove { elem, .. } => Some(Cow::Borrowed(elem)),
            Self::Link {
                link_type, target, ..
            } => Some(Cow::Owned(crate::link::elem(link_type, *target))),
            Self::Retract { comment, .. } => Some(Cow::Owned(comment.to_string())),
            Self::ShortlistActor { actor, .. } => Some(Cow::Borrowed(actor)),
            Self::Create
            | Self::SetField { .. }
            | Self::Comment { .. }
            | Self::Claim { .. }
            | Self::CreateProject { .. }
            | Self::RenameProject { .. }
            | Self::CreateShortlist
            | Self::SetShortlist { .. }
            | Self::ShortlistSource => None,
        }
    }

    /// Rejects field names that cannot be used safely as JSON object keys and
    /// filter qualifiers, and Project keys not in their stored form.
    ///
    /// # Errors
    ///
    /// [`OpError::InvalidField`] when the field name is not `[a-z][a-z0-9_]*`,
    /// or [`OpError::InvalidProjectKey`].
    pub fn validate(&self) -> Result<(), OpError> {
        if let Some(field) = self.field()
            && !crate::scheme::is_id(field)
        {
            return Err(OpError::InvalidField(field.to_owned()));
        }
        // Another kind writing a Link's element would merge with it unseen.
        if self.field() == Some(crate::link::FIELD) && !matches!(self, Self::Link { .. }) {
            return Err(OpError::ReservedField(crate::link::FIELD));
        }
        if self.field() == Some(CLAIM_FIELD) && !matches!(self, Self::Claim { .. }) {
            return Err(OpError::ReservedField(CLAIM_FIELD));
        }
        if self.field() == Some(COMMENTS_FIELD) && !matches!(self, Self::Retract { .. }) {
            return Err(OpError::ReservedField(COMMENTS_FIELD));
        }
        if let Self::Link { link_type, .. } = self
            && !crate::scheme::is_id(link_type)
        {
            return Err(OpError::InvalidLinkType(link_type.clone()));
        }
        let key = match self {
            Self::CreateProject { key, .. } => Some(key),
            Self::RenameProject { key, .. } => key.as_ref(),
            _ => None,
        };
        if let Some(key) = key
            && !crate::project::is_key(key)
        {
            return Err(OpError::InvalidProjectKey(key.clone()));
        }
        match self {
            Self::SetShortlist { field, value } => check_shortlist(*field, value),
            Self::ShortlistActor { actor, .. } => {
                check_name("actor", actor)?;
                // One spelling per Actor, so adds and removes meet as one element.
                if *actor != shortlist::actor_key(actor) {
                    return Err(OpError::InvalidShortlist {
                        field: "actor",
                        reason: "must be lowercase, without surrounding spaces",
                    });
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Held to on replay too, so a value another Replica wrote is read the same way.
fn check_shortlist(field: shortlist::Field, value: &serde_json::Value) -> Result<(), OpError> {
    let invalid = |reason| {
        Err(OpError::InvalidShortlist {
            field: field.as_str(),
            reason,
        })
    };
    match field {
        shortlist::Field::Name => match value.as_str() {
            Some(name) if shortlist::is_name(name) => Ok(()),
            _ => invalid(
                "must be 1 to 64 letters, digits, '.', '-' or '_', starting with a letter or digit",
            ),
        },
        shortlist::Field::Size => match value.as_u64() {
            Some(size) if (1..=shortlist::MAX_SIZE).contains(&size) => Ok(()),
            _ => invalid("must be a whole number from 1 to 1000"),
        },
        shortlist::Field::Deleted if value.is_boolean() => Ok(()),
        shortlist::Field::Deleted => invalid("must be true or false"),
    }
}

/// An operation as stored. `payload` holds the JSON written at creation and is
/// never re-serialized, so `op_hash` stays reproducible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Op {
    pub v: u16,
    pub op_id: Uuid,
    pub replica_id: Uuid,
    pub actor: String,
    /// The human an Agent writes for; none when the human writes.
    pub on_behalf_of: Option<String>,
    /// Where an imported operation came from, such as `beads:tm-u8g.10`;
    /// none for any other write (ADR 0013).
    pub source: Option<String>,
    pub project: ProjectId,
    /// What the operation changes: an Issue, or a Project for Project kinds.
    pub subject: Uuid,
    pub kind: String,
    pub hlc: Hlc,
    pub base_seq: u64,
    pub utc_ms: i64,
    pub offset_min: i16,
    pub payload: String,
}

impl Op {
    /// Parses the payload, holding a stored operation to the same rules as a new write.
    ///
    /// # Errors
    ///
    /// The [`OpError`] for whichever format rule the operation breaks.
    pub fn change(&self) -> Result<Change, OpError> {
        match self.v {
            0 => return Err(OpError::InvalidVersion),
            v if v > CURRENT_VERSION => return Err(OpError::UnsupportedVersion(v)),
            _ => {}
        }
        check_name("actor", &self.actor)?;
        if let Some(human) = &self.on_behalf_of {
            check_name("on_behalf_of", human)?;
        }
        if let Some(source) = &self.source {
            check_name("source", source)?;
        }
        if self.payload.len() > MAX_PAYLOAD_BYTES {
            return Err(OpError::TooLarge(self.payload.len()));
        }
        let change: Change = serde_json::from_str(&self.payload)?;
        if change.kind() != self.kind {
            return Err(OpError::KindMismatch {
                stored: self.kind.clone(),
                payload: change.kind(),
            });
        }
        change.validate()?;
        if matches!(change, Change::ShortlistSource) && self.source.is_none() {
            return Err(OpError::InvalidName {
                field: "source",
                reason: "is needed by a Short-list source operation",
            });
        }
        Ok(change)
    }

    /// The fixed binary envelope of every field the Replica writes.
    #[must_use]
    pub fn envelope(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(128 + self.payload.len());
        buf.push(ENVELOPE_FORMAT);
        buf.extend_from_slice(&self.v.to_be_bytes());
        buf.extend_from_slice(self.op_id.as_bytes());
        buf.extend_from_slice(self.replica_id.as_bytes());
        put_bytes(&mut buf, self.actor.as_bytes());
        put_optional(&mut buf, self.on_behalf_of.as_deref());
        put_optional(&mut buf, self.source.as_deref());
        put_bytes(&mut buf, self.project.to_string().as_bytes());
        buf.extend_from_slice(self.subject.as_bytes());
        put_bytes(&mut buf, self.kind.as_bytes());
        buf.extend_from_slice(&self.hlc.raw().to_be_bytes());
        buf.extend_from_slice(&self.base_seq.to_be_bytes());
        buf.extend_from_slice(&self.utc_ms.to_be_bytes());
        buf.extend_from_slice(&self.offset_min.to_be_bytes());
        put_bytes(&mut buf, self.payload.as_bytes());
        buf
    }

    /// The operation's identity: SHA-256 over a domain prefix and the envelope.
    #[must_use]
    pub fn op_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update([OP_HASH_PREFIX]);
        hasher.update(self.envelope());
        hasher.finalize().into()
    }
}

fn check_name(field: &'static str, name: &str) -> Result<(), OpError> {
    let reason = if name.trim().is_empty() {
        "is blank"
    } else if name.chars().any(char::is_control) {
        "has a control character"
    } else {
        return Ok(());
    };
    Err(OpError::InvalidName { field, reason })
}

fn put_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).expect("envelope field longer than 4 GiB");
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(bytes);
}

/// A presence byte, then the value when there is one.
fn put_optional(buf: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(value) => {
            buf.push(1);
            put_bytes(buf, value.as_bytes());
        }
        None => buf.push(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Mutation = Box<dyn Fn(&mut Op)>;

    /// Changing the envelope layout or hash input changes every Replica's
    /// operation identities, so the values are pinned: a human's operation,
    /// an Agent's and an imported one.
    const PINNED_OP_HASH: &str = "a8955b9eb128f2db92a808af4e475a422bd2a61e9f6648a92b0b4c5dc029d2c8";
    const PINNED_OP_HASH_AGENT: &str =
        "29e25b29652cf0e3df4c449c845e2ae89d960ca289235c51c16d07522fff66fc";
    const PINNED_OP_HASH_IMPORTED: &str =
        "26d70f3f0460e6d9b31b82fae2ec7c19a79c14d1f1c45bb5f4783228ea37cf13";

    /// A human's operation.
    fn sample() -> Op {
        let change = Change::SetField {
            field: "status".into(),
            label: "Status".into(),
            value: serde_json::json!("active"),
            value_label: None,
        };
        Op {
            v: 1,
            op_id: Uuid::from_u128(1),
            replica_id: Uuid::from_u128(2),
            actor: "jace".into(),
            on_behalf_of: None,
            source: None,
            project: ProjectId::from_uuid(Uuid::from_u128(4)),
            subject: Uuid::from_u128(3),
            kind: change.kind().into(),
            hlc: Hlc::new(1_000, 0),
            base_seq: 0,
            utc_ms: 1_000,
            offset_min: -300,
            payload: serde_json::to_string(&change).unwrap(),
        }
    }

    /// A Short-list source operation without a source records nothing, so
    /// replay refuses it as a write does.
    #[test]
    fn a_list_source_operation_needs_a_source() {
        let change = Change::ShortlistSource;
        let mut op = Op {
            kind: change.kind().into(),
            payload: serde_json::to_string(&change).unwrap(),
            ..sample()
        };
        let refused = op.change().unwrap_err().to_string();
        assert_eq!(refused, "source is needed by a Short-list source operation");
        op.source = Some("tm:0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".into());
        assert_eq!(op.change().unwrap(), change);
    }

    /// An Agent's operation.
    fn sample_agent() -> Op {
        Op {
            actor: "claude".into(),
            on_behalf_of: Some("jace".into()),
            ..sample()
        }
    }

    fn hex(hash: [u8; 32]) -> String {
        hash.iter().fold(String::new(), |mut hex, b| {
            use std::fmt::Write;
            write!(hex, "{b:02x}").unwrap();
            hex
        })
    }

    /// One change of every kind; `every_change_has_every_kind` fails until a
    /// new kind is added.
    fn every_change() -> Vec<Change> {
        vec![
            Change::Create,
            Change::SetField {
                field: "a".into(),
                label: String::new(),
                value: serde_json::Value::Null,
                value_label: None,
            },
            Change::Add {
                field: "a".into(),
                label: String::new(),
                elem: "x".into(),
                elem_label: None,
            },
            Change::Remove {
                field: "a".into(),
                label: String::new(),
                elem: "x".into(),
                elem_label: None,
            },
            Change::Retract {
                comment: Uuid::nil(),
                retracted: true,
            },
            Change::Comment {
                body: String::new(),
            },
            Change::Link {
                link_type: "blocks".into(),
                target: Uuid::from_u128(5),
                present: true,
            },
            Change::Claim {
                action: ClaimAction::Release {
                    claim: Uuid::from_u128(6),
                },
            },
            Change::Claim {
                action: ClaimAction::Renew {
                    claim: Uuid::from_u128(6),
                    ttl_ms: 7_200_000,
                },
            },
            Change::Claim {
                action: ClaimAction::Hand {
                    claim: Uuid::from_u128(6),
                    agent: "codex".into(),
                    ttl_ms: Some(7_200_000),
                    note: "the parser is half done".into(),
                },
            },
            Change::CreateProject {
                key: "CPL".into(),
                name: "Core".into(),
            },
            Change::RenameProject {
                key: None,
                name: Some("Core".into()),
            },
            Change::CreateShortlist,
            Change::SetShortlist {
                field: shortlist::Field::Name,
                value: serde_json::json!("docs"),
            },
            Change::ShortlistActor {
                actor: "claude".into(),
                present: true,
            },
            Change::ShortlistSource,
        ]
    }

    #[test]
    fn op_hash_is_deterministic() {
        assert_eq!(sample().op_hash(), sample().op_hash());
        assert_eq!(sample_agent().op_hash(), sample_agent().op_hash());
    }

    #[test]
    fn op_hash_covers_every_written_field() {
        let base = sample_agent().op_hash();
        let variants: Vec<Mutation> = vec![
            Box::new(|o| o.v = 2),
            Box::new(|o| o.op_id = Uuid::from_u128(9)),
            Box::new(|o| o.replica_id = Uuid::from_u128(9)),
            Box::new(|o| o.actor = "codex".into()),
            Box::new(|o| o.on_behalf_of = None),
            Box::new(|o| o.on_behalf_of = Some("ana".into())),
            Box::new(|o| o.source = Some("beads:tm-1".into())),
            Box::new(|o| o.project = ProjectId::from_uuid(Uuid::from_u128(9))),
            Box::new(|o| o.subject = Uuid::from_u128(9)),
            Box::new(|o| o.kind = "add".into()),
            Box::new(|o| o.hlc = Hlc::new(1_000, 1)),
            Box::new(|o| o.base_seq = 1),
            Box::new(|o| o.utc_ms = 1_001),
            Box::new(|o| o.offset_min = 0),
            Box::new(|o| o.payload.push(' ')),
        ];
        for (i, change) in variants.iter().enumerate() {
            let mut op = sample_agent();
            change(&mut op);
            assert_ne!(op.op_hash(), base, "variant {i} did not change op_hash");
        }
    }

    #[test]
    fn names_must_be_printable() {
        let cases: [(Mutation, &str, &str); 6] = [
            (Box::new(|o| o.actor = " ".into()), "actor", "is blank"),
            (
                Box::new(|o| o.actor = "cl\naude".into()),
                "actor",
                "has a control character",
            ),
            (
                Box::new(|o| o.on_behalf_of = Some(" ".into())),
                "on_behalf_of",
                "is blank",
            ),
            (
                Box::new(|o| o.on_behalf_of = Some("ja\u{7}ce".into())),
                "on_behalf_of",
                "has a control character",
            ),
            (
                Box::new(|o| o.source = Some(String::new())),
                "source",
                "is blank",
            ),
            (
                Box::new(|o| o.source = Some("beads:\ntm-1".into())),
                "source",
                "has a control character",
            ),
        ];
        for (mutate, field, reason) in cases {
            let mut op = sample_agent();
            mutate(&mut op);
            assert!(
                matches!(op.change(), Err(OpError::InvalidName { field: f, reason: r }) if f == field && r == reason),
                "{field} {reason}"
            );
        }
        assert!(sample_agent().change().is_ok());
    }

    #[test]
    fn short_list_values_are_checked_as_written_and_replayed() {
        let set = |field, value| Change::SetShortlist { field, value };
        let actor = |name: &str| Change::ShortlistActor {
            actor: name.into(),
            present: true,
        };
        let refused = [
            set(shortlist::Field::Name, serde_json::json!("my list")),
            set(shortlist::Field::Size, serde_json::json!(0)),
            set(shortlist::Field::Size, serde_json::json!(1001)),
            set(shortlist::Field::Deleted, serde_json::json!("x")),
            actor(""),
            actor("Codex"),
            actor(" codex"),
        ];
        for change in refused {
            assert!(
                matches!(
                    change.validate(),
                    Err(OpError::InvalidShortlist { .. } | OpError::InvalidName { .. })
                ),
                "{change:?}"
            );
        }
        set(shortlist::Field::Deleted, serde_json::json!(false))
            .validate()
            .unwrap();
        actor("codex").validate().unwrap();
    }

    #[test]
    fn every_change_has_every_kind() {
        // serde's error for an unknown tag lists every tag it knows.
        let err = serde_json::from_str::<Change>(r#"{"kind":"?"}"#)
            .unwrap_err()
            .to_string();
        let listed = err
            .split("expected one of ")
            .nth(1)
            .expect("serde lists the known tags");
        let tags: std::collections::BTreeSet<&str> = listed
            .split(", ")
            .map(|tag| tag.trim_matches(|c: char| c == '`' || !c.is_ascii_graphic()))
            .map(|tag| tag.split('`').next().unwrap_or(tag))
            .collect();
        let kinds: std::collections::BTreeSet<&str> =
            every_change().iter().map(Change::kind).collect();
        assert_eq!(kinds, tags);
    }

    #[test]
    fn length_prefixes_keep_adjacent_fields_apart() {
        let encode = |fields: [&str; 2]| {
            let mut buf = Vec::new();
            for field in fields {
                put_bytes(&mut buf, field.as_bytes());
            }
            buf
        };
        assert_ne!(encode(["ab", "c"]), encode(["a", "bc"]));
    }

    #[test]
    fn change_round_trips_through_payload() {
        let op = sample();
        assert_eq!(op.change().unwrap().kind(), "set_field");
    }

    #[test]
    fn value_labels_round_trip_and_are_omitted_when_absent() {
        let labelled = Change::SetField {
            field: "status".into(),
            label: "Status".into(),
            value: serde_json::json!("in_review"),
            value_label: Some("In Review".into()),
        };
        let json = serde_json::to_string(&labelled).unwrap();
        assert_eq!(serde_json::from_str::<Change>(&json).unwrap(), labelled);
        assert!(!sample().payload.contains("value_label"));
    }

    #[test]
    fn newer_format_is_refused() {
        let mut op = sample();
        op.v = CURRENT_VERSION + 1;
        assert!(matches!(op.change(), Err(OpError::UnsupportedVersion(_))));
    }

    #[test]
    fn op_hash_has_a_fixed_value() {
        let imported = Op {
            source: Some("beads:tm-u8g.10".into()),
            ..sample()
        };
        let got = [sample(), sample_agent(), imported].map(|op| hex(op.op_hash()));
        assert_eq!(
            got,
            [
                PINNED_OP_HASH,
                PINNED_OP_HASH_AGENT,
                PINNED_OP_HASH_IMPORTED
            ]
        );
    }

    #[test]
    fn version_zero_is_refused() {
        let mut op = sample();
        op.v = 0;
        assert!(matches!(op.change(), Err(OpError::InvalidVersion)));
    }

    #[test]
    fn a_kind_that_disagrees_with_the_payload_is_refused() {
        let mut op = sample();
        op.kind = "comment".into();
        assert!(matches!(op.change(), Err(OpError::KindMismatch { .. })));
    }

    #[test]
    fn a_stored_invalid_field_name_is_refused() {
        let mut op = sample();
        op.payload = op.payload.replace("status", "a.b");
        assert!(matches!(op.change(), Err(OpError::InvalidField(_))));
    }

    #[test]
    fn an_oversized_payload_is_refused() {
        let mut op = sample();
        op.payload = "x".repeat(MAX_PAYLOAD_BYTES + 1);
        assert!(matches!(op.change(), Err(OpError::TooLarge(_))));
    }

    #[test]
    fn kind_names_match_the_serialized_tag() {
        for change in every_change() {
            let tag = serde_json::to_value(&change).unwrap()["kind"].clone();
            assert_eq!(tag, change.kind());
        }
    }

    #[test]
    fn project_keys_in_operations_must_be_stored_uppercase() {
        let ok = Change::RenameProject {
            key: Some("CORE".into()),
            name: None,
        };
        assert!(ok.validate().is_ok());
        for bad in ["cpl", "C", "CP-L"] {
            let change = Change::CreateProject {
                key: bad.into(),
                name: "x".into(),
            };
            assert!(
                matches!(change.validate(), Err(OpError::InvalidProjectKey(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_link_targets_its_type_and_target_under_links() {
        let target = Uuid::from_u128(5);
        let link = Change::Link {
            link_type: "blocks".into(),
            target,
            present: false,
        };
        assert_eq!(link.field(), Some("links"));
        assert_eq!(link.elem().as_deref(), Some(&*format!("blocks/{target}")));
        let bad = Change::Link {
            link_type: "Blocks".into(),
            target,
            present: true,
        };
        assert!(matches!(bad.validate(), Err(OpError::InvalidLinkType(_))));
        let forged = Change::Remove {
            field: "links".into(),
            label: String::new(),
            elem: format!("blocks/{target}"),
            elem_label: None,
        };
        assert!(matches!(
            forged.validate(),
            Err(OpError::ReservedField("links"))
        ));
    }

    #[test]
    fn a_claim_round_trips_and_owns_the_claim_field() {
        let take = Change::Claim {
            action: ClaimAction::Take {
                agent: "claude".into(),
                ttl_ms: Some(7_200_000),
                prev: None,
                session: Some("s1".into()),
            },
        };
        let json = serde_json::to_string(&take).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"claim","action":{"action":"take","agent":"claude","ttl_ms":7200000,"session":"s1"}}"#
        );
        let earlier = r#"{"kind":"claim","action":{"action":"take","agent":"claude"}}"#;
        assert!(matches!(
            serde_json::from_str::<Change>(earlier).unwrap(),
            Change::Claim {
                action: ClaimAction::Take { session: None, .. }
            }
        ));
        assert_eq!(serde_json::from_str::<Change>(&json).unwrap(), take);
        assert_eq!(take.field(), Some("claim"));
        let forged = Change::SetField {
            field: "claim".into(),
            label: String::new(),
            value: serde_json::json!("x"),
            value_label: None,
        };
        assert!(matches!(
            forged.validate(),
            Err(OpError::ReservedField("claim"))
        ));
    }

    #[test]
    fn field_names_are_validated() {
        let ok = Change::Add {
            field: "file_paths".into(),
            label: "Files".into(),
            elem: "a".into(),
            elem_label: None,
        };
        assert!(ok.validate().is_ok());
        for bad in ["", "Status", "1x", "a.b", "a\"b", "a-b"] {
            let change = Change::SetField {
                field: bad.into(),
                label: String::new(),
                value: serde_json::Value::Null,
                value_label: None,
            };
            assert!(change.validate().is_err(), "{bad:?} accepted");
        }
    }
}
