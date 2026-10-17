//! Applies one operation to the projection tables. The same code runs for a
//! new write and for a rebuild, so the two cannot disagree.
//!
//! Operations are applied in canonical order: acknowledged ones by
//! `server_seq`, then pending ones by `hlc`. Sync must re-apply pending
//! operations after the ones it pulls, because `Version::beats` is decided
//! pairwise and is not guaranteed to commute.

use rusqlite::{OptionalExtension, Transaction, params};
use tm_core::{Change, Hlc, Op};
use uuid::Uuid;

use crate::StoreError;

/// What decides a field: the winning operation's position and clock.
struct Version {
    hlc: Hlc,
    replica_id: Uuid,
    op_id: Uuid,
    base_seq: u64,
    server_seq: Option<u64>,
    server_received_ms: Option<u64>,
}

impl Version {
    fn of(op: &Op, server_seq: Option<u64>, server_received_ms: Option<u64>) -> Self {
        Self {
            hlc: op.hlc,
            replica_id: op.replica_id,
            op_id: op.op_id,
            base_seq: op.base_seq,
            server_seq,
            server_received_ms,
        }
    }

    /// ADR 0006: a pending edit shows over acknowledged ones on its own
    /// Replica; between acknowledged operations, one that saw the other wins,
    /// and concurrent ones go to the later capped clock.
    fn beats(&self, current: &Self) -> bool {
        match (self.server_seq, current.server_seq) {
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (Some(seq), Some(current_seq)) => {
                if self.base_seq >= current_seq {
                    true
                } else if current.base_seq >= seq {
                    false
                } else {
                    self.rank() > current.rank()
                }
            }
            (None, None) => self.rank() > current.rank(),
        }
    }

    fn rank(&self) -> (Hlc, Uuid, Uuid) {
        let hlc = self
            .server_received_ms
            .map_or(self.hlc, |ms| self.hlc.capped(ms));
        (hlc, self.replica_id, self.op_id)
    }
}

pub(crate) fn apply(
    tx: &Transaction<'_>,
    op: &Op,
    server_seq: Option<u64>,
    server_received_ms: Option<u64>,
) -> Result<(), StoreError> {
    let change = op.change()?;
    // `base_seq` and `server_seq` are per Project, so an operation filed under
    // another Project would compare positions from different chains.
    if !matches!(change, Change::Create) {
        let expected =
            issue_project(tx, op.issue_id)?.ok_or(StoreError::UnknownIssue(op.issue_id))?;
        if expected != op.project {
            return Err(StoreError::ProjectMismatch {
                issue: op.issue_id,
                expected,
                found: op.project.clone(),
            });
        }
    }
    let version = Version::of(op, server_seq, server_received_ms);
    match change {
        Change::Create => create(tx, op),
        Change::SetField { field, value, .. } => {
            if wins(tx, op.issue_id, &field, "", &version)? {
                record(tx, op.issue_id, &field, "", true, &version)?;
                set_value(tx, op.issue_id, &field, &value)?;
            }
            Ok(())
        }
        Change::Add { field, elem, .. } => {
            set_member(tx, op.issue_id, &field, &elem, true, &version)
        }
        Change::Remove { field, elem, .. } => {
            set_member(tx, op.issue_id, &field, &elem, false, &version)
        }
        Change::Comment { body } => {
            tx.execute(
                "INSERT INTO comments (op_id, issue_id, actor, body, hlc, utc_ms, offset_min)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    op.op_id.as_bytes(),
                    op.issue_id.as_bytes(),
                    op.actor,
                    body,
                    sql_int(op.hlc.raw())?,
                    op.utc_ms,
                    op.offset_min,
                ],
            )?;
            Ok(())
        }
    }
}

fn create(tx: &Transaction<'_>, op: &Op) -> Result<(), StoreError> {
    if issue_project(tx, op.issue_id)?.is_some() {
        return Err(StoreError::DuplicateIssue(op.issue_id));
    }
    tx.execute(
        "INSERT INTO issues (id, project, created_hlc) VALUES (?1, ?2, ?3)",
        params![op.issue_id.as_bytes(), op.project, sql_int(op.hlc.raw())?],
    )?;
    Ok(())
}

