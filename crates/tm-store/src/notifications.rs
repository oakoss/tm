//! Notifications (ADR 0019): derived from the operations that carry
//! mentions, with read marks a record of their own.

use rusqlite::{OptionalExtension, Transaction, params};
use tm_core::mention::Mentioned;
use tm_core::notification::{self, Site};
use tm_core::{Actor, Change, Hlc, Op};
use uuid::Uuid;

use crate::apply::{Version, rust_int, sql_int, uuid_from};
use crate::{Draft, Store, StoreError};

/// One Notification as its recipient reads it, built only by the store, so
/// its `site` is always its `kind`'s.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Notification {
    pub id: Uuid,
    /// The operation that mentions the recipient.
    pub op_id: Uuid,
    pub issue_id: Uuid,
    /// The Change kind of that operation, such as `comment`.
    pub kind: String,
    pub site: Site,
    /// The text that mentions the recipient, as written.
    pub text: String,
    pub actor: String,
    pub on_behalf_of: Option<String>,
    pub utc_ms: i64,
    pub read: bool,
}

/// Notifies whom the comment, question, answer or withdrawal `op` writes as
/// `body` mentions, when `site`, its Change's, says it notifies.
pub(crate) fn comment(
    tx: &Transaction<'_>,
    op: &Op,
    site: Option<Site>,
    body: &str,
) -> Result<(), StoreError> {
    let Some(site) = site else { return Ok(()) };
    derive(tx, op, site, (body, None), Some(op.op_id))
}

/// Notifies whom an edit of `comment` to `body` adds a mention of, against
/// the text it replaces, when `site` says it notifies.
pub(crate) fn edit(
    tx: &Transaction<'_>,
    op: &Op,
    site: Option<Site>,
    (comment, body): (Uuid, &str),
) -> Result<(), StoreError> {
    let Some(site) = site else { return Ok(()) };
    let before = replaced_text(tx, comment)?;
    derive(tx, op, site, (body, before.as_deref()), Some(comment))
}

/// Notifies whom a field's `text` adds a mention of, against the text it
/// replaces, when `site` says the field notifies, as only the description
/// does; run before the field is written.
pub(crate) fn description(
    tx: &Transaction<'_>,
    op: &Op,
    site: Option<Site>,
    text: &str,
) -> Result<(), StoreError> {
    let Some(site) = site else { return Ok(()) };
    let before: Option<String> = tx.query_row(
        "SELECT description FROM issues WHERE id = ?1",
        [op.subject.as_bytes()],
        |row| row.get(0),
    )?;
    derive(tx, op, site, (text, before.as_deref()), None)
}

/// Records a Notification for each Actor `text` adds a mention of against
/// `before`, apart from the writer. An imported operation replays what was
/// already said, so it notifies no one.
fn derive(
    tx: &Transaction<'_>,
    op: &Op,
    site: Site,
    (text, before): (&str, Option<&str>),
    comment: Option<Uuid>,
) -> Result<(), StoreError> {
    debug_assert_eq!(Site::of_kind(&op.kind), Some(site), "{}", op.kind);
    if op.source.is_some() {
        return Ok(());
    }
    let writer = notification::writer(&op.actor, op.on_behalf_of.as_deref());
    for recipient in notification::recipients(text, before, &writer) {
        notify(tx, op, &recipient, text, comment)?;
    }
    Ok(())
}

/// The text an edit of `comment` replaces: the comment's text now, or as
/// written when the edit replays before it.
fn replaced_text(tx: &Transaction<'_>, comment: Uuid) -> Result<Option<String>, StoreError> {
    let shown: Option<String> = tx
        .query_row(
            "SELECT body FROM comments WHERE op_id = ?1",
            [comment.as_bytes()],
            |row| row.get(0),
        )
        .optional()?;
    if shown.is_some() {
        return Ok(shown);
    }
    let payload: Option<String> = tx
        .query_row(
            "SELECT payload FROM ops WHERE op_id = ?1",
            [comment.as_bytes()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(
        payload.and_then(|payload| match serde_json::from_str(&payload) {
            Ok(Change::Comment { body }) => Some(body),
            _ => None,
        }),
    )
}

/// Notifies a Claim's holder that `op`'s batch wrote past it; the text
/// names the fields written.
pub(crate) fn passed(tx: &Transaction<'_>, op: &Op, passed: &Change) -> Result<(), StoreError> {
    if passed.site().is_none() {
        return Ok(());
    }
    let Change::PassedClaim {
        holder,
        held_for,
        fields,
        ..
    } = passed
    else {
        unreachable!("only a passed Claim reaches here")
    };
    let recipient = notification::holder(holder, held_for);
    notify(tx, op, &recipient, &fields.join(", "), None)
}

fn notify(
    tx: &Transaction<'_>,
    op: &Op,
    recipient: &Mentioned,
    text: &str,
    comment: Option<Uuid>,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT OR IGNORE INTO notifications
         (id, recipient, op_id, issue_id, comment, kind, text, actor, on_behalf_of,
          hlc, utc_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            notification::id(op.op_id, recipient).as_bytes(),
            recipient.token(),
            op.op_id.as_bytes(),
            op.subject.as_bytes(),
            comment.map(|comment| *comment.as_bytes()),
            op.kind,
            text,
            op.actor,
            op.on_behalf_of,
            sql_int(op.hlc.raw())?,
            op.utc_ms,
        ],
    )?;
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
    let kind: String = row.get(3)?;
    let site = Site::of_kind(&kind)
        .ok_or_else(|| StoreError::Corrupt(format!("a Notification from a {kind:?} operation")))?;
    Ok(Notification {
        id: uuid_from(&row.get::<_, Vec<u8>>(0)?)?,
        op_id: uuid_from(&row.get::<_, Vec<u8>>(1)?)?,
        issue_id: uuid_from(&row.get::<_, Vec<u8>>(2)?)?,
        kind,
        site,
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
