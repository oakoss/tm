//! Every failure as a coded document (ADR 0011).

use std::process::ExitCode;

use serde_json::{Map, Value, json};
use tm_core::OpError;
use tm_core::filter::FilterError;
use tm_core::op::{CURRENT_VERSION, MAX_PAYLOAD_BYTES};
use tm_store::workspace::WorkspaceError;
use tm_store::{Claim, StoreError};
use uuid::Uuid;

/// Keys the document itself uses, which details never take.
const RESERVED: [&str; 6] = [
    "schema_version",
    "data",
    "warnings",
    "error",
    "message",
    "fix",
];

/// What an export a restore refuses can still bring in (ADR 0013).
pub(crate) const RECORDS_FIX: &str = "tm import --records FILE";

#[derive(Debug)]
pub(crate) struct CliError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    /// A command that fixes it, when tm can write one.
    pub(crate) fix: Option<String>,
    pub(crate) details: Map<String, Value>,
    /// Warnings the command gathered before it failed.
    pub(crate) warnings: Vec<String>,
}

impl CliError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            fix: None,
            details: Map::new(),
            warnings: Vec::new(),
        }
    }

    /// 2 for a usage error, 1 for any other.
    pub(crate) fn exit_code(&self) -> ExitCode {
        if self.code == "usage" {
            ExitCode::from(2)
        } else {
            ExitCode::FAILURE
        }
    }

    #[must_use]
    pub(crate) fn warned(mut self, warnings: Vec<String>) -> Self {
        self.warnings.extend(warnings);
        self
    }

    #[must_use]
    pub(crate) fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    #[must_use]
    pub(crate) fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        debug_assert!(!RESERVED.contains(&key), "{key} is reserved");
        self.details.insert(key.to_owned(), value.into());
        self
    }

    pub(crate) fn internal(cause: &str, message: impl Into<String>) -> Self {
        Self::new("internal", message).with("cause", cause)
    }

    /// The failure document, carrying the warnings gathered before it.
    pub(crate) fn document(&self, warnings: &[String]) -> Value {
        let mut doc = Map::new();
        doc.insert(
            "schema_version".into(),
            json!(crate::output::SCHEMA_VERSION),
        );
        doc.insert("error".into(), json!(self.code));
        doc.insert("message".into(), json!(self.message));
        if let Some(fix) = &self.fix {
            doc.insert("fix".into(), json!(fix));
        }
        doc.insert(
            "warnings".into(),
            json!(warnings.iter().chain(&self.warnings).collect::<Vec<_>>()),
        );
        for (key, value) in &self.details {
            doc.entry(key.clone()).or_insert_with(|| value.clone());
        }
        Value::Object(doc)
    }
}

// A Claim refusal names who holds the Claim, never its token: a caller that
// retried with a token an error gave would take or free another session's
// Claim. `tm show` gives the token to whoever asks for it.
impl CliError {
    fn already_claimed(
        message: String,
        issue: Uuid,
        held_by: (String, String),
        expires_ms: Option<i64>,
    ) -> Self {
        Self::new("already_claimed", message)
            .with("issue", issue.to_string())
            .with("agent", held_by.0)
            .with("on_behalf_of", held_by.1)
            .with("expires_ms", expires_ms)
    }

    fn not_claim_holder(message: String, issue: Uuid, claim: Uuid, holder: Option<&Claim>) -> Self {
        Self::new("not_claim_holder", message)
            .with("issue", issue.to_string())
            .with("claim", claim.to_string())
            .with(
                "holder",
                holder.map(|held| {
                    json!({
                        "agent": held.agent,
                        "on_behalf_of": held.on_behalf_of,
                        "expires_ms": held.expires_ms,
                    })
                }),
            )
    }
}

impl CliError {
    /// A record import's refusal carries the refused write's own code and
    /// details, placed in the file.
    fn import(err: StoreError, message: String) -> Self {
        match err {
            StoreError::RecordRefused {
                line,
                record,
                cause,
                written,
            } => {
                let written: Vec<String> = written.iter().map(Uuid::to_string).collect();
                let mut refused = Self::from(*cause)
                    .with("line", line)
                    .with("record", record)
                    .with("written", written);
                // The record's source and cause are the file's words.
                refused.message = crate::output::printable(&message);
                refused
            }
            StoreError::ImportChanged(issue) => Self::changed("import_changed", message, issue),
            other => Self::internal(variant(&other), message),
        }
    }

