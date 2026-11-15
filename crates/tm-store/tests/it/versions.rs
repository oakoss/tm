//! An Issue's version, and a write refused when a field it sets changed
//! after the version it was based on.

use serde_json::json;
use tm_core::Change;
use tm_core::actor::SessionId;
use tm_store::{Store, StoreError, WriteOptions};
use uuid::Uuid;

use crate::support::{
    ManualClock, TempDir, claude, draft, field, new_issue, open, set_field, unpassed,
};

fn set(store: &mut Store, issue: Uuid, name: &str, value: &str) -> Result<(), StoreError> {
    set_based(store, issue, name, value, None)
}

fn set_based(
    store: &mut Store,
    issue: Uuid,
    name: &str,
    value: &str,
    based_on: Option<i64>,
) -> Result<(), StoreError> {
    let options = WriteOptions {
        based_on,
        ..WriteOptions::default()
    };
    store
        .write(vec![draft(issue, set_field(name, json!(value)))], &options)
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

    let err = set_based(&mut store, issue, "description", "mine", Some(read)).unwrap_err();
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
    set_based(&mut store, issue, "description", "mine", Some(now)).unwrap();
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
    unpassed(store.append(draft(issue, comment)).unwrap());
    assert_eq!(store.issue_version(issue).unwrap(), read);
}

#[test]
fn a_write_judged_from_a_version_or_a_claim_writes_one_issue() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (issue, other) = (new_issue(&mut store), new_issue(&mut store));
    let read = store.issue_version(issue).unwrap();
    let both = || {
        vec![
            draft(issue, set_field("description", json!("a"))),
            draft(other, set_field("description", json!("b"))),
        ]
    };
    let judged = [
        WriteOptions {
            based_on: Some(read),
            ..WriteOptions::default()
        },
        WriteOptions {
            claim: Some(None),
            ..WriteOptions::default()
        },
    ];
    for options in &judged {
        for drafts in [both(), Vec::new()] {
            let err = store.write(drafts, options).unwrap_err();
            assert!(matches!(err, StoreError::Invalid { .. }), "{err}");
        }
    }
    assert_eq!(field(&store, issue, "description"), None);
}
