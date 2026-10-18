//! Operations: the typed, field-level changes the log records (ADR 0006).

use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::Hlc;
use crate::project::ProjectId;

/// The operation format this binary writes and fully understands. Every new
/// `Change` kind raises it, so an older binary refuses the kind by number
/// (ADR 0010).
pub const CURRENT_VERSION: u16 = 2;

/// The first format whose envelope carries `on_behalf_of` and whose names
/// must be printable; v1 operations replay as they were written.
const IDENTITY_SINCE: u16 = 2;

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
    #[error("operation format v{0} has no on_behalf_of, so its hash would leave it out")]
    OnBehalfOfUnsupported(u16),
    #[error("{field} {reason}")]
    InvalidName {
        field: &'static str,
        reason: &'static str,
    },
    #[error("operation kind {kind:?} needs format v{needs}, but the operation is v{v}")]
    KindNewerThanFormat {
        kind: &'static str,
        needs: u16,
        v: u16,
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
}

/// The field a Claim is filed under; no other kind may write it.
pub const CLAIM_FIELD: &str = "claim";

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
    },
    Release {
        claim: Uuid,
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
            Self::Link { .. } => "link",
            Self::Claim { .. } => "claim",
            Self::CreateProject { .. } => "create_project",
            Self::RenameProject { .. } => "rename_project",
        }
    }

    /// The format this kind first appeared in. A new kind takes a new
    /// `CURRENT_VERSION`, so a binary that predates it refuses it by number.
    #[must_use]
    pub const fn introduced_in(&self) -> u16 {
        match self {
            Self::Create
            | Self::SetField { .. }
            | Self::Add { .. }
            | Self::Remove { .. }
            | Self::Comment { .. }
            | Self::Link { .. }
            | Self::Claim { .. }
            | Self::CreateProject { .. }
            | Self::RenameProject { .. } => 1,
        }
    }

    /// Whether the subject is a Project rather than an Issue.
    #[must_use]
    pub const fn is_project(&self) -> bool {
        matches!(
            self,
            Self::CreateProject { .. } | Self::RenameProject { .. }
        )
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
            Self::Create
            | Self::Comment { .. }
            | Self::CreateProject { .. }
            | Self::RenameProject { .. } => None,
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
            Self::Create
            | Self::SetField { .. }
            | Self::Comment { .. }
            | Self::Claim { .. }
            | Self::CreateProject { .. }
            | Self::RenameProject { .. } => None,
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
    /// The human an Agent writes for; none when the human writes.
    pub on_behalf_of: Option<String>,
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
        if self.v < IDENTITY_SINCE {
            if self.on_behalf_of.is_some() {
                return Err(OpError::OnBehalfOfUnsupported(self.v));
            }
        } else {
            check_name("actor", &self.actor)?;
            if let Some(human) = &self.on_behalf_of {
                check_name("on_behalf_of", human)?;
            }
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
        if change.introduced_in() > self.v {
            return Err(OpError::KindNewerThanFormat {
                kind: change.kind(),
                needs: change.introduced_in(),
                v: self.v,
            });
        }
        change.validate()?;
        Ok(change)
    }

    /// The fixed binary envelope of every field the Replica writes. A `v: 1`
    /// operation keeps the layout it was hashed with, so its `op_hash` never
    /// changes; from `v: 2` the layout adds `on_behalf_of` after `actor`.
    #[must_use]
    pub fn envelope(&self) -> Vec<u8> {
        let v1 = self.v < IDENTITY_SINCE;
        let mut buf = Vec::with_capacity(128 + self.payload.len());
        buf.push(if v1 { 1 } else { 2 });
        buf.extend_from_slice(&self.v.to_be_bytes());
        buf.extend_from_slice(self.op_id.as_bytes());
        buf.extend_from_slice(self.replica_id.as_bytes());
        put_bytes(&mut buf, self.actor.as_bytes());
        if !v1 {
            match &self.on_behalf_of {
                Some(human) => {
                    buf.push(1);
                    put_bytes(&mut buf, human.as_bytes());
                }
                None => buf.push(0),
            }
        }
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
    /// Only an operation [`Op::change`] accepts has a meaningful hash; a v1
    /// operation carrying `on_behalf_of` hashes as if it had none.
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

#[cfg(test)]
mod tests {
    use super::*;

    type Mutation = Box<dyn Fn(&mut Op)>;

    /// Changing the envelope layout or hash input changes every Replica's
    /// operation identities, so the values are pinned, one per format.
    const PINNED_OP_HASH: &str = "ee4e794fbfc682546be2fc037644bcbc28ab2809018acaeac0c84f5f5604acb6";
    const PINNED_OP_HASH_V2: &str =
        "ac2176bd4504ae65b92afef3e4b77f930d532c15fb800a3280572d29ecd6faa5";
    const PINNED_OP_HASH_V2_HUMAN: &str =
        "3f5b9a9aa7516a2671669172d75b5e363863d079dae34768e2889970bdde327d";

    /// The kinds each format knows. A row never changes once released; a new
    /// kind goes in a new row with a new `CURRENT_VERSION`.
    const KINDS_BY_FORMAT: [(u16, &[&str]); 2] = [
        (
            1,
            &[
                "create",
                "set_field",
                "add",
                "remove",
                "comment",
                "link",
                "claim",
                "create_project",
                "rename_project",
            ],
        ),
        (
            2,
            &[
                "create",
                "set_field",
                "add",
                "remove",
                "comment",
                "link",
                "claim",
                "create_project",
                "rename_project",
            ],
        ),
    ];

    /// A v1 operation, as stored before format v2.
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

    /// An Agent's operation in the current format.
    fn sample_v2() -> Op {
        Op {
            v: 2,
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
            Change::CreateProject {
                key: "CPL".into(),
                name: "Core".into(),
            },
            Change::RenameProject {
                key: None,
                name: Some("Core".into()),
            },
        ]
    }

    #[test]
    fn op_hash_is_deterministic() {
        assert_eq!(sample().op_hash(), sample().op_hash());
        assert_eq!(sample_v2().op_hash(), sample_v2().op_hash());
    }

    #[test]
    fn op_hash_covers_every_written_field() {
        let base = sample_v2().op_hash();
        let variants: Vec<Mutation> = vec![
            Box::new(|o| o.v = 3),
            Box::new(|o| o.op_id = Uuid::from_u128(9)),
            Box::new(|o| o.replica_id = Uuid::from_u128(9)),
            Box::new(|o| o.actor = "codex".into()),
            Box::new(|o| o.on_behalf_of = None),
            Box::new(|o| o.on_behalf_of = Some("ana".into())),
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
            let mut op = sample_v2();
            change(&mut op);
            assert_ne!(op.op_hash(), base, "variant {i} did not change op_hash");
        }
    }

    #[test]
    fn a_v1_operation_cannot_carry_on_behalf_of() {
        let mut op = sample();
        op.on_behalf_of = Some("jace".into());
        assert!(matches!(
            op.change(),
            Err(OpError::OnBehalfOfUnsupported(1))
        ));
        assert!(sample_v2().change().is_ok());
    }

    #[test]
    fn names_must_be_printable_from_v2() {
        let cases: [(Mutation, &str, &str); 4] = [
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
        ];
        for (mutate, field, reason) in cases {
            let mut op = sample_v2();
            mutate(&mut op);
            assert!(
                matches!(op.change(), Err(OpError::InvalidName { field: f, reason: r }) if f == field && r == reason),
                "{field} {reason}"
            );
        }
        let mut v1 = sample();
        v1.actor = String::new();
        assert!(v1.change().is_ok(), "v1 operations replay as written");
    }

    /// Checks nothing until a kind arrives after format 1, then holds it to
    /// its format.
    #[test]
    fn a_kind_is_refused_in_a_format_before_it() {
        for change in every_change() {
            let needs = change.introduced_in();
            if needs <= 1 {
                continue;
            }
            let op = Op {
                v: needs - 1,
                kind: change.kind().into(),
                payload: serde_json::to_string(&change).unwrap(),
                ..sample()
            };
            assert!(
                matches!(op.change(), Err(OpError::KindNewerThanFormat { .. })),
                "{} accepted at v{}",
                change.kind(),
                needs - 1
            );
        }
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
    fn every_kind_belongs_to_the_format_that_introduced_it() {
        assert_eq!(
            KINDS_BY_FORMAT.last().unwrap().0,
            CURRENT_VERSION,
            "a new format needs its row of kinds"
        );
        for change in every_change() {
            assert!(
                change.introduced_in() <= CURRENT_VERSION,
                "{} needs a new CURRENT_VERSION",
                change.kind()
            );
        }
        for (format, kinds) in KINDS_BY_FORMAT {
            let known: std::collections::BTreeSet<&str> = every_change()
                .iter()
                .filter(|change| change.introduced_in() <= format)
                .map(Change::kind)
                .collect();
            assert_eq!(
                known,
                kinds.iter().copied().collect(),
                "format {format}'s kinds changed; a new kind needs a new format"
            );
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
        assert_eq!(hex(sample().op_hash()), PINNED_OP_HASH);
        assert_eq!(hex(sample_v2().op_hash()), PINNED_OP_HASH_V2);
        let human = Op {
            actor: "jace".into(),
            on_behalf_of: None,
            ..sample_v2()
        };
        assert_eq!(hex(human.op_hash()), PINNED_OP_HASH_V2_HUMAN);
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
            },
        };
        let json = serde_json::to_string(&take).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"claim","action":{"action":"take","agent":"claude","ttl_ms":7200000}}"#
        );
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
