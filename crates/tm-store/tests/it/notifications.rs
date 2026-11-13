//! Notifications (ADR 0019): derived from the operations carrying mentions,
//! read marks a record of their own.

use serde_json::json;
use tm_core::{Actor, Change};
use tm_store::{Draft, Notification, Store, StoreError};
use uuid::Uuid;

use crate::support::{
    ManualClock, PROJECT, TempDir, claude, draft, dump, jace, new_issue, open, set_field,
};

fn kim() -> Actor {
    Actor::human("kim@example.com")
}

fn lee() -> Actor {
    Actor::human("lee@example.com")
}

fn write(store: &mut Store, issue: Uuid, change: Change) -> Uuid {
    store.append(draft(issue, change)).unwrap().op_id
}

fn comment(store: &mut Store, issue: Uuid, body: &str) -> Uuid {
    write(store, issue, Change::Comment { body: body.into() })
}

fn unread(store: &Store, actor: &Actor) -> Vec<Notification> {
    store.notifications(actor, false).unwrap()
}

fn issue_ops(store: &Store, issue: Uuid) -> i64 {
    store
        .connection()
        .query_row(
            "SELECT count(*) FROM ops WHERE issue_id = ?1",
            [issue.as_bytes()],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn a_mention_notifies_each_actor_once_but_not_the_writer_or_code() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let said = comment(
        &mut store,
        issue,
        "<@kim@example.com>, <@kim@example.com> and <@jace>; `<@lee@example.com>`",
    );
    let listed = unread(&store, &kim());
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].op_id, said);
    assert_eq!(listed[0].issue_id, issue);
    assert_eq!(listed[0].kind, "comment");
    assert!(!listed[0].read);
    assert!(unread(&store, &jace()).is_empty(), "the writer");
    assert!(unread(&store, &lee()).is_empty(), "a token in code");
    let before = dump(&store);
    store.rebuild().unwrap();
    assert_eq!(dump(&store), before);
    assert_eq!(unread(&store, &kim()), listed);
}

#[test]
fn marking_read_and_undoing_it_never_touch_the_issue() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    comment(&mut store, issue, "<@kim@example.com>");
    let version = store.issue_version(issue).unwrap();
    let updated = store.issue(issue).unwrap().unwrap().fields["updated"].clone();
    let ops = issue_ops(&store, issue);
    let listed = unread(&store, &kim());
    let marked = store.mark_read(&kim(), &listed).unwrap();
    assert_eq!(unread(&store, &kim()).len(), 0);
    assert!(store.notifications(&kim(), true).unwrap()[0].read);
    store.rebuild().unwrap();
    assert!(
        unread(&store, &kim()).is_empty(),
        "the mark survives a rebuild"
    );
    let all = store.notifications(&kim(), true).unwrap();
    assert_eq!(
        store.mark_read(&kim(), &all).unwrap().len(),
        0,
        "a read one is not marked again"
    );
    store.undo(&kim(), marked[0].op_id).unwrap();
    assert_eq!(unread(&store, &kim()).len(), 1, "undo leaves it unread");
    assert_eq!(store.issue_version(issue).unwrap(), version);
    assert_eq!(
        store.issue(issue).unwrap().unwrap().fields["updated"],
        updated
    );
    assert_eq!(
        issue_ops(&store, issue),
        ops,
        "nothing in the Issue's history"
    );
}

#[test]
fn only_the_recipient_marks_a_notification() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    comment(&mut store, issue, "<@kim@example.com>");
    let listed = unread(&store, &kim());
    let refused = store.mark_read(&lee(), &listed);
    assert!(
        matches!(refused, Err(StoreError::Op(_) | StoreError::Invalid { .. })),
        "{refused:?}"
    );
    let forged = store.append(Draft {
        actor: lee(),
        project: PROJECT,
        subject: tm_core::notification::id(
            listed[0].op_id,
            &tm_core::notification::writer("lee@example.com", None),
        ),
        change: Change::SetRead {
            op: listed[0].op_id,
            read: true,
        },
    });
    assert!(
        matches!(forged, Err(StoreError::Invalid { .. })),
        "a mark on a Notification lee never received: {forged:?}"
    );
    assert_eq!(unread(&store, &kim()).len(), 1);
}

#[test]
fn an_edit_notifies_only_the_mentions_it_adds() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    write(
        &mut store,
        issue,
        set_field("description", json!("for <@kim@example.com>")),
    );
    write(
        &mut store,
        issue,
        set_field(
            "description",
            json!("for <@kim@example.com> and <@lee@example.com>"),
        ),
    );
    assert_eq!(unread(&store, &kim()).len(), 1);
    let lee_heard = unread(&store, &lee());
    assert_eq!(lee_heard.len(), 1);
    assert_eq!(lee_heard[0].kind, "set_field");
    let said = comment(&mut store, issue, "ask <@kim@example.com>");
    write(
        &mut store,
        issue,
        Change::EditComment {
            comment: said,
            body: "ask <@kim@example.com> or <@lee@example.com>".into(),
        },
    );
    assert_eq!(unread(&store, &kim()).len(), 2, "the comment, not its edit");
    let lee_heard = unread(&store, &lee());
    assert_eq!(lee_heard.len(), 2);
    assert_eq!(lee_heard[0].kind, "edit_comment");
    let before = dump(&store);
    store.rebuild().unwrap();
    assert_eq!(dump(&store), before);
}

#[test]
fn an_agents_notification_reaches_it_whatever_its_session() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    comment(&mut store, issue, "<@claude for jace>, look");
    assert_eq!(unread(&store, &claude()).len(), 1);
    let other_session =
        claude().in_session(tm_core::actor::SessionId::parse("other-session").unwrap());
    assert_eq!(unread(&store, &other_session).len(), 1);
    assert!(
        unread(&store, &jace()).is_empty(),
        "the human is not the Agent"
    );
}

#[test]
fn a_retracted_comment_is_not_listed() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let said = comment(&mut store, issue, "<@kim@example.com>");
    write(
        &mut store,
        issue,
        Change::Retract {
            comment: said,
            retracted: true,
        },
    );
    assert_eq!(store.notifications(&kim(), true).unwrap().len(), 0);
}

#[test]
fn an_import_notifies_no_one() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    store
        .set_workspace_id(Uuid::from_u128(0x0190_a000_0000_7000_8000_0000_0000_0002))
        .unwrap();
    let line = json!({
        "_type": "issue", "source": "beads:a", "project": "CPL", "type": "task",
        "title": "t", "status": "todo", "reporter": "ana",
        "description": "for <@kim@example.com>",
        "comments": [{"id": "c1", "actor": "ana", "body": "hi <@kim@example.com>",
                      "at": "2026-01-02T00:00:00Z"}],
        "created": "2026-01-01T00:00:00Z", "updated": "2026-01-03T00:00:00Z",
    });
    let mut bytes = serde_json::to_vec(&line).unwrap();
    bytes.push(b'\n');
    store
        .import_records(&mut bytes.as_slice(), &jace())
        .unwrap();
    assert_eq!(store.notifications(&kim(), true).unwrap().len(), 0);
}
