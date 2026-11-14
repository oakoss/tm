//! What each kind of operation is to the rest of tm: how undo treats it,
//! whether an import may carry it, and where a Notification it causes was
//! written. Every answer matches every kind with no catch-all arm, so a new
//! kind does not compile until it is placed here.

use crate::notification::Site;
use crate::op::{Change, ClaimAction};

/// How undoing a batch treats one of its operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Undo {
    /// The field or element it wrote is restored to what it held before.
    Restore,
    /// It created its record, which undo retires as a whole.
    Created,
    /// A take, hand or release, which the Claim's own rules undo.
    Claim,
    /// It records what happened rather than changing anything, so it stays.
    Leave,
    /// Its batch cannot be undone, for this reason.
    Refuse(&'static str),
}

/// The kinds whose batches a bare undo never offers, as [`Change::undo`]
/// refuses them; a test keeps the two in step.
pub const UNDO_REFUSED_KINDS: [&str; 3] = ["create_project", "rename_project", "set_name"];

impl Change {
    /// How undoing its batch treats this operation.
    #[must_use]
    pub const fn undo(&self) -> Undo {
        match self {
            Self::SetField { .. }
            | Self::Add { .. }
            | Self::Remove { .. }
            | Self::Comment { .. }
            | Self::Ask { .. }
            | Self::Answer { .. }
            | Self::Withdraw { .. }
            | Self::Retract { .. }
            | Self::EditComment { .. }
            | Self::Link { .. }
            | Self::SetShortlist { .. }
            | Self::ShortlistActor { .. }
            | Self::SetMemory { .. }
            | Self::SetRead { .. } => Undo::Restore,
            Self::Create | Self::CreateShortlist | Self::CreateMemory => Undo::Created,
            // A renewal records that the session was active (ADR 0009).
            Self::Claim {
                action: ClaimAction::Renew { .. },
            }
            | Self::ShortlistSource
            | Self::MemorySource
            | Self::PassedClaim { .. } => Undo::Leave,
            Self::Claim {
                action:
                    ClaimAction::Take { .. } | ClaimAction::Hand { .. } | ClaimAction::Release { .. },
            } => Undo::Claim,
            Self::CreateProject { .. } | Self::RenameProject { .. } => {
                Undo::Refuse("it wrote a Project, which undo does not revert")
            }
            Self::SetName { .. } => Undo::Refuse(
                "it recorded a name, which undo does not revert; change git's user.name and write again",
            ),
        }
    }

    /// Whether it records what happened rather than changing anything.
    #[must_use]
    pub const fn records_only(&self) -> bool {
        matches!(self.undo(), Undo::Leave)
    }

