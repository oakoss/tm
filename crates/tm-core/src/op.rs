//! Operations: the typed, field-level changes the log records (ADR 0006).

use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::project::ProjectId;
use crate::{Hlc, memory, shortlist};

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
    #[error("memory {field} {reason}")]
    InvalidMemory {
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
    /// A question the Issue waits on, written into its comments.
    Ask {
        body: String,
        /// Replies the asker offers; any other reply is taken too.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<String>,
    },
    /// The reply to the Issue's question, written into its comments.
    Answer {
        body: String,
    },
    /// Why the asker settled its own question, written into its comments.
    Withdraw {
        body: String,
    },
    /// Hides a comment, or shows it again; the comment stays in the log.
    Retract {
        /// The id of the operation that added the comment.
        comment: Uuid,
        retracted: bool,
    },
    /// Replaces a comment's text; the text it had stays in the log.
    EditComment {
        /// The id of the operation that added the comment.
        comment: Uuid,
        body: String,
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
    /// A memory; its key and body are written beside it (ADR 0017).
    CreateMemory,
    SetMemory {
        field: memory::Field,
        value: serde_json::Value,
    },
    /// Carries a source an import matched a memory to by key, so a later
    /// import from it finds the memory (ADR 0017).
    MemorySource,
    /// The writer's own name, from git's `user.name`, on the record
    /// [`crate::person::id`] of their email (ADR 0019).
    SetName {
        name: String,
    },
    /// Marks the writer's Notification of operation `op` read or unread, on
    /// the record [`crate::notification::id`] of the two (ADR 0019).
    SetRead {
        op: Uuid,
        read: bool,
    },
    /// Records that its batch wrote `fields`, which Claim `claim` protects,
    /// while `holder` held it for `held_for`, so the holder is notified
    /// (ADR 0009, ADR 0019).
    PassedClaim {
        claim: Uuid,
        holder: String,
        held_for: String,
        fields: Vec<String>,
    },
}

/// The kind of record an operation's subject id names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Subject {
    Issue,
    Project,
    Shortlist,
    Memory,
    Person,
    Notification,
}

/// The field a Claim is filed under; no other kind may write it.
pub const CLAIM_FIELD: &str = "claim";

/// The field a retraction is filed under, one element per comment.
pub const COMMENTS_FIELD: &str = "comments";

