//! The operation log's row format: how an [`Op`] is written to `ops` and read
//! back, and how a log of an older shape is upgraded in place.

use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use tm_core::{Change, Hlc, Op};

use crate::StoreError;
use crate::apply::{project_from, rust_int, sql_int, uuid_from};
use crate::schema;

/// The `ops` columns in order; a store whose log has others is refused, since
/// `CREATE TABLE IF NOT EXISTS` never adds a column.
pub(crate) const COLUMNS: [&str; 22] = [
    "seq",
    "op_id",
    "op_hash",
    "v",
    "kind",
    "replica_id",
    "actor",
    "on_behalf_of",
    "source",
    "project",
    "issue_id",
    "field",
    "elem",
    "hlc",
    "base_seq",
    "utc_ms",
    "offset_min",
    "payload",
    "server_seq",
    "server_received_ms",
    "assigned_key",
    "session",
];

/// The `ops` table's one older shape, without `session`, which opening a
/// store upgrades in place by adding it.
const COLUMNS_BEFORE_SESSION: usize = 21;

/// Adds `session` to a log from before sessions were recorded; the
/// immutability trigger is dropped so [`schema::LOG`] recreates it covering
/// `session`.
const ADD_SESSION: &str = "
ALTER TABLE ops ADD COLUMN session TEXT;
DROP TRIGGER IF EXISTS ops_immutable;
";

/// The `ops` columns in order; none before the log is created.
pub(crate) fn columns(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare("SELECT name FROM pragma_table_info('ops') ORDER BY cid")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns)
}

/// Refuses a log another build wrote with other columns: until v1 the format
/// changes in place, and such a store is recreated (ADR 0010). A log from
/// before sessions were recorded gains its `session` column.
pub(crate) fn upgrade(conn: &mut Connection) -> Result<(), StoreError> {
    let found = columns(conn)?;
    if found.is_empty() || found == COLUMNS {
        return Ok(());
    }
    if found[..] != COLUMNS[..COLUMNS_BEFORE_SESSION] {
        return Err(StoreError::FormatChanged);
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Another process may have upgraded it while this one waited.
    if columns(&tx)?.len() == COLUMNS_BEFORE_SESSION {
        tx.execute_batch(ADD_SESSION)?;
        tx.execute_batch(schema::LOG)?;
    }
    tx.commit()?;
    Ok(())
}

/// Writes `op` to the log.
pub(crate) fn insert(
    tx: &Transaction<'_>,
    op: &Op,
    change: &Change,
    assigned_key: Option<&str>,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO ops (op_id, op_hash, v, kind, replica_id, actor, on_behalf_of, source,
                          project, issue_id, field, elem, hlc, base_seq, utc_ms, offset_min,
                          payload, assigned_key, session)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                 ?18, ?19)",
        params![
            op.op_id.as_bytes(),
            op.op_hash(),
            op.v,
            op.kind,
            op.replica_id.as_bytes(),
            op.actor,
            op.on_behalf_of,
            op.source,
            op.project.to_string(),
            op.subject.as_bytes(),
            change.field(),
            change.elem(),
            sql_int(op.hlc.raw())?,
            sql_int(op.base_seq)?,
            op.utc_ms,
            op.offset_min,
            op.payload,
            assigned_key,
            op.session,
        ],
    )?;
    Ok(())
}

/// Every logged operation, in the order replay applies them.
pub(crate) fn in_replay_order(tx: &Transaction<'_>) -> Result<Vec<StoredOp>, StoreError> {
    let mut stmt = tx.prepare(
        "SELECT seq, op_id, op_hash, v, replica_id, actor, project, issue_id, kind, hlc,
                base_seq, utc_ms, offset_min, payload, server_seq, server_received_ms,
                assigned_key, on_behalf_of, source, session
         FROM ops
         ORDER BY server_seq IS NULL, server_seq, hlc, replica_id, op_id",
    )?;
    let rows = stmt.query_map([], StoredOp::read)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// One row of the log as SQLite returns it, before conversion can fail.
pub(crate) struct StoredOp {
    pub(crate) seq: i64,
    pub(crate) op_id: Vec<u8>,
    pub(crate) op_hash: Vec<u8>,
    v: i64,
    replica_id: Vec<u8>,
    actor: String,
    project: String,
    issue_id: Vec<u8>,
    kind: String,
    hlc: i64,
    base_seq: i64,
    utc_ms: i64,
    offset_min: i64,
    payload: String,
    server_seq: Option<i64>,
    server_received_ms: Option<i64>,
    pub(crate) assigned_key: Option<String>,
    on_behalf_of: Option<String>,
    source: Option<String>,
    session: Option<String>,
}

impl StoredOp {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            seq: row.get(0)?,
            op_id: row.get(1)?,
            op_hash: row.get(2)?,
            v: row.get(3)?,
            replica_id: row.get(4)?,
            actor: row.get(5)?,
            project: row.get(6)?,
            issue_id: row.get(7)?,
            kind: row.get(8)?,
            hlc: row.get(9)?,
            base_seq: row.get(10)?,
            utc_ms: row.get(11)?,
            offset_min: row.get(12)?,
            payload: row.get(13)?,
            server_seq: row.get(14)?,
            server_received_ms: row.get(15)?,
            assigned_key: row.get(16)?,
            on_behalf_of: row.get(17)?,
            source: row.get(18)?,
            session: row.get(19)?,
        })
    }

    /// The operation, its server position and when the server received it.
    pub(crate) fn decode(&self) -> Result<(Op, Option<u64>, Option<u64>), StoreError> {
        let narrow =
            |name: &str, value: i64| StoreError::Corrupt(format!("{name} {value} is out of range"));
        let op = Op {
            v: u16::try_from(self.v).map_err(|_| narrow("version", self.v))?,
            op_id: uuid_from(&self.op_id)?,
            replica_id: uuid_from(&self.replica_id)?,
            actor: self.actor.clone(),
            on_behalf_of: self.on_behalf_of.clone(),
            source: self.source.clone(),
            project: project_from(&self.project)?,
            subject: uuid_from(&self.issue_id)?,
            kind: self.kind.clone(),
            hlc: Hlc::from_raw(rust_int(self.hlc)?),
            base_seq: rust_int(self.base_seq)?,
            utc_ms: self.utc_ms,
            offset_min: i16::try_from(self.offset_min)
                .map_err(|_| narrow("UTC offset", self.offset_min))?,
            payload: self.payload.clone(),
            session: self.session.clone(),
        };
        let server_seq = self.server_seq.map(rust_int).transpose()?;
        let received = self.server_received_ms.map(rust_int).transpose()?;
        Ok((op, server_seq, received))
    }
}
