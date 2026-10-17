use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use tm_core::Change;
use tm_core::op::CURRENT_VERSION;
use tm_core::scheme::{Choice, Field, FieldType};
use tm_core::{Hlc, Op, Scheme};
use tm_store::{Clock, Draft, Store, WallTime};
use uuid::Uuid;

/// A temporary directory under Cargo's target dir, removed on drop.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new() -> Self {
        let path =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("tm-store-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub(crate) fn db(&self) -> PathBuf {
        self.0.join("tm.db")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A clock the test advances by hand; shared so a test can move it after the
/// store owns it.
#[derive(Clone)]
pub(crate) struct ManualClock(Arc<AtomicI64>);

impl ManualClock {
    pub(crate) fn at(ms: i64) -> Self {
        Self(Arc::new(AtomicI64::new(ms)))
    }

    pub(crate) fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }

    pub(crate) fn set(&self, ms: i64) {
        self.0.store(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Result<WallTime, String> {
        Ok(WallTime {
            utc_ms: self.0.load(Ordering::SeqCst),
            offset_min: -300,
        })
    }
}

/// An operation as another Replica would have written it.
pub(crate) fn remote_op(
    replica: u128,
    issue_id: Uuid,
    change: &Change,
    hlc: Hlc,
    base_seq: u64,
) -> Op {
    Op {
        v: CURRENT_VERSION,
        op_id: Uuid::now_v7(),
        replica_id: Uuid::from_u128(replica),
        actor: "codex".into(),
        project: "CPL".into(),
        issue_id,
        kind: change.kind().into(),
        hlc,
        base_seq,
        utc_ms: i64::try_from(hlc.ms()).unwrap(),
        offset_min: 0,
        payload: serde_json::to_string(change).unwrap(),
    }
}

/// Inserts `op` as if the server had acknowledged it at `server_seq`, the way
/// sync will; the caller rebuilds to apply it.
pub(crate) fn insert_acked(store: &Store, op: &Op, server_seq: i64, received_ms: i64) {
    let change = op.change().unwrap();
    store
        .connection()
        .execute(
            "INSERT INTO ops (op_id, op_hash, v, kind, replica_id, actor, project, issue_id, field,
                              elem, hlc, base_seq, utc_ms, offset_min, payload, server_seq,
                              server_received_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            rusqlite::params![
                op.op_id.as_bytes(),
                op.op_hash(),
                op.v,
                op.kind,
                op.replica_id.as_bytes(),
                op.actor,
                op.project,
                op.issue_id.as_bytes(),
                change.field(),
                change.elem(),
                i64::try_from(op.hlc.raw()).unwrap(),
                i64::try_from(op.base_seq).unwrap(),
                op.utc_ms,
                op.offset_min,
                op.payload,
                server_seq,
                received_ms,
            ],
        )
        .unwrap();
}

/// The built-in Scheme plus Scheme fields of each kind the tests write.
pub(crate) fn scheme() -> Scheme {
    let mut scheme = Scheme::built_in();
    let area = Field::custom("area", "Area", FieldType::Number, Vec::new());
    let tier = Field::custom(
        "tier",
        "Tier",
        FieldType::SingleSelect,
        vec![
            Choice {
                id: "gold".into(),
                label: "Gold".into(),
            },
            Choice {
                id: "free".into(),
                label: "Free".into(),
            },
        ],
    );
    let tags = Field::custom(
        "tags",
        "Tags",
        FieldType::MultiSelect,
        vec![
            Choice {
                id: "x".into(),
                label: "Ex".into(),
            },
            Choice {
                id: "y".into(),
                label: "Why".into(),
            },
        ],
    );
    for field in [
        area,
        tier,
        tags,
        Field::custom("due", "Due", FieldType::Date, Vec::new()),
        Field::custom("seen", "Seen", FieldType::Datetime, Vec::new()),
        Field::custom("flag", "Flag", FieldType::Boolean, Vec::new()),
    ] {
        scheme.add_field(field).unwrap();
    }
    scheme
}

pub(crate) fn open(path: &Path, clock: &ManualClock) -> Store {
    Store::open_with_clock(path, Box::new(clock.clone()))
        .unwrap()
        .with_scheme(scheme())
}

/// A new Task titled "t" in Project CPL.
pub(crate) fn new_issue(store: &mut Store) -> Uuid {
    store.create_issue("jace", "CPL", "task", "t").unwrap().0
}

/// One field of an Issue as it reads now.
pub(crate) fn field(store: &Store, issue: Uuid, name: &str) -> Option<serde_json::Value> {
    store
        .issue(issue)
        .unwrap()
        .unwrap()
        .fields
        .get(name)
        .cloned()
}

pub(crate) fn draft(issue_id: Uuid, change: Change) -> Draft {
    Draft {
        actor: "jace".into(),
        project: "CPL".into(),
        issue_id,
        change,
    }
}

pub(crate) fn set_field(field: &str, value: serde_json::Value) -> Change {
    Change::SetField {
        field: field.into(),
        label: field.to_uppercase(),
        value,
        value_label: None,
    }
}

/// Every projection row, sorted, as text: equal dumps mean equal projections.
pub(crate) fn dump(store: &Store) -> Vec<String> {
    let conn = store.connection();
    let mut out = Vec::new();
    for table in [
        "issues",
        "field_values",
        "field_versions",
        "set_members",
        "comments",
        "keys",
        "claims",
    ] {
        let mut stmt = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let columns = stmt.column_count();
        let mut rows: Vec<String> = stmt
            .query_map([], |row| {
                let mut cells = Vec::with_capacity(columns);
                for i in 0..columns {
                    let value: rusqlite::types::Value = row.get(i)?;
                    cells.push(format!("{value:?}"));
                }
                Ok(format!("{table}|{}", cells.join("|")))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows.sort();
        out.extend(rows);
    }
    out
}
