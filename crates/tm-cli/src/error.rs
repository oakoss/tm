//! Every failure as a coded document (ADR 0011).

use std::fmt::Write as _;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use serde_json::{Map, Value, json};
use tm_core::OpError;
use tm_core::filter::FilterError;
use tm_core::op::{CURRENT_VERSION, MAX_PAYLOAD_BYTES};
use tm_store::workspace::WorkspaceError;
use tm_store::{ClaimState, Holder, Named, StoreError};
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

const SHARED_NAME_FIX: &str = "name a list by the id tm shortlist --all --json gives";
const SHARED_NAME_IMPORT_FIX: &str =
    "rename one with tm shortlist edit ID --name NAME, by the id tm shortlist --all --json gives";

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

    /// A file operation on `path` that failed.
    pub(crate) fn io(path: &Path, err: &io::Error) -> Self {
        Self::new("io", format!("{}: {err}", path.display()))
            .with("path", path.display().to_string())
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

    /// A write refused for another session's Claim names `forced`, the
    /// command that overrides it, in the message and never as a `fix`, so it
    /// is not run by rote (ADR 0009).
    pub(crate) fn past_claim(err: StoreError, forced: &str) -> Self {
        // A Claim handed on is the receiver's to take, which --force skips.
        let taken_with = match &err {
            StoreError::Protected { issue, holder, .. } if holder.handed => Some(format!(
                "; {} takes it with tm start {}",
                holder.agent,
                issue.key.clone().unwrap_or_else(|| issue.id.to_string())
            )),
            _ => None,
        };
        let protected = matches!(err, StoreError::Protected { .. });
        let mut err = Self::from(err);
        if let Some(taken_with) = taken_with {
            err.message.push_str(&taken_with);
        }
        if protected {
            let _ = write!(err.message, "; {forced} writes it anyway");
        }
        err
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

    /// The Issue an error names: its id, and its Key or import record when
    /// the message names it by one.
    #[must_use]
    pub(crate) fn issue(self, issue: Named) -> Self {
        self.with("issue", issue.id.to_string())
            .with("key", issue.key)
            .with("source", issue.source)
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
    /// A take refused for another's Claim, a write to a field it protects,
    /// naming the field, or a write based on a version a field moved past,
    /// with each such field's current value.
    fn refused_write(message: String, err: StoreError) -> Self {
        let (issue, holder, field) = match err {
            StoreError::Stale {
                issue,
                based_on,
                version,
                current,
            } => {
                let shown = issue.key.clone().unwrap_or_else(|| issue.id.to_string());
                return Self::new("stale_value", message)
                    .issue(issue)
                    .with("based_on", based_on)
                    .with("version", version)
                    .with("current", json!(current))
                    .fix(format!("tm show {shown}"));
            }
            StoreError::AlreadyClaimed { issue, holder } => (issue, holder, None),
            StoreError::Protected {
                issue,
                field,
                holder,
            } => (issue, holder, Some(field)),
            other => return Self::internal(variant(&other), message),
        };
        let err = Self::new("already_claimed", message)
            .issue(issue)
            .with("agent", holder.agent.clone())
            .with("on_behalf_of", holder.on_behalf_of.clone())
            .with("expires_ms", holder.expires_ms)
            .with("session", holder.session.clone());
        match field {
            Some(field) => err.with("field", field),
            None => err,
        }
    }

    fn not_claim_holder(message: String, issue: Named, claim: Uuid, holder: &Holder) -> Self {
        let held = match holder {
            Holder::Unclaimed => None,
            Holder::Current(state) => Some(match &**state {
                ClaimState::Held(held) => (held, "holds"),
                ClaimState::Lapsed(held) => (held, "lapsed"),
            }),
            Holder::TookOver(held) => Some((&**held, "took_over")),
        };
        Self::new("not_claim_holder", message)
            .issue(issue)
            .with("claim", claim.to_string())
            .with(
                "holder",
                held.map(|(held, state)| {
                    json!({
                        "agent": held.agent,
                        "on_behalf_of": held.on_behalf_of,
                        "expires_ms": held.expires_ms,
                        "session": held.session,
                        "state": state,
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
                let shared = matches!(*cause, StoreError::ShortlistNameShared { .. });
                let mut refused = Self::from(*cause)
                    .with("line", line)
                    .with("record", record)
                    .with("written", written);
                // An id in a record's list name makes a new list.
                if shared {
                    refused.fix = Some(SHARED_NAME_IMPORT_FIX.into());
                }
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
    fn changed(code: &'static str, message: String, issue: Named) -> Self {
        Self::new(code, message).issue(issue).fix("retry")
    }

    fn cycle(message: String, relation: &'static str, path: Vec<Named>) -> Self {
        let (path, (keys, sources)): (Vec<_>, (Vec<_>, Vec<_>)) = path
            .into_iter()
            .map(|issue| (issue.id.to_string(), (issue.key, issue.source)))
            .unzip();
        Self::new("cycle", message)
            .with("relation", relation)
            .with("path", path)
            .with("keys", keys)
            .with("sources", sources)
    }

    /// A memory no live one in the Project answers to.
    fn memory(err: StoreError, message: String) -> Self {
        match err {
            StoreError::MemoryNotFound(key) => Self::new("not_found", message)
                .with("kind", "memory")
                .with("key", key),
            StoreError::UnknownMemory(id) => Self::not_found("memory", &id, message),
            StoreError::MemoryKeyShared { key, ids } => {
                let ids: Vec<String> = ids.iter().map(Uuid::to_string).collect();
                return Self::new("ambiguous_key", message)
                    .with("kind", "memory")
                    .with("key", key)
                    .with("ids", ids)
                    .fix("tm memories --json");
            }
            other => Self::internal(variant(&other), message),
        }
        .fix("tm memories")
    }

    /// A Short-list refusal, with the fix where one command gives it.
    fn membership(err: StoreError, message: String) -> Self {
        match err {
            StoreError::ShortlistNotFound(name) => Self::new("not_found", message)
                .with("kind", "shortlist")
                .with("name", name)
                .fix("tm shortlist --all"),
            StoreError::UnknownShortlist(id) => {
                Self::not_found("shortlist", &id, message).fix("tm shortlist --all")
            }
            StoreError::ShortlistNameShared { name, count } => {
                Self::new("shortlist_name_shared", message)
                    .with("kind", "shortlist")
                    .with("name", name)
                    .with("count", count)
                    .fix(SHARED_NAME_FIX)
            }
            StoreError::ShortlistFull { name, size } => Self::new("shortlist_full", message)
                .with("name", name.clone())
                .with("size", size)
                .fix(format!(
                    "raise its size with tm shortlist edit {name} --size N, or remove an Issue with tm shortlist remove KEY"
                )),
            StoreError::CannotJoin { issue, reason } => Self::new("cannot_join", message)
                .with("reason", reason)
                .issue(issue),
            StoreError::NotOnShortlist(issue) => Self::new("not_on_shortlist", message)
                .issue(issue)
                .fix("tm shortlist add LIST KEY"),
            other => Self::internal("store", other.to_string()),
        }
    }

    fn not_found(kind: &str, id: &impl ToString, message: String) -> Self {
        Self::new("not_found", message)
            .with("kind", kind)
            .with("id", id.to_string())
    }

    fn not_linked(message: String, link_type: String, from: Named, to: Named) -> Self {
        Self::new("not_linked", message)
            .with("type", link_type)
            .with("from", from.id.to_string())
            .with("from_key", from.key)
            .with("to", to.id.to_string())
            .with("to_key", to.key)
    }

    fn missing_fields(message: String, issue: Named, fields: Vec<String>) -> Self {
        Self::new("missing_fields", message).issue(issue).with(
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
            StoreError::UnknownIssue(id) => Self::not_found("issue", &id, message),
            StoreError::UnknownProject(id) => Self::not_found("project", &id, message),
            err @ (StoreError::UnknownShortlist(_)
            | StoreError::ShortlistNotFound(_)
            | StoreError::ShortlistNameShared { .. }
            | StoreError::ShortlistFull { .. }
            | StoreError::CannotJoin { .. }
            | StoreError::NotOnShortlist(_)) => Self::membership(err, message),
            err @ (StoreError::MemoryNotFound(_)
            | StoreError::UnknownMemory(_)
            | StoreError::MemoryKeyShared { .. }) => Self::memory(err, message),
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
            StoreError::Cycle { relation, path } => Self::cycle(message, relation, path),
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
            } => Self::not_linked(message, link_type, *from, *to),
            err @ (StoreError::AlreadyClaimed { .. }
            | StoreError::Protected { .. }
            | StoreError::Stale { .. }) => Self::refused_write(message, err),
            StoreError::NotClaimHolder {
                issue,
                claim,
                holder,
            } => Self::not_claim_holder(message, issue, claim, &holder),
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
            | StoreError::DuplicateShortlist(_)
            | StoreError::ShortlistMismatch { .. }
            | StoreError::DuplicateMemory(_)
            | StoreError::MemoryMismatch { .. }
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
            OpError::InvalidName { field, .. }
            | OpError::ReservedField(field)
            | OpError::InvalidShortlist { field, .. }
            | OpError::InvalidMemory { field, .. } => {
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
        match &err {
            FilterError::NoIssue { .. } => {
                details.insert("kind".into(), "issue".into());
            }
            FilterError::NoShortlist { .. } => {
                details.insert("kind".into(), "shortlist".into());
            }
            FilterError::ShortlistNameShared { value, .. } => {
                details.insert("kind".into(), "shortlist".into());
                details.insert("name".into(), value.clone().into());
            }
            _ => {}
        }
        let code = match err {
            FilterError::Syntax { .. } => "syntax",
            FilterError::BareWord { .. } => "bare_word",
            FilterError::Unsupported { .. } => "unsupported",
            FilterError::UnknownField { .. } => "unknown_field",
            FilterError::UnknownValue { .. } => "unknown_value",
            FilterError::InvalidValue { .. } => "invalid_value",
            FilterError::NoIdentity { .. } => "no_identity",
            FilterError::NoIssue { .. } | FilterError::NoShortlist { .. } => "not_found",
            FilterError::ShortlistNameShared { .. } => "shortlist_name_shared",
        };
        details.retain(|key, _| !RESERVED.contains(&key.as_str()));
        let fix = match err {
            FilterError::NoShortlist { .. } => Some("tm shortlist --all".into()),
            FilterError::ShortlistNameShared { .. } => Some(SHARED_NAME_FIX.into()),
            _ => None,
        };
        Self {
            code,
            message,
            fix,
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

/// The variant's name, for `internal`'s `cause`.
fn variant(err: &StoreError) -> &'static str {
    match err {
        StoreError::DuplicateIssue(_) => "duplicate_issue",
        StoreError::SubjectNotProject { .. } => "subject_not_project",
        StoreError::ProjectMismatch { .. } => "project_mismatch",
        StoreError::DuplicateShortlist(_) => "duplicate_shortlist",
        StoreError::ShortlistMismatch { .. } => "shortlist_mismatch",
        StoreError::DuplicateMemory(_) => "duplicate_memory",
        StoreError::MemoryMismatch { .. } => "memory_mismatch",
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
    fn a_shared_shortlist_name_reads_alike_from_a_filter_and_a_command() {
        let filter = CliError::from(FilterError::ShortlistNameShared {
            value: "docs".into(),
            span: tm_core::filter::Span(10, 14),
            count: 2,
        });
        let command = CliError::from(StoreError::ShortlistNameShared {
            name: "docs".into(),
            count: 2,
        });
        assert_eq!(
            SHARED_NAME_FIX,
            "name a list by the id tm shortlist --all --json gives"
        );
        for err in [&filter, &command] {
            assert_eq!(err.code, "shortlist_name_shared");
            assert_eq!(err.details["kind"], "shortlist");
            assert_eq!(err.details["name"], "docs");
            assert_eq!(err.details["count"], 2);
            assert_eq!(err.fix.as_deref(), Some(SHARED_NAME_FIX));
        }
        let import = CliError::from(StoreError::RecordRefused {
            line: 3,
            record: "src-1".into(),
            cause: Box::new(StoreError::ShortlistNameShared {
                name: "docs".into(),
                count: 2,
            }),
            written: Vec::new(),
        });
        assert_eq!(
            (import.code, &import.details["name"]),
            ("shortlist_name_shared", &json!("docs"))
        );
        assert_eq!(import.fix.as_deref(), Some(SHARED_NAME_IMPORT_FIX));
        let full = CliError::from(StoreError::RecordRefused {
            line: 3,
            record: "src-1".into(),
            cause: Box::new(StoreError::ShortlistFull {
                name: "docs".into(),
                size: 2,
            }),
            written: Vec::new(),
        });
        assert!(
            full.fix
                .as_deref()
                .is_some_and(|fix| fix.starts_with("raise its size")),
            "{:?}",
            full.fix
        );
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
