//! An Issue's history: every operation that wrote it, in the order replay
//! applies them.

use std::collections::HashMap;

use tm_core::Change;
use tm_core::op::CLAIM_FIELD;
use uuid::Uuid;

use rusqlite::OptionalExtension;

use crate::apply::{Version, label_key, uuid_from};
use crate::{Store, StoreError};

/// One operation in an Issue's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub op_id: Uuid,
    /// The write it was part of; none for one this Replica did not write.
    pub batch: Option<Uuid>,
    /// The batch this one's batch undid, when it is an undo.
    pub undid: Option<Uuid>,
    /// The undo that stands against this one's batch, as `tm undo` decides it.
    pub undone_by: Option<Uuid>,
    pub actor: String,
    pub on_behalf_of: Option<String>,
    /// The record an import wrote it from.
    pub source: Option<String>,
    /// The Agent's harness session that wrote it.
    pub session: Option<String>,
    pub utc_ms: i64,
    /// The UTC offset where it was written, in minutes.
    pub offset_min: i64,
    /// The Issue it is filed under: this one, or the other end of a Link.
    pub subject: Uuid,
    pub change: Change,
    /// Why replay could not apply it in full, when it could not.
    pub unapplied: Option<String>,
    /// Whether a concurrent change to the same field or element won the
    /// merge (ADR 0006), so this one changed nothing.
    pub lost: bool,
}

impl Store {
    /// Every operation that wrote `issue`, Links filed under their other end
    /// included, in replay order. Each field's merge is replayed alongside,
    /// so an operation that lost to a concurrent one says so.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn history(&self, issue: Uuid) -> Result<Vec<HistoryEntry>, StoreError> {
        self.read(|store| {
            let mut stmt = store.conn.prepare(
                "SELECT o.op_id, b.batch,
                        (SELECT undid FROM undo_batches WHERE batch = b.batch),
                        o.offset_min,
                        o.actor, o.on_behalf_of, o.source, o.utc_ms, o.issue_id, o.payload,
                        (SELECT reason FROM unapplied WHERE op_id = o.op_id LIMIT 1),
                        o.field, o.elem,
                        o.hlc, o.replica_id, o.op_id, o.base_seq, o.server_seq,
                        o.server_received_ms, o.session
                 FROM ops o LEFT JOIN op_batches b ON b.op_id = o.op_id
                 WHERE (o.issue_id = ?1 OR (o.field = ?2 AND o.elem LIKE ?3))
                   AND o.kind NOT IN ('create_project', 'rename_project',
                                      'create_shortlist', 'set_shortlist', 'shortlist_actor',
                                      'shortlist_source')
                 ORDER BY o.server_seq IS NULL, o.server_seq, o.hlc, o.replica_id, o.op_id",
            )?;
            // A Link's element ends in the id of the Issue it points at.
            let other_end = format!("%/{issue}");
            let params = rusqlite::params![issue.as_bytes(), tm_core::link::FIELD, other_end];
            let mut rows = stmt.query(params)?;
            let standing = crate::undo::standing_undos(&store.conn)?;
            let mut history = Vec::new();
            let mut winners: HashMap<(Uuid, String, String), Version> = HashMap::new();
            let id = |bytes: Option<Vec<u8>>| bytes.as_deref().map(uuid_from).transpose();
            while let Some(row) = rows.next()? {
                let payload: String = row.get(9)?;
                let subject = uuid_from(&row.get::<_, Vec<u8>>(8)?)?;
                let field: Option<String> = row.get(11)?;
                // Replay keys labels without case, as it compares them.
                let elem = field.as_deref().map(|field| {
                    let elem = row.get::<_, Option<String>>(12)?.unwrap_or_default();
                    Ok::<_, StoreError>(label_key(field, &elem, None).0)
                });
                let elem = elem.transpose()?.unwrap_or_default();
                let op_id = uuid_from(&row.get::<_, Vec<u8>>(0)?)?;
                let mut unapplied: Option<String> = row.get(10)?;
                // A field's current winner applied, though a Link it added
                // may point at an Issue that does not exist.
                if unapplied.is_some()
                    && let Some(field) = &field
                    && winner(&store.conn, (subject, field, &elem))? == Some(op_id)
                {
                    unapplied = None;
                }
                // Claims apply by compare-and-set, not by merge (ADR 0009).
                let merged = field.filter(|field| field != CLAIM_FIELD);
                let lost = match merged {
                    Some(field) if unapplied.is_none() => {
                        let version = Version::from_row(row, 13)?;
                        let key = (subject, field, elem);
                        let lost = winners
                            .get(&key)
                            .is_some_and(|current| !version.beats(current));
                        if !lost {
                            winners.insert(key, version);
                        }
                        lost
                    }
                    _ => false,
                };
                let batch = id(row.get(1)?)?;
                history.push(HistoryEntry {
                    op_id,
                    batch,
                    undid: id(row.get(2)?)?,
                    undone_by: batch.and_then(|batch| standing.get(&batch).map(|(undo, _)| *undo)),
                    actor: row.get(4)?,
                    on_behalf_of: row.get(5)?,
                    source: row.get(6)?,
                    session: row.get(19)?,
                    utc_ms: row.get(7)?,
                    offset_min: row.get(3)?,
                    subject,
                    change: serde_json::from_str(&payload)
                        .map_err(|err| StoreError::Corrupt(format!("a logged payload: {err}")))?,
                    unapplied,
                    lost,
                });
            }
            Ok(history)
        })
    }
}

/// The operation that currently wins `(issue, field, elem)`.
fn winner(
    conn: &rusqlite::Connection,
    (issue, field, elem): (Uuid, &str, &str),
) -> Result<Option<Uuid>, StoreError> {
    let op_id: Option<Vec<u8>> = conn
        .query_row(
            "SELECT op_id FROM field_versions WHERE issue_id = ?1 AND field = ?2 AND elem = ?3",
            rusqlite::params![issue.as_bytes(), field, elem],
            |row| row.get(0),
        )
        .optional()?;
    op_id.as_deref().map(uuid_from).transpose()
}