    /// An Issue another writer changed under this write, which a retry reads
    /// again.
    fn changed(code: &'static str, message: String, issue: Uuid) -> Self {
        Self::new(code, message)
            .with("issue", issue.to_string())
            .fix("retry")
    }

    fn missing_fields(message: String, issue: Uuid, fields: Vec<String>) -> Self {
        Self::new("missing_fields", message)
            .with("issue", issue.to_string())
            .with(
                "fields",
                fields
                    .into_iter()
                    .map(|name| json!({ "name": name }))
                    .collect::<Vec<_>>(),
            )
    }

    /// A store, or an export, this command cannot use as it is.
    fn unusable(err: StoreError, message: String) -> Self {
        match err {
            StoreError::NotAStore(path) => {
                Self::new("not_a_store", message).with("path", path.display().to_string())
            }
            StoreError::ReadOnly => Self::new("read_only", message),
            StoreError::WorkspaceMismatch { stored, given } => {
                Self::new("workspace_mismatch", message)
                    .with("stored", stored.to_string())
                    .with("given", given.to_string())
            }
            StoreError::NewerStore { found, supported } => Self::new("upgrade_required", message)
                .with("found", found)
                .with("supported", supported)
                .fix("upgrade tm"),
            StoreError::FormatChanged => Self::new("store_format_changed", message),
            StoreError::SnapshotVersion { found, expected } => {
                Self::new("snapshot_version", message)
                    .with("found", found)
                    .with("expected", expected)
                    .fix(if found > expected {
                        "upgrade tm"
                    } else {
                        "run any write with this tm where it may write the store, which publishes a new Snapshot"
                    })
            }
            StoreError::WorkspaceNotEmpty { workspace, held } => {
                Self::new("workspace_not_empty", message)
                    .with("workspace", workspace.to_string())
                    .with("held", held)
                    .fix(RECORDS_FIX)
            }
            StoreError::InvalidExport { line, .. } => {
                // The reason can quote the file.
                Self::new("invalid_export", crate::output::printable(&message)).with("line", line)
            }
            StoreError::ExportFormat { found, supported } => {
                Self::new("export_format_changed", message)
                    .with("found", found)
                    .with("supported", supported)
                    .fix(RECORDS_FIX)
            }
            StoreError::Io(_) | StoreError::Snapshot { .. } => Self::new("io", message),
            other => Self::internal(variant(&other), message),
        }
    }
}