    /// The field a refusal names when an import may not bring it in.
    #[must_use]
    pub const fn import_refusal(&self) -> Option<&'static str> {
        match self {
            Self::Claim { .. } | Self::PassedClaim { .. } => Some(crate::op::CLAIM_FIELD),
            Self::SetName { .. } => Some("name"),
            Self::SetRead { .. } => Some(crate::notification::READ_FIELD),
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
            | Self::CreateProject { .. }
            | Self::RenameProject { .. }
            | Self::CreateShortlist
            | Self::SetShortlist { .. }
            | Self::ShortlistActor { .. }
            | Self::ShortlistSource
            | Self::CreateMemory
            | Self::SetMemory { .. }
            | Self::MemorySource => None,
        }
    }

    /// Where a Notification it causes was written; none for a kind that
    /// never notifies.
    #[must_use]
    pub fn site(&self) -> Option<Site> {
        match self {
            Self::Comment { .. } => Some(Site::Comment),
            Self::Ask { .. } => Some(Site::Question),
            Self::Answer { .. } => Some(Site::Answer),
            Self::Withdraw { .. } => Some(Site::Withdrawal),
            Self::EditComment { .. } => Some(Site::EditedComment),
            Self::SetField { field, .. } => (field == "description").then_some(Site::Description),
            Self::PassedClaim { .. } => Some(Site::Claim),
            Self::Create
            | Self::Add { .. }
            | Self::Remove { .. }
            | Self::Retract { .. }
            | Self::Link { .. }
            | Self::Claim { .. }
            | Self::CreateProject { .. }
            | Self::RenameProject { .. }
            | Self::CreateShortlist
            | Self::SetShortlist { .. }
            | Self::ShortlistActor { .. }
            | Self::ShortlistSource
            | Self::CreateMemory
            | Self::SetMemory { .. }
            | Self::MemorySource
            | Self::SetName { .. }
            | Self::SetRead { .. } => None,
        }
    }

    /// Whether only an import writes it: a source a later import finds a
    /// record by.
    #[must_use]
    pub const fn source_only(&self) -> bool {
        match self {
            Self::ShortlistSource | Self::MemorySource => true,
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
            | Self::CreateProject { .. }
            | Self::RenameProject { .. }
            | Self::CreateShortlist
            | Self::SetShortlist { .. }
            | Self::ShortlistActor { .. }
            | Self::CreateMemory
            | Self::SetMemory { .. }
            | Self::SetName { .. }
            | Self::SetRead { .. }
            | Self::PassedClaim { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::op::tests::every_change;

    #[test]
    fn every_kind_states_how_undo_and_imports_treat_it() {
        for change in every_change() {
            let undo = change.undo();
            let refused = matches!(undo, Undo::Refuse(_));
            assert_eq!(
                UNDO_REFUSED_KINDS.contains(&change.kind()),
                refused,
                "{} and UNDO_REFUSED_KINDS disagree",
                change.kind()
            );
            assert_eq!(change.records_only(), undo == Undo::Leave);
            // Undo restores what a kind wrote by its field; a comment's own
            // operation is restored through the comments field instead.
            let comment = matches!(
                change,
                Change::Comment { .. }
                    | Change::Ask { .. }
                    | Change::Answer { .. }
                    | Change::Withdraw { .. }
            );
            if undo == Undo::Restore && !comment {
                assert!(change.field().is_some(), "{} names no field", change.kind());
            }
            if change.source_only() {
                assert_eq!(undo, Undo::Leave, "{} is a source", change.kind());
            }
        }
    }

    #[test]
    fn each_kind_is_placed_as_tm_treats_it() {
        let renew = Change::Claim {
            action: ClaimAction::Renew {
                claim: uuid::Uuid::nil(),
                ttl_ms: 1,
            },
        };
        let named = Change::SetName {
            name: "Jace".into(),
        };
        let comment = Change::Comment { body: "x".into() };
        assert_eq!(renew.undo(), Undo::Leave);
        assert!(renew.records_only());
        assert_eq!(renew.import_refusal(), Some(crate::op::CLAIM_FIELD));
        assert!(matches!(named.undo(), Undo::Refuse(_)));
        assert_eq!(named.import_refusal(), Some("name"));
        assert_eq!(comment.undo(), Undo::Restore);
        assert_eq!(comment.import_refusal(), None);
        assert!(Change::MemorySource.source_only());
        assert!(!comment.source_only());
        assert_eq!(Change::CreateMemory.undo(), Undo::Created);
    }

    #[test]
    fn a_stored_kind_reads_back_as_the_site_its_change_gave() {
        let description = Change::SetField {
            field: "description".into(),
            label: String::new(),
            value: serde_json::Value::Null,
            value_label: None,
        };
        assert_eq!(description.site(), Some(Site::Description));
        for change in every_change().into_iter().chain([description]) {
            match (&change, change.site()) {
                (Change::SetField { field, .. }, site) => {
                    assert_eq!(site.is_some(), field == "description", "{field}");
                }
                (_, site) => assert_eq!(Site::of_kind(change.kind()), site, "{}", change.kind()),
            }
        }
    }
}