fn set_member(
    tx: &Transaction<'_>,
    issue_id: Uuid,
    field: &str,
    elem: &str,
    present: bool,
    version: &Version,
) -> Result<(), StoreError> {
    if !wins(tx, issue_id, field, elem, version)? {
        return Ok(());
    }
    record(tx, issue_id, field, elem, present, version)?;
    if present {
        tx.execute(
            "INSERT OR IGNORE INTO set_members (issue_id, field, elem) VALUES (?1, ?2, ?3)",
            params![issue_id.as_bytes(), field, elem],
        )?;
    } else {
        tx.execute(
            "DELETE FROM set_members WHERE issue_id = ?1 AND field = ?2 AND elem = ?3",
            params![issue_id.as_bytes(), field, elem],
        )?;
    }
    Ok(())
}

fn set_value(
    tx: &Transaction<'_>,
    issue_id: Uuid,
    field: &str,
    value: &serde_json::Value,
) -> Result<(), StoreError> {
    let path = format!("$.{field}");
    if value.is_null() {
        tx.execute(
            "UPDATE issues SET fields = json_remove(fields, ?1) WHERE id = ?2",
            params![path, issue_id.as_bytes()],
        )?;
    } else {
        tx.execute(
            "UPDATE issues SET fields = json_set(fields, ?1, json(?2)) WHERE id = ?3",
            params![path, value.to_string(), issue_id.as_bytes()],
        )?;
    }
    Ok(())
}

fn wins(
    tx: &Transaction<'_>,
    issue_id: Uuid,
    field: &str,
    elem: &str,
    version: &Version,
) -> Result<bool, StoreError> {
    let current = tx
        .query_row(
            "SELECT hlc, replica_id, op_id, base_seq, server_seq, server_received_ms
             FROM field_versions WHERE issue_id = ?1 AND field = ?2 AND elem = ?3",
            params![issue_id.as_bytes(), field, elem],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((hlc, replica_id, op_id, base_seq, server_seq, received)) = current else {
        return Ok(true);
    };
    let current = Version {
        hlc: Hlc::from_raw(rust_int(hlc)?),
        replica_id: uuid_from(&replica_id)?,
        op_id: uuid_from(&op_id)?,
        base_seq: rust_int(base_seq)?,
        server_seq: server_seq.map(rust_int).transpose()?,
        server_received_ms: received.map(rust_int).transpose()?,
    };
    Ok(version.beats(&current))
}

fn record(
    tx: &Transaction<'_>,
    issue_id: Uuid,
    field: &str,
    elem: &str,
    present: bool,
    version: &Version,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT OR REPLACE INTO field_versions
         (issue_id, field, elem, present, op_id, hlc, replica_id, base_seq, server_seq, server_received_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            issue_id.as_bytes(),
            field,
            elem,
            present,
            version.op_id.as_bytes(),
            sql_int(version.hlc.raw())?,
            version.replica_id.as_bytes(),
            sql_int(version.base_seq)?,
            version.server_seq.map(sql_int).transpose()?,
            version.server_received_ms.map(sql_int).transpose()?,
        ],
    )?;
    Ok(())
}

fn issue_project(tx: &Transaction<'_>, issue_id: Uuid) -> Result<Option<String>, StoreError> {
    Ok(tx
        .query_row(
            "SELECT project FROM issues WHERE id = ?1",
            [issue_id.as_bytes()],
            |row| row.get(0),
        )
        .optional()?)
}

pub(crate) fn sql_int(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::Corrupt(format!("{value} does not fit an SQLite integer")))
}

pub(crate) fn rust_int(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value)
        .map_err(|_| StoreError::Corrupt(format!("negative value {value} where none is allowed")))
}

pub(crate) fn uuid_from(bytes: &[u8]) -> Result<Uuid, StoreError> {
    Uuid::from_slice(bytes)
        .map_err(|_| StoreError::Corrupt(format!("{} bytes where a UUID belongs", bytes.len())))
}
