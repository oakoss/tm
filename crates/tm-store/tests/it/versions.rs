//! An Issue's version, and a write refused when a field it sets changed
//! after the version it was based on.

use serde_json::json;
use tm_core::Change;
use tm_core::actor::SessionId;
use tm_store::{Provenance, Store, StoreError, WallTime};
use uuid::Uuid;

use crate::support::{ManualClock, TempDir, claude, draft, field, new_issue, open, set_field};

fn set(store: &mut Store, issue: Uuid, name: &str, value: &str) -> Result<(), StoreError> {
    store
        .append_all(vec![draft(issue, set_field(name, json!(value)))])
        .map(drop)
}

#[test]
fn a_field_changed_after_the_version_refuses_the_write_with_its_value() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let read = store.issue_version(issue).unwrap();
    set(&mut store, issue, "description", "theirs").unwrap();
    assert!(store.issue_version(issue).unwrap() > read);

    store.base_writes_on(issue, read);
    let err = set(&mut store, issue, "description", "mine").unwrap_err();
    let StoreError::Stale {
        based_on, current, ..
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(*based_on, read);
    assert_eq!(current["description"], json!("theirs"));
    assert_eq!(field(&store, issue, "description"), Some(json!("theirs")));
    assert!(matches!(
        store.check_version(issue, read, &["description"]),
        Err(StoreError::Stale { .. })
    ));

    set(&mut store, issue, "title", "another field").unwrap();
    let now = store.issue_version(issue).unwrap();
    store.base_writes_on(issue, now);
    set(&mut store, issue, "description", "mine").unwrap();
}

#[test]
fn a_comment_or_a_claim_moves_no_version() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    let read = store.issue_version(issue).unwrap();
    let agent = claude().in_session(SessionId::parse("s1").unwrap());
    store.take(&agent, issue, Some(3_600_000)).unwrap();
    let comment = Change::Comment {
        body: "a note".into(),
    };
    store.append(draft(issue, comment)).unwrap();
    assert_eq!(store.issue_version(issue).unwrap(), read);
}

#[test]
fn the_base_covers_its_own_issue_and_never_an_import() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let other = new_issue(&mut store);
    let read = store.issue_version(issue).unwrap();
    set(&mut store, issue, "description", "theirs").unwrap();
    set(&mut store, other, "description", "x").unwrap();

    store.base_writes_on(issue, read);
    set(&mut store, other, "description", "y").unwrap();
    let provenance = Provenance {
        source: "test".into(),
        at: WallTime {
            utc_ms: 500,
            offset_min: 0,
        },
    };
    let imported = draft(issue, set_field("description", json!("from elsewhere")));
    store.append_imported(vec![(imported, provenance)]).unwrap();
}
