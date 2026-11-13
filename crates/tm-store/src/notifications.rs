//! Notifications (ADR 0019): derived from the operations that carry
//! mentions, with read marks a record of their own.

use rusqlite::{OptionalExtension, Transaction, params};
use tm_core::mention::Mentioned;
use tm_core::{Actor, Change, Hlc, Op, notification};
use uuid::Uuid;

use crate::apply::{Version, rust_int, sql_int, uuid_from};
use crate::{Draft, Store, StoreError};

/// One Notification as its recipient reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: Uuid,
    /// The operation that mentions the recipient.
    pub op_id: Uuid,
    pub issue_id: Uuid,
    /// The Change kind of that operation, such as `comment`.
    pub kind: String,
    /// The text that mentions the recipient, as written.
    pub text: String,
    pub actor: String,
    pub on_behalf_of: Option<String>,
    pub utc_ms: i64,
    pub read: bool,
}

/// What the operation `op` wrote that may mention someone, with the text it
/// replaces and the comment it belongs to.
pub(crate) struct Mentioning<'a> {
    pub(crate) text: &'a str,
    pub(crate) before: Option<String>,
    pub(crate) comment: Option<Uuid>,
}

/// Records a Notification for each Actor `mentioning` adds a mention of,
/// apart from the writer. An imported operation replays what was already
/// said, so it notifies no one.
pub(crate) fn derive(
    tx: &Transaction<'_>,
    op: &Op,
    mentioning: &Mentioning<'_>,
) -> Result<(), StoreError> {
    if op.source.is_some() {
        return Ok(());
    }
    let writer = notification::writer(&op.actor, op.on_behalf_of.as_deref());
    let recipients =
        notification::recipients(mentioning.text, mentioning.before.as_deref(), &writer);
    for recipient in recipients {
        tx.execute(
            "INSERT OR IGNORE INTO notifications
             (id, recipient, op_id, issue_id, comment, kind, text, actor, on_behalf_of,
              hlc, utc_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                notification::id(op.op_id, &recipient).as_bytes(),
                recipient.token(),
                op.op_id.as_bytes(),
                op.subject.as_bytes(),
                mentioning.comment.map(|comment| *comment.as_bytes()),
                op.kind,
                mentioning.text,
                op.actor,
                op.on_behalf_of,
                sql_int(op.hlc.raw())?,
                op.utc_ms,
            ],
        )?;
    }
    Ok(())
}

/// Applies a read mark: the latest by clock wins, as a reader's marks may be
/// filed under different Projects whose server positions do not compare.
pub(crate) fn apply(
    tx: &Transaction<'_>,
    op: &Op,
    change: &Change,
    version: &Version,
) -> Result<(), StoreError> {
    let Change::SetRead { read, .. } = change else {
        unreachable!("only a read mark reaches here")
    };
    if !crate::project::exists(tx, op.project)? {
        return Err(StoreError::UnknownProject(op.project));
    }
    let rank = version.rank();
    let current = tx
        .query_row(
            "SELECT hlc, replica_id, op_id FROM notification_reads WHERE id = ?1",
            [op.subject.as_bytes()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .optional()?;
    if let Some((hlc, replica, op_id)) = current {
        let current = (
            Hlc::from_raw(rust_int(hlc)?),
            uuid_from(&replica)?,
            uuid_from(&op_id)?,
        );
        if current >= rank {
            return Ok(());
        }
    }
    let (hlc, replica, op_id) = rank;
    tx.execute(
        "INSERT INTO notification_reads (id, read, hlc, replica_id, op_id)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (id) DO UPDATE SET read = excluded.read, hlc = excluded.hlc,
           replica_id = excluded.replica_id, op_id = excluded.op_id",
        params![
            op.subject.as_bytes(),
            read,
            sql_int(hlc.raw())?,
            replica.as_bytes(),
            op_id.as_bytes()
        ],
    )?;
    Ok(())
}

/// Where a read mark is written, only on a Notification `actor` received.
pub(crate) fn check(
    tx: &Transaction<'_>,
    actor: &Actor,
    subject: Uuid,
    change: &Change,
) -> Result<(), StoreError> {
    let Change::SetRead { op, .. } = change else {
        unreachable!("only a read mark reaches here")
    };
    let reader = reader(actor);
    let received: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM notifications WHERE id = ?1 AND recipient = ?2)",
        params![subject.as_bytes(), reader.token()],
        |row| row.get(0),
    )?;
    if subject != notification::id(*op, &reader) || !received {
        return Err(StoreError::Invalid {
            field: notification::READ_FIELD.into(),
            reason: "is marked only by the Actor it notifies".into(),
            valid: Vec::new(),
        });
    }
    Ok(())
}

