//! Operations: the typed, field-level changes the log records (ADR 0006).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::Hlc;
use crate::project::ProjectId;

/// The operation format this binary writes and fully understands.
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
}

/// One change to one Issue. Field changes carry the display labels of the
/// field and of an enumerated value at the time, so history stays readable
/// after a rename.
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
        }
    }

    /// The field this change targets, if any.
    #[must_use]
    pub fn field(&self) -> Option<&str> {
        match self {
            Self::SetField { field, .. } | Self::Add { field, .. } | Self::Remove { field, .. } => {
                Some(field)
            }
            Self::Create | Self::Comment { .. } => None,
        }
    }

    /// The set element this change targets, if any.
    #[must_use]
    pub fn elem(&self) -> Option<&str> {
        match self {
            Self::Add { elem, .. } | Self::Remove { elem, .. } => Some(elem),
            Self::Create | Self::SetField { .. } | Self::Comment { .. } => None,
        }
    }

    /// Rejects field names that cannot be used safely as JSON object keys and
    /// filter qualifiers.
    ///
    /// # Errors
    ///
    /// [`OpError::InvalidField`] when the field name is not `[a-z][a-z0-9_]*`.
    pub fn validate(&self) -> Result<(), OpError> {
        if let Some(field) = self.field()
            && !crate::scheme::is_id(field)
        {
            return Err(OpError::InvalidField(field.to_owned()));
        }
        Ok(())
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
    pub project: ProjectId,
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

fn put_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).expect("envelope field longer than 4 GiB");
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    type Mutation = Box<dyn Fn(&mut Op)>;

    /// Changing the envelope layout or hash input changes every Replica's
    /// operation identities, so the value is pinned.
    const PINNED_OP_HASH: &str = "ee4e794fbfc682546be2fc037644bcbc28ab2809018acaeac0c84f5f5604acb6";

    fn sample() -> Op {
        let change = Change::SetField {
            field: "status".into(),
            label: "Status".into(),
            value: serde_json::json!("active"),
            value_label: None,
        };
        Op {
            v: CURRENT_VERSION,
            op_id: Uuid::from_u128(1),
            replica_id: Uuid::from_u128(2),
            actor: "jace".into(),
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

    #[test]
    fn op_hash_is_deterministic() {
        assert_eq!(sample().op_hash(), sample().op_hash());
    }

    #[test]
    fn op_hash_covers_every_written_field() {
        let base = sample().op_hash();
        let variants: Vec<Mutation> = vec![
            Box::new(|o| o.v = 2),
            Box::new(|o| o.op_id = Uuid::from_u128(9)),
            Box::new(|o| o.replica_id = Uuid::from_u128(9)),
            Box::new(|o| o.actor = "codex".into()),
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
            let mut op = sample();
            change(&mut op);
            assert_ne!(op.op_hash(), base, "variant {i} did not change op_hash");
        }
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
        let hex = sample().op_hash().iter().fold(String::new(), |mut hex, b| {
            use std::fmt::Write;
            write!(hex, "{b:02x}").unwrap();
            hex
        });
        assert_eq!(hex, PINNED_OP_HASH);
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
        let changes = [
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
            Change::Comment {
                body: String::new(),
            },
        ];
        for change in changes {
            let tag = serde_json::to_value(&change).unwrap()["kind"].clone();
            assert_eq!(tag, change.kind());
        }
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
