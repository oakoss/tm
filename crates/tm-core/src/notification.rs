//! Notifications (ADR 0019): derived from the operation that carries a
//! mention, one per Actor it names apart from the writer.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};
use uuid::{Builder, Uuid};

use crate::mention::{self, Mentioned};
use crate::person;

/// The field a read mark is filed under.
pub const READ_FIELD: &str = "read";

/// The Notification `op` gives `recipient`: one id per operation and Actor,
/// so a rebuild finds the same one.
#[must_use]
pub fn id(op: Uuid, recipient: &Mentioned) -> Uuid {
    let digest = Sha256::new()
        .chain_update(b"tm notification\0")
        .chain_update(op.as_bytes())
        .chain_update(recipient.token().as_bytes())
        .finalize();
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    Builder::from_custom_bytes(bytes).into_uuid()
}

/// The Actor writing as `actor`, for `on_behalf_of` when an Agent, as a
/// mention names it.
#[must_use]
pub fn writer(actor: &str, on_behalf_of: Option<&str>) -> Mentioned {
    match on_behalf_of {
        Some(human) => Mentioned::Agent {
            name: actor.to_owned(),
            human: person::key(human),
        },
        None => Mentioned::Person(person::key(actor)),
    }
}

/// The Actor holding a Claim taken by `holder` for `held_for`: an Agent, or
/// the person when they took it themselves.
#[must_use]
pub fn holder(holder: &str, held_for: &str) -> Mentioned {
    if holder.trim().eq_ignore_ascii_case(held_for.trim()) {
        Mentioned::Person(person::key(held_for))
    } else {
        writer(holder, Some(held_for))
    }
}

/// Whom `text` notifies when it replaces `before`: each Actor mentioned
/// outside code that `before` did not mention, apart from `writer`.
#[must_use]
pub fn recipients(text: &str, before: Option<&str>, writer: &Mentioned) -> BTreeSet<Mentioned> {
    if !text.contains("<@") {
        return BTreeSet::new();
    }
    let had: BTreeSet<Mentioned> = before
        .map(|before| {
            mention::tokens(before)
                .into_iter()
                .map(|(_, actor)| actor)
                .collect()
        })
        .unwrap_or_default();
    mention::tokens(text)
        .into_iter()
        .map(|(_, actor)| actor)
        .filter(|actor| actor != writer && !had.contains(actor))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kim() -> Mentioned {
        Mentioned::Person("kim@example.com".into())
    }

    #[test]
    fn one_notification_per_operation_and_recipient() {
        let op = Uuid::from_u128(1);
        assert_eq!(id(op, &kim()), id(op, &kim()));
        assert_ne!(id(op, &kim()), id(Uuid::from_u128(2), &kim()));
        assert_ne!(
            id(op, &kim()),
            id(op, &Mentioned::Person("lee@example.com".into()))
        );
        assert_eq!(id(op, &kim()).get_version_num(), 8);
    }

    #[test]
    fn a_writer_is_named_as_a_mention_names_them() {
        assert_eq!(writer(" Kim@Example.com", None), kim());
        assert_eq!(
            writer("claude", Some("Kim@example.com")),
            Mentioned::Agent {
                name: "claude".into(),
                human: "kim@example.com".into(),
            }
        );
    }

    #[test]
    fn a_claim_is_held_by_its_agent_or_the_person_who_took_it() {
        assert_eq!(
            holder("claude", "Kim@example.com"),
            Mentioned::Agent {
                name: "claude".into(),
                human: "kim@example.com".into(),
            }
        );
        assert_eq!(holder("Kim@example.com", "kim@example.com"), kim());
    }

    #[test]
    fn only_mentions_added_outside_code_notify_and_never_the_writer() {
        let jace = Mentioned::Person("jace@example.com".into());
        let text = "<@kim@example.com> and <@jace@example.com>, `<@lee@example.com>`";
        assert_eq!(recipients(text, None, &jace), BTreeSet::from([kim()]));
        assert!(recipients(text, Some("hi <@kim@example.com>"), &jace).is_empty());
        let agent = Mentioned::Agent {
            name: "claude".into(),
            human: "jace@example.com".into(),
        };
        assert_eq!(
            recipients("<@jace@example.com>", None, &agent),
            BTreeSet::from([jace]),
            "an Agent mentioning its own human notifies them"
        );
    }
}