/// The Actor `actor` is as a mention names it.
fn reader(actor: &Actor) -> Mentioned {
    notification::writer(actor.name(), actor.on_behalf_of())
}

const COLUMNS: &str = "n.id, n.op_id, n.issue_id, n.kind, n.text, n.actor, n.on_behalf_of,
     n.utc_ms, coalesce(r.read, 0)";

fn notification(row: &rusqlite::Row<'_>) -> Result<Notification, StoreError> {
    Ok(Notification {
        id: uuid_from(&row.get::<_, Vec<u8>>(0)?)?,
        op_id: uuid_from(&row.get::<_, Vec<u8>>(1)?)?,
        issue_id: uuid_from(&row.get::<_, Vec<u8>>(2)?)?,
        kind: row.get(3)?,
        text: row.get(4)?,
        actor: row.get(5)?,
        on_behalf_of: row.get(6)?,
        utc_ms: row.get(7)?,
        read: row.get(8)?,
    })
}

impl Store {
    /// `actor`'s Notifications, newest first: the unread ones, or every one
    /// with `all`. One whose comment was retracted is not listed.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn notifications(&self, actor: &Actor, all: bool) -> Result<Vec<Notification>, StoreError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM notifications n
             LEFT JOIN notification_reads r ON r.id = n.id
             LEFT JOIN comments c ON c.op_id = n.comment
             WHERE n.recipient = ?1 AND (?2 OR NOT coalesce(r.read, 0))
               AND NOT coalesce(c.retracted, 0)
             ORDER BY n.hlc DESC, n.id"
        ))?;
        let mut rows = stmt.query(params![reader(actor).token(), all])?;
        let mut listed = Vec::new();
        while let Some(row) = rows.next()? {
            listed.push(notification(row)?);
        }
        Ok(listed)
    }

    /// `actor`'s Notification `id`, read or not.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn notification(
        &self,
        actor: &Actor,
        id: Uuid,
    ) -> Result<Option<Notification>, StoreError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM notifications n
             LEFT JOIN notification_reads r ON r.id = n.id
             WHERE n.id = ?1 AND n.recipient = ?2"
        ))?;
        let mut rows = stmt.query(params![id.as_bytes(), reader(actor).token()])?;
        rows.next()?.map(notification).transpose()
    }

    /// Marks each of `notifications` that is unread read in one batch, filed
    /// under its Issue's Project; none are written when all are read, since a
    /// mark that changes nothing would leave the earlier one past undoing.
    ///
    /// # Errors
    ///
    /// [`StoreError::Invalid`] for a Notification `actor` did not receive,
    /// or as [`Store::append_all`].
    pub fn mark_read(
        &mut self,
        actor: &Actor,
        notifications: &[Notification],
    ) -> Result<Vec<Op>, StoreError> {
        let mut drafts = Vec::with_capacity(notifications.len());
        for notified in notifications.iter().filter(|notified| !notified.read) {
            let project = crate::apply::issue_project(&self.conn, notified.issue_id)?
                .ok_or(StoreError::UnknownIssue(notified.issue_id))?;
            drafts.push(Draft {
                actor: actor.clone(),
                project,
                subject: notified.id,
                change: Change::SetRead {
                    op: notified.op_id,
                    read: true,
                },
            });
        }
        if drafts.is_empty() {
            return Ok(Vec::new());
        }
        self.append_all(drafts)
    }
}
