//! Who a mention may name (ADR 0019).

use serde_json::json;
use tm_core::Actor;
use tm_core::mention::{Known, Mentioned};

use crate::support::{ManualClock, PROJECT, TempDir, claude, draft, open, set_field, unpassed};

#[test]
fn writers_those_agents_write_for_assignees_and_list_actors_may_be_named() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let jace = Actor::human("Jace@Example.com");
    store.record_name(&jace, PROJECT, "Jace Babin").unwrap();
    let (issue, _) = store.create_issue(&claude(), PROJECT, "task", "t").unwrap();
    unpassed(
        store
            .append(draft(
                issue,
                set_field("assignee", json!("ana@example.com")),
            ))
            .unwrap(),
    );
    store
        .create_shortlist(
            &Actor::human("jace"),
            PROJECT,
            "next",
            &["claude".to_owned(), "bo@example.com".to_owned()],
            None,
        )
        .unwrap();
    let known = store.mentionable().unwrap();
    let has = |actor: Mentioned| known.iter().find(|k| k.actor == actor).cloned();
    assert_eq!(
        has(Mentioned::Person("jace@example.com".into())),
        Some(Known {
            actor: Mentioned::Person("jace@example.com".into()),
            name: Some("Jace Babin".into()),
        })
    );
    for key in ["ana@example.com", "bo@example.com", "jace"] {
        assert!(
            has(Mentioned::Person(key.into())).is_some(),
            "{key}: {known:?}"
        );
    }
    assert!(
        has(Mentioned::Agent {
            name: "claude".into(),
            human: "jace".into(),
        })
        .is_some()
    );
    assert!(
        has(Mentioned::Person("claude".into())).is_none(),
        "a list Actor named as an Agent is the Agent: {known:?}"
    );
}