impl From<StoreError> for CliError {
    fn from(err: StoreError) -> Self {
        let message = err.to_string();
        match err {
            StoreError::Busy(_) => Self::new("busy", message).fix("retry"),
            StoreError::UnknownIssue(id) => Self::new("not_found", message)
                .with("kind", "issue")
                .with("id", id.to_string()),
            StoreError::UnknownProject(id) => Self::new("not_found", message)
                .with("kind", "project")
                .with("id", id.to_string()),
            StoreError::DuplicateProject(id) => {
                Self::new("duplicate_project", message).with("id", id.to_string())
            }
            StoreError::KeyTaken { key, project } => Self::new("key_taken", message)
                .with("key", key)
                .with("project", project.to_string()),
            StoreError::KeysExhausted { key, .. } => {
                Self::new("keys_exhausted", message).with("key", key)
            }
            err @ (StoreError::NotAStore(_)
            | StoreError::ReadOnly
            | StoreError::WorkspaceMismatch { .. }
            | StoreError::NewerStore { .. }
            | StoreError::FormatChanged
            | StoreError::SnapshotVersion { .. }
            | StoreError::WorkspaceNotEmpty { .. }
            | StoreError::InvalidExport { .. }
            | StoreError::ExportFormat { .. }
            | StoreError::Io(_)
            | StoreError::Snapshot { .. }) => Self::unusable(err, message),
            StoreError::UnknownField { field, valid } => Self::new("unknown_field", message)
                .with("field", field)
                .with("valid", valid),
            StoreError::Invalid {
                field,
                reason,
                valid,
            } => Self::new("invalid_value", message)
                .with("field", field)
                .with("reason", reason)
                .with("allowed", valid),
            StoreError::Cycle { relation, path } => Self::new("cycle", message)
                .with("relation", relation)
                .with("path", ids(&path)),
            StoreError::NoAgent { actor } => Self::new("no_agent", message).with("actor", actor),
            StoreError::UnknownBatch(batch) => Self::new("not_found", message)
                .with("kind", "batch")
                .with("reference", batch.to_string()),
            StoreError::UndoConflict { batch, conflicts } => Self::new("undo_conflict", message)
                .with("batch", batch.to_string())
                .with("conflicts", json!(conflicts)),
            StoreError::CannotUndo { batch, .. } => {
                Self::new("cannot_undo", message).with("batch", batch.to_string())
            }
            StoreError::NotLinked {
                link_type,
                from,
                to,
            } => Self::new("not_linked", message)
                .with("type", link_type)
                .with("from", from.to_string())
                .with("to", to.to_string()),
            StoreError::AlreadyClaimed {
                issue,
                agent,
                on_behalf_of,
                expires_ms,
                ..
            } => Self::already_claimed(message, issue, (agent, on_behalf_of), expires_ms),
            StoreError::NotClaimHolder {
                issue,
                claim,
                holder,
            } => Self::not_claim_holder(message, issue, claim, holder.as_deref()),
            StoreError::ClaimChanged(issue) => Self::changed("claim_changed", message, issue),
            StoreError::MissingFields { issue, fields } => {
                Self::missing_fields(message, issue, fields)
            }
            StoreError::Op(err) => Self::from(err),
            err @ (StoreError::RecordRefused { .. } | StoreError::ImportChanged(_)) => {
                Self::import(err, message)
            }
            StoreError::Replay { source, seq, .. } => match *source {
                StoreError::Op(OpError::UnsupportedVersion(found)) => {
                    Self::from(OpError::UnsupportedVersion(found))
                }
                other => Self::internal(variant(&other), message).with("seq", seq),
            },
            StoreError::Filter(err) => Self::from(err),
            StoreError::DuplicateIssue(_)
            | StoreError::SubjectNotProject { .. }
            | StoreError::ProjectMismatch { .. }
            | StoreError::Clock(_)
            | StoreError::Corrupt(_)
            | StoreError::Sqlite(_) => Self::internal(variant(&err), message),
        }
    }
}

impl From<OpError> for CliError {
    fn from(err: OpError) -> Self {
        let message = err.to_string();
        match err {
            OpError::UnsupportedVersion(found) => Self::new("upgrade_required", message)
                .with("found", found)
                .with("supported", CURRENT_VERSION)
                .fix("upgrade tm"),
            OpError::TooLarge(bytes) => Self::new("too_large", message)
                .with("bytes", bytes)
                .with("limit", MAX_PAYLOAD_BYTES),
            OpError::InvalidName { field, .. } | OpError::ReservedField(field) => {
                Self::new("invalid_value", message).with("field", field)
            }
            OpError::InvalidField(field) => {
                Self::new("invalid_value", message).with("field", field)
            }
            OpError::InvalidLinkType(value) | OpError::InvalidProjectKey(value) => {
                Self::new("invalid_value", message).with("value", value)
            }
            OpError::Payload(_) => Self::internal("payload", message),
            OpError::InvalidVersion => Self::internal("invalid_version", message),
            OpError::KindMismatch { .. } => Self::internal("kind_mismatch", message),
        }
    }
}

impl From<FilterError> for CliError {
    fn from(err: FilterError) -> Self {
        let message = err.to_string();
        let mut details = match serde_json::to_value(&err) {
            Ok(Value::Object(details)) => details,
            _ => Map::new(),
        };
        let code = match err {
            FilterError::Syntax { .. } => "syntax",
            FilterError::BareWord { .. } => "bare_word",
            FilterError::Unsupported { .. } => "unsupported",
            FilterError::UnknownField { .. } => "unknown_field",
            FilterError::UnknownValue { .. } => "unknown_value",
            FilterError::InvalidValue { .. } => "invalid_value",
            FilterError::NoIdentity { .. } => "no_identity",
        };
        details.retain(|key, _| !RESERVED.contains(&key.as_str()));
        Self {
            code,
            message,
            fix: None,
            details,
            warnings: Vec::new(),
        }
    }
}