/// The field an edit of a comment's text is filed under, one element per
/// comment, apart from [`COMMENTS_FIELD`] so an edit and a retraction never
/// decide each other.
pub const COMMENT_BODY_FIELD: &str = "comment_body";

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
        /// The git branch checked out where it was taken; none outside a
        /// repository or on a detached head.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        /// The files it covers, as [`crate::paths`] keeps them, which another
        /// session is warned before editing.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        paths: Vec<String>,
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
            Self::Ask { .. } => "ask",
            Self::Answer { .. } => "answer",
            Self::Withdraw { .. } => "withdraw",
            Self::Retract { .. } => "retract",
            Self::EditComment { .. } => "edit_comment",
            Self::Link { .. } => "link",
            Self::Claim { .. } => "claim",
            Self::CreateProject { .. } => "create_project",
            Self::RenameProject { .. } => "rename_project",
            Self::CreateShortlist => "create_shortlist",
            Self::SetShortlist { .. } => "set_shortlist",
            Self::ShortlistActor { .. } => "shortlist_actor",
            Self::ShortlistSource => "shortlist_source",
            Self::CreateMemory => "create_memory",
            Self::SetMemory { .. } => "set_memory",
            Self::MemorySource => "memory_source",
            Self::SetName { .. } => "set_name",
            Self::SetRead { .. } => "set_read",
            Self::PassedClaim { .. } => "passed_claim",
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
            Self::CreateMemory | Self::SetMemory { .. } | Self::MemorySource => Subject::Memory,
            Self::SetName { .. } => Subject::Person,
            Self::SetRead { .. } => Subject::Notification,
            Self::Create
            | Self::SetField { .. }
            | Self::Add { .. }
            | Self::Remove { .. }
            | Self::Comment { .. }
            | Self::Ask { .. }
            | Self::Answer { .. }
            | Self::Withdraw { .. }
            | Self::Retract { .. }
            | Self::EditComment { .. }
            | Self::Link { .. }
            | Self::Claim { .. }
            | Self::PassedClaim { .. } => Subject::Issue,
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
            Self::EditComment { .. } => Some(COMMENT_BODY_FIELD),
            Self::SetShortlist { field, .. } => Some(field.as_str()),
            Self::SetMemory { field, .. } => Some(field.as_str()),
            Self::ShortlistActor { .. } => Some(shortlist::ACTORS),
            Self::SetRead { .. } => Some(crate::notification::READ_FIELD),
            Self::Create
            | Self::Comment { .. }
            | Self::Ask { .. }
            | Self::Answer { .. }
            | Self::Withdraw { .. }
            | Self::CreateProject { .. }
            | Self::RenameProject { .. }
            | Self::CreateShortlist
            | Self::ShortlistSource
            | Self::CreateMemory
            | Self::MemorySource
            | Self::SetName { .. }
            | Self::PassedClaim { .. } => None,
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
            Self::Retract { comment, .. } | Self::EditComment { comment, .. } => {
                Some(Cow::Owned(comment.to_string()))
            }
            Self::ShortlistActor { actor, .. } => Some(Cow::Borrowed(actor)),
            Self::Create
            | Self::SetField { .. }
            | Self::Comment { .. }
            | Self::Ask { .. }
            | Self::Answer { .. }
            | Self::Withdraw { .. }
            | Self::Claim { .. }
            | Self::CreateProject { .. }
            | Self::RenameProject { .. }
            | Self::CreateShortlist
            | Self::SetShortlist { .. }
            | Self::ShortlistSource
            | Self::CreateMemory
            | Self::SetMemory { .. }
            | Self::MemorySource
            | Self::SetName { .. }
            | Self::SetRead { .. }
            | Self::PassedClaim { .. } => None,
        }
    }

    /// The value a Short-list or memory field is set to.
    #[must_use]
    pub const fn set_value(&self) -> Option<&serde_json::Value> {
        match self {
            Self::SetShortlist { value, .. } | Self::SetMemory { value, .. } => Some(value),
            _ => None,
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
        if self.field() == Some(COMMENT_BODY_FIELD) && !matches!(self, Self::EditComment { .. }) {
            return Err(OpError::ReservedField(COMMENT_BODY_FIELD));
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
            Self::SetMemory { field, value } => check_memory(*field, value),
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
            Self::PassedClaim {
                holder,
                held_for,
                fields,
                ..
            } => {
                check_name("holder", holder)?;
                check_name("held_for", held_for)?;
                if fields.is_empty() || fields.iter().any(|field| !crate::scheme::is_id(field)) {
                    return Err(OpError::InvalidName {
                        field: "fields",
                        reason: "names no field, or one that is not a field id",
                    });
                }
                Ok(())
            }
            Self::SetName { name } => {
                check_name("name", name)?;
                if name.trim() != name {
                    return Err(OpError::InvalidName {
                        field: "name",
                        reason: "has surrounding spaces",
                    });
                }
                if name.chars().count() > crate::person::MAX_NAME_CHARS {
                    return Err(OpError::InvalidName {
                        field: "name",
                        reason: "is longer than 200 characters",
                    });
                }
                if name.chars().any(crate::person::breaks) {
                    return Err(OpError::InvalidName {
                        field: "name",
                        reason: "holds a line separator or a character that reorders text",
                    });
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// The replies a question may offer, trimmed: none, or two or more distinct
/// ones in words. A number would read as another option's position, so a
/// reply could pick either. Checked where this Replica writes, not on replay.
///
/// # Errors
///
/// Why the options cannot be offered.
pub fn question_options(options: &[String]) -> Result<Vec<String>, String> {
    let options: Vec<String> = options
        .iter()
        .map(|option| option.trim().to_owned())
        .collect();
    if options.iter().any(String::is_empty) {
        return Err("an option is blank".into());
    }
    if options.len() == 1 {
        return Err("offer two or more replies, or none".into());
    }
    for (i, option) in options.iter().enumerate() {
        if is_number(option) {
            return Err(format!(
                "the option {option} reads as a number, which picks an option by position; word it, as \"{option} points\""
            ));
        }
        if options[..i].iter().any(|seen| same_reply(seen, option)) {
            return Err(format!("the option {option} is offered twice"));
        }
    }
    Ok(options)
}

/// The number of the option `reply` is, as an answer records an option it
/// picks: in the option's own words.
#[must_use]
pub fn choice(options: &[String], reply: &str) -> Option<usize> {
    options
        .iter()
        .position(|option| option == reply)
        .map(|at| at + 1)
}

/// Whether two replies match, ignoring case in any script.
#[must_use]
pub fn same_reply(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Whether `text` is a whole number, signed or not, of any size.
#[must_use]
pub fn is_number(text: &str) -> bool {
    let digits = text.trim().trim_start_matches(['-', '+']);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
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

/// Held to on replay too; a body's length is a write-time rule.
fn check_memory(field: memory::Field, value: &serde_json::Value) -> Result<(), OpError> {
    let invalid = |reason| {
        Err(OpError::InvalidMemory {
            field: field.as_str(),
            reason,
        })
    };
    match field {
        memory::Field::Key => match value.as_str().map(memory::check_key) {
            Some(Ok(())) => Ok(()),
            _ => invalid(memory::KEY_RULE),
        },
        memory::Field::Body if value.is_string() => Ok(()),
        memory::Field::Body => invalid("must be text"),
        memory::Field::Forgotten if value.is_boolean() => Ok(()),
        memory::Field::Forgotten => invalid("must be true or false"),
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
    /// The harness session of the Agent that wrote it; none for a human, an
    /// import, or an Agent whose harness names no session.
    pub session: Option<String>,
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
        if self
            .session
            .as_deref()
            .is_some_and(|session| !crate::actor::is_session_id(session))
        {
            return Err(OpError::InvalidName {
                field: "session",
                reason: "holds a character a session id may not",
            });
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
        if matches!(change, Change::MemorySource) && self.source.is_none() {
            return Err(OpError::InvalidName {
                field: "source",
                reason: "is needed by a memory source operation",
            });
        }
        // A record of what happened here, which an import never brings.
        if matches!(change, Change::PassedClaim { .. }) && self.source.is_some() {
            return Err(OpError::InvalidName {
                field: "source",
                reason: "is not imported on a passed Claim",
            });
        }
        // A read mark is the reader's own, on their Notification of `op`.
        if let Change::SetRead { op, .. } = change {
            let reader = crate::notification::writer(&self.actor, self.on_behalf_of.as_deref());
            let reason = if self.source.is_some() {
                "is not imported"
            } else if self.subject != crate::notification::id(op, &reader) {
                "belongs on the writer's own Notification"
            } else {
                return Ok(change);
            };
            return Err(OpError::InvalidName {
                field: "read",
                reason,
            });
        }
        // Only a person names themselves, so a name is never another's guess.
        if matches!(change, Change::SetName { .. }) {
            let reason = if self.on_behalf_of.is_some() || self.session.is_some() {
                "is written by the person it names, not an Agent"
            } else if self.source.is_some() {
                "is not imported"
            } else if self.subject != crate::person::id(&self.actor) {
                "belongs on the record of the email writing it"
            } else {
                return Ok(change);
            };
            return Err(OpError::InvalidName {
                field: "name",
                reason,
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
        // Written only when present, after the length-prefixed payload, so an
        // operation from before sessions were recorded hashes as it did.
        if let Some(session) = &self.session {
            put_bytes(&mut buf, session.as_bytes());
        }
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
            session: None,
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
            Change::EditComment {
                comment: Uuid::nil(),
                body: "fixed".into(),
            },
            Change::Comment {
                body: String::new(),
            },
            Change::Ask {
                body: "which?".into(),
                options: vec!["this".into(), "that".into()],
            },
            Change::Answer {
                body: "this".into(),
            },
            Change::Withdraw {
                body: "settled".into(),
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
            Change::CreateMemory,
            Change::MemorySource,
            Change::SetMemory {
                field: memory::Field::Key,
                value: serde_json::json!("tests-need-race"),
            },
        ]
        .into_iter()
        .chain(records())
        .collect()
    }

    /// The changes to records of their own: names, read marks and passed
    /// Claims.
    fn records() -> Vec<Change> {
        vec![
            Change::SetName {
                name: "Jace Babin".into(),
            },
            Change::SetRead {
                op: Uuid::from_u128(7),
                read: true,
            },
            Change::PassedClaim {
                claim: Uuid::from_u128(8),
                holder: "claude".into(),
                held_for: "jace".into(),
                fields: vec!["status".into()],
            },
        ]
    }

    /// `actor`'s read mark on its Notification of operation 7.
    fn marked(actor: &str, on_behalf_of: Option<&str>) -> Op {
        let change = Change::SetRead {
            op: Uuid::from_u128(7),
            read: true,
        };
        let reader = crate::notification::writer(actor, on_behalf_of);
        Op {
            actor: actor.into(),
            on_behalf_of: on_behalf_of.map(Into::into),
            subject: crate::notification::id(Uuid::from_u128(7), &reader),
            kind: change.kind().into(),
            payload: serde_json::to_string(&change).unwrap(),
            ..sample()
        }
    }

    #[test]
    fn a_passed_claim_names_fields_and_is_never_imported() {
        let passed = |fields: Vec<String>| {
            let change = Change::PassedClaim {
                claim: Uuid::from_u128(8),
                holder: "claude".into(),
                held_for: "jace".into(),
                fields,
            };
            Op {
                kind: change.kind().into(),
                payload: serde_json::to_string(&change).unwrap(),
                ..sample()
            }
        };
        assert!(passed(vec!["status".into()]).change().is_ok());
        let err = passed(Vec::new()).change().unwrap_err().to_string();
        assert!(err.contains("names no field"), "{err}");
        let imported = Op {
            source: Some("beads:tm-1".into()),
            ..passed(vec!["status".into()])
        };
        let err = imported.change().unwrap_err().to_string();
        assert!(err.contains("not imported"), "{err}");
    }

    #[test]
    fn a_read_mark_is_its_readers_own() {
        assert!(marked("kim@example.com", None).change().is_ok());
        assert!(
            marked("claude", Some("kim@example.com")).change().is_ok(),
            "an Agent marks its own"
        );
        let refused = |op: Op, reason: &str| {
            let err = op.change().unwrap_err().to_string();
            assert!(err.contains(reason), "{err}");
        };
        refused(
            Op {
                actor: "jace@example.com".into(),
                ..marked("kim@example.com", None)
            },
            "the writer's own Notification",
        );
        refused(
            Op {
                on_behalf_of: Some("kim@example.com".into()),
                ..marked("kim@example.com", None)
            },
            "the writer's own Notification",
        );
        refused(
            Op {
                source: Some("beads:tm-1".into()),
                ..marked("kim@example.com", None)
            },
            "is not imported",
        );
    }

    /// A name `actor` records, as `sample` writes it.
    fn named(actor: &str, name: &str) -> Op {
        let change = Change::SetName { name: name.into() };
        Op {
            actor: actor.into(),
            subject: crate::person::id(actor),
            kind: change.kind().into(),
            payload: serde_json::to_string(&change).unwrap(),
            ..sample()
        }
    }

    #[test]
    fn a_name_is_its_writers_own_and_fits_on_a_line() {
        assert!(named("jace@example.com", "Jace Babin").change().is_ok());
        let refused = |op: Op, reason: &str| {
            let err = op.change().unwrap_err().to_string();
            assert!(err.contains(reason), "{err}");
        };
        refused(
            Op {
                subject: crate::person::id("kim@example.com"),
                ..named("jace@example.com", "Kim")
            },
            "the record of the email writing it",
        );
        refused(
            Op {
                on_behalf_of: Some("jace@example.com".into()),
                ..named("claude", "Claude")
            },
            "not an Agent",
        );
        refused(
            Op {
                source: Some("beads:tm-1".into()),
                ..named("jace@example.com", "Jace")
            },
            "is not imported",
        );
        refused(
            named("jace@example.com", "Jace\nBabin"),
            "control character",
        );
        refused(named("jace@example.com", " Jace"), "surrounding spaces");
        refused(
            named("jace@example.com", &"j".repeat(201)),
            "longer than 200",
        );
        assert!(named("jace@example.com", &"é".repeat(200)).change().is_ok());
        for breaking in ["Jace\u{2028}Babin", "Jace\u{202E}", "Jace\u{061C}"] {
            refused(named("jace@example.com", breaking), "reorders text");
        }
        for emoji in ["Kim \u{2615}\u{FE0F}", "Kim \u{1F468}\u{200D}\u{1F4BB}"] {
            assert!(named("kim@example.com", emoji).change().is_ok(), "{emoji}");
        }
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
            Box::new(|o| o.session = Some("s1".into())),
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
    fn a_question_without_options_reads_as_before() {
        let plain = r#"{"kind":"ask","body":"which?"}"#;
        let ask = Change::Ask {
            body: "which?".into(),
            options: Vec::new(),
        };
        assert_eq!(serde_json::from_str::<Change>(plain).unwrap(), ask);
        assert_eq!(serde_json::to_string(&ask).unwrap(), plain);
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

    /// The `PINNED_OP_HASH*` operations have no session, so they hash as
    /// before sessions were recorded; one with a session differs.
    #[test]
    fn a_session_is_hashed_only_when_present() {
        let mut op = sample_agent();
        let without = op.op_hash();
        op.session = Some("s1".into());
        let with = op.op_hash();
        assert_ne!(with, without);
        assert_eq!(
            hex(with),
            "32e88ba2a630df90ffe607bfbe3b1d07893809019127c24a8db818e666c93b0a",
            "the bytes a session adds to the hash are fixed"
        );
        op.session = Some("s2".into());
        assert_ne!(op.op_hash(), with);
        op.session = Some("a b".into());
        assert!(matches!(
            op.change(),
            Err(OpError::InvalidName {
                field: "session",
                ..
            })
        ));
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
                branch: Some("feat/x".into()),
                paths: vec!["crates/tm-store/**".into()],
            },
        };
        let json = serde_json::to_string(&take).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"claim","action":{"action":"take","agent":"claude","ttl_ms":7200000,"session":"s1","branch":"feat/x","paths":["crates/tm-store/**"]}}"#
        );
        let earlier = r#"{"kind":"claim","action":{"action":"take","agent":"claude"}}"#;
        assert!(matches!(
            serde_json::from_str::<Change>(earlier).unwrap(),
            Change::Claim {
                action: ClaimAction::Take {
                    session: None,
                    branch: None,
                    ref paths,
                    ..
                }
            } if paths.is_empty()
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