impl From<WorkspaceError> for CliError {
    fn from(err: WorkspaceError) -> Self {
        let message = err.to_string();
        match err {
            WorkspaceError::NoHome | WorkspaceError::RelativeHome { .. } => {
                Self::new("no_home", message)
            }
            WorkspaceError::TwoRepoFiles { first, .. } => {
                Self::new("invalid_repo_file", message).with("path", first.display().to_string())
            }
            WorkspaceError::InvalidRepoFile { path, .. } => {
                Self::new("invalid_repo_file", message).with("path", path.display().to_string())
            }
            WorkspaceError::UnknownWorkspace { id, .. } => Self::new("unknown_workspace", message)
                .with("workspace", id.to_string())
                .fix("tm import FILE"),
            WorkspaceError::IncompleteWorkspace { dir } => {
                Self::new("incomplete_workspace", message).with("dir", dir.display().to_string())
            }
            WorkspaceError::WorkspaceExists { id, .. } => {
                Self::new("workspace_exists", message).with("workspace", id.to_string())
            }
            WorkspaceError::Io { path, .. } => {
                Self::new("io", message).with("path", path.display().to_string())
            }
            WorkspaceError::OpenStore { source, .. } => {
                let mut wrapped = Self::from(*source);
                wrapped.message = message;
                wrapped
            }
            WorkspaceError::Store(err) => Self::from(err),
        }
    }
}

fn ids(ids: &[uuid::Uuid]) -> Vec<String> {
    ids.iter().map(ToString::to_string).collect()
}

/// The variant's name, for `internal`'s `cause`.
fn variant(err: &StoreError) -> &'static str {
    match err {
        StoreError::DuplicateIssue(_) => "duplicate_issue",
        StoreError::SubjectNotProject { .. } => "subject_not_project",
        StoreError::ProjectMismatch { .. } => "project_mismatch",
        StoreError::Clock(_) => "clock",
        StoreError::Corrupt(_) => "corrupt",
        StoreError::Sqlite(_) => "sqlite",
        StoreError::Replay { .. } => "replay",
        _ => "store",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_never_replace_the_documents_own_keys() {
        let err = CliError::from(FilterError::NoIdentity {
            span: tm_core::filter::Span(0, 3),
        });
        let doc = err.document(&["w".into()]);
        assert_eq!(doc["error"], "no_identity");
        assert_eq!(doc["schema_version"], 1);
        assert_eq!(doc["warnings"], json!(["w"]));
        assert_eq!(doc["span"], json!([0, 3]));
    }

    #[test]
    fn a_newer_operation_met_while_replaying_asks_for_an_upgrade() {
        let err = CliError::from(StoreError::Replay {
            seq: 4,
            op_id: None,
            source: Box::new(StoreError::Op(OpError::UnsupportedVersion(9))),
        });
        assert_eq!(err.code, "upgrade_required");
        assert_eq!(err.details["found"], 9);
        assert_eq!(err.fix.as_deref(), Some("upgrade tm"));
    }

    #[test]
    fn a_wrapped_store_error_keeps_its_code() {
        let err = CliError::from(WorkspaceError::OpenStore {
            path: "/s/store.db".into(),
            source: Box::new(StoreError::NewerStore {
                found: 2,
                supported: 1,
            }),
        });
        assert_eq!(err.code, "upgrade_required");
        assert!(err.message.starts_with("/s/store.db"), "{}", err.message);
    }

    #[test]
    fn errors_that_should_not_happen_are_internal_with_a_cause() {
        let err = CliError::from(StoreError::Corrupt("bad row".into()));
        assert_eq!(
            (err.code, &err.details["cause"]),
            ("internal", &json!("corrupt"))
        );
    }
}
