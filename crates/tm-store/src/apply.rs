//! Applies one operation to the projection tables. The same code runs for a
//! new write and for a rebuild, so the two cannot disagree.
//!
//! Operations are applied in canonical order: acknowledged ones by
//! `server_seq`, then pending ones by `hlc`. Sync must re-apply pending
//! operations after the ones it pulls, because `Version::beats` is decided
//! pairwise and is not guaranteed to commute.

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value as Json;
use tm_core::op::{COMMENTS_FIELD, ClaimAction};
use tm_core::scheme::priority_rank;
use tm_core::{Change, Hlc, Op, ProjectId, link};
use uuid::Uuid;

use crate::{StoreError, project};

/// What decides a field: the winning operation's position and clock.
pub(crate) struct Version {
    hlc: Hlc,
    replica_id: Uuid,
    op_id: Uuid,
    base_seq: u64,
    server_seq: Option<u64>,
    server_received_ms: Option<u64>,
}

impl Version {
    /// A logged operation's version from its `ops` row: `hlc`, `replica_id`,
    /// `op_id`, `base_seq`, `server_seq` and `server_received_ms`, in that
    /// order from `first`.
    pub(crate) fn from_row(row: &rusqlite::Row<'_>, first: usize) -> Result<Self, StoreError> {
        Ok(Self {
            hlc: Hlc::from_raw(rust_int(row.get(first)?)?),
            replica_id: uuid_from(&row.get::<_, Vec<u8>>(first + 1)?)?,
            op_id: uuid_from(&row.get::<_, Vec<u8>>(first + 2)?)?,
            base_seq: rust_int(row.get(first + 3)?)?,
            server_seq: row
                .get::<_, Option<i64>>(first + 4)?
                .map(rust_int)
                .transpose()?,
            server_received_ms: row
                .get::<_, Option<i64>>(first + 5)?
                .map(rust_int)
                .transpose()?,
        })
    }

    pub(crate) const fn op_id(&self) -> Uuid {
        self.op_id
    }

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
    pub(crate) fn beats(&self, current: &Self) -> bool {
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

/// Where the operation sits: its server position, if acknowledged, and the
/// Key its `create` was assigned, if any.
#[derive(Clone, Copy, Default)]
pub(crate) struct Effects<'a> {
    pub(crate) server_seq: Option<u64>,
    pub(crate) server_received_ms: Option<u64>,
    pub(crate) assigned_key: Option<&'a str>,
}

/// Applies `op`. `Ok(Some(reason))` means it was kept but did not take full
/// effect, such as a create whose Key could not be recorded; replay records
/// the reason as written rather than dropping the operation.
pub(crate) fn apply(
    tx: &Transaction<'_>,
    op: &Op,
    effects: Effects<'_>,
) -> Result<Option<String>, StoreError> {
    let Effects {
        server_seq,
        server_received_ms,
        assigned_key,
    } = effects;
    let change = op.change()?;
    if change.is_project() {
        return project::apply(tx, op, change).map(|()| None);
    }
    // `base_seq` and `server_seq` are per Project, so an operation filed under
    // another Project would compare positions from different chains.
    if matches!(change, Change::Create) {
        if !project::exists(tx, op.project)? {
            return Err(StoreError::UnknownProject(op.project));
        }
    } else {
        let expected =
            issue_project(tx, op.subject)?.ok_or(StoreError::UnknownIssue(op.subject))?;
        if expected != op.project {
            return Err(StoreError::ProjectMismatch {
                issue: op.subject,
                expected,
                found: op.project,
            });
        }
    }
    let version = Version::of(op, server_seq, server_received_ms);
    match change {
        Change::Create => return create(tx, op, assigned_key),
        Change::CreateProject { .. } | Change::RenameProject { .. } => {
            unreachable!("Project operations return above")
        }
        Change::SetField { field, value, .. } => {
            // Checked before anything is written, so a value that does not fit
            // leaves the projection untouched.
            let target = target(&field, &value)?;
            if wins(tx, op.subject, &field, "", &version)? {
                record(tx, op.subject, &field, "", true, &version)?;
                write_target(tx, op.subject, &field, target)?;
            }
        }
        Change::Add {
            field,
            elem,
            elem_label,
            ..
        } => {
            let (elem, display) = label_key(&field, &elem, elem_label);
            set_member(
                tx,
                op.subject,
                &field,
                &elem,
                true,
                display.as_deref(),
                &version,
            )?;
        }
        Change::Remove { field, elem, .. } => {
            let (elem, _) = label_key(&field, &elem, None);
            set_member(tx, op.subject, &field, &elem, false, None, &version)?;
        }
        Change::Link {
            link_type,
            target,
            present,
        } => link(tx, op.subject, &link_type, target, present, &version)?,
        // A lost take or release changes nothing, so it is not activity
        // either; nor is a renewal, which a write to another Issue carries.
        Change::Claim { action } => {
            if let Some(lost) = crate::claim::apply(tx, op, &action)? {
                return Ok(Some(lost));
            }
            if matches!(action, ClaimAction::Renew { .. }) {
                return Ok(None);
            }
        }
        Change::Retract { comment, retracted } => {
            retract(tx, op.subject, comment, retracted, &version)?;
        }
        Change::Comment { body } => {
            tx.execute(
                "INSERT INTO comments
                 (op_id, issue_id, actor, body, hlc, utc_ms, offset_min, retracted)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, coalesce(
                   (SELECT present FROM field_versions
                    WHERE issue_id = ?2 AND field = ?8 AND elem = ?9), 0))",
                params![
                    op.op_id.as_bytes(),
                    op.subject.as_bytes(),
                    op.actor,
                    body,
                    sql_int(op.hlc.raw())?,
                    op.utc_ms,
                    op.offset_min,
                    COMMENTS_FIELD,
                    op.op_id.to_string(),
                ],
            )?;
        }
    }
    // Every operation counts as activity, comments included.
    tx.execute(
        "UPDATE issues SET updated_ms = max(updated_ms, ?1) WHERE id = ?2",
        params![op.utc_ms, op.subject.as_bytes()],
    )?;
    Ok(None)
}

/// Hides or shows comment `comment` if this retraction wins. The state is
/// recorded even before the comment replays, which takes it on arrival.
fn retract(
    tx: &Transaction<'_>,
    issue_id: Uuid,
    comment: Uuid,
    retracted: bool,
    version: &Version,
) -> Result<(), StoreError> {
    let elem = comment.to_string();
    if !wins(tx, issue_id, COMMENTS_FIELD, &elem, version)? {
        return Ok(());
    }
    record(tx, issue_id, COMMENTS_FIELD, &elem, retracted, version)?;
    tx.execute(
        "UPDATE comments SET retracted = ?1 WHERE op_id = ?2 AND issue_id = ?3",
        params![retracted, comment.as_bytes(), issue_id.as_bytes()],
    )?;
    Ok(())
}

/// Labels compare without case. Writes already store them lowercased; this
/// applies the same rule to an operation logged before that rule, keeping the
/// spelling it was written with as its display form.
pub(crate) fn label_key(
    field: &str,
    elem: &str,
    display: Option<String>,
) -> (String, Option<String>) {
    if field != "labels" {
        return (elem.to_owned(), display);
    }
    let key = elem.trim().to_lowercase();
    let display = display.or_else(|| (key != elem).then(|| elem.trim().to_owned()));
    (key, display)
}

fn create(
    tx: &Transaction<'_>,
    op: &Op,
    assigned_key: Option<&str>,
) -> Result<Option<String>, StoreError> {
    if issue_project(tx, op.subject)?.is_some() {
        return Err(StoreError::DuplicateIssue(op.subject));
    }
    tx.execute(
        "INSERT INTO issues (id, project, created_hlc, reporter, created_ms, updated_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![
            op.subject.as_bytes(),
            op.project.to_string(),
            sql_int(op.hlc.raw())?,
            op.actor,
            op.utc_ms,
        ],
    )?;
    match assigned_key {
        Some(key) => Ok(project::record_key(tx, key, op.subject, op.project)?
            .map(|reason| format!("applied without its Key: {reason}"))),
        None => Ok(None),
    }
}

/// `display` is how the element shows when it differs from `elem`.
fn set_member(
    tx: &Transaction<'_>,
    issue_id: Uuid,
    field: &str,
    elem: &str,
    present: bool,
    display: Option<&str>,
    version: &Version,
) -> Result<(), StoreError> {
    if !wins(tx, issue_id, field, elem, version)? {
        return Ok(());
    }
    record(tx, issue_id, field, elem, present, version)?;
    if present {
        tx.execute(
            "INSERT OR REPLACE INTO set_members (issue_id, field, elem, display)
             VALUES (?1, ?2, ?3, ?4)",
            params![issue_id.as_bytes(), field, elem, display],
        )?;
    } else {
        tx.execute(
            "DELETE FROM set_members WHERE issue_id = ?1 AND field = ?2 AND elem = ?3",
            params![issue_id.as_bytes(), field, elem],
        )?;
    }
    Ok(())
}

/// Neither cycles nor the target are checked: a merge can make a cycle (ADR
/// 0009), and a target in another Project's chain can replay after the Link.
fn link(
    tx: &Transaction<'_>,
    source: Uuid,
    link_type: &str,
    target: Uuid,
    present: bool,
    version: &Version,
) -> Result<(), StoreError> {
    let invalid = |reason: &str| StoreError::Invalid {
        field: "link".into(),
        reason: reason.to_owned(),
        valid: Vec::new(),
    };
    if target == source {
        return Err(invalid("cannot join an Issue to itself"));
    }
    // Only a built-in type is known to be symmetric; replay never reads the Scheme.
    if link::link_type(link_type).is_some_and(|known| known.symmetric) && target < source {
        return Err(invalid("is not filed under its canonical source"));
    }
    let elem = link::elem(link_type, target);
    if !wins(tx, source, link::FIELD, &elem, version)? {
        return Ok(());
    }
    record(tx, source, link::FIELD, &elem, present, version)?;
    if present {
        tx.execute(
            "INSERT INTO links (type, src, dst) VALUES (?1, ?2, ?3)
             ON CONFLICT (type, src, dst) DO NOTHING",
            params![link_type, source.as_bytes(), target.as_bytes()],
        )?;
    } else {
        tx.execute(
            "DELETE FROM links WHERE type = ?1 AND src = ?2 AND dst = ?3",
            params![link_type, source.as_bytes(), target.as_bytes()],
        )?;
    }
    Ok(())
}

/// Where a `set_field` value lands.
enum Target {
    Column(&'static str, SqlValue),
    SchemeValue(Option<String>, Option<f64>),
    ClearSchemeValue,
}

/// The largest integer an `f64` holds exactly.
const MAX_EXACT: u64 = 1 << 53;

/// Built-in fields are columns; every other field is a Scheme field. Values
/// were checked against the Scheme when written, so this only checks that
/// each fits where it is stored.
fn target(field: &str, value: &Json) -> Result<Target, StoreError> {
    let invalid = |reason: &str| StoreError::Invalid {
        field: field.to_owned(),
        reason: reason.to_owned(),
        valid: Vec::new(),
    };
    let text_column = |name: &str| match name {
        "type" => Some("type"),
        "title" => Some("title"),
        "description" => Some("description"),
        "status" => Some("status"),
        "assignee" => Some("assignee"),
        _ => None,
    };
    Ok(match (field, value) {
        (name, Json::String(text)) if text_column(name).is_some() => Target::Column(
            text_column(name).unwrap_or_default(),
            SqlValue::Text(text.clone()),
        ),
        (name, Json::Null) if text_column(name).is_some() => {
            Target::Column(text_column(name).unwrap_or_default(), SqlValue::Null)
        }
        ("priority", Json::String(level)) => {
            let rank = priority_rank(level).ok_or_else(|| invalid("is not a priority level"))?;
            Target::Column("priority", SqlValue::Integer(rank))
        }
        ("priority", Json::Null) => Target::Column("priority", SqlValue::Integer(0)),
        ("parent", Json::String(id)) => {
            let parent = Uuid::parse_str(id).map_err(|_| invalid("is not an Issue id"))?;
            Target::Column("parent_id", SqlValue::Blob(parent.as_bytes().to_vec()))
        }
        ("parent", Json::Null) => Target::Column("parent_id", SqlValue::Null),
        ("archived", Json::Bool(archived)) => {
            Target::Column("archived", SqlValue::Integer(i64::from(*archived)))
        }
        ("archived", Json::Null) => Target::Column("archived", SqlValue::Integer(0)),
        (
            "type" | "title" | "description" | "status" | "assignee" | "priority" | "parent"
            | "archived",
            _,
        ) => return Err(invalid("has the wrong value type")),
        ("key" | "reporter" | "agent" | "claim" | "labels" | "created" | "updated", _) => {
            return Err(invalid("is computed and cannot be set"));
        }
        (_, Json::Null) => Target::ClearSchemeValue,
        (_, Json::String(text)) => Target::SchemeValue(Some(text.clone()), None),
        (_, Json::Number(number)) => {
            let exact = number
                .as_u64()
                .or_else(|| number.as_i64().map(i64::unsigned_abs))
                .is_none_or(|magnitude| magnitude <= MAX_EXACT);
            // A float was already rounded when parsed, so one that reaches 2^53
            // may have been larger.
            #[allow(clippy::cast_precision_loss)]
            let limit = MAX_EXACT as f64;
            let whole_in_range = |n: f64| {
                n.fract() != 0.0
                    || if number.is_f64() {
                        n.abs() < limit
                    } else {
                        n.abs() <= limit
                    }
            };
            let number = number
                .as_f64()
                .filter(|n| exact && whole_in_range(*n))
                .ok_or_else(|| invalid("is too large to store exactly"))?;
            Target::SchemeValue(None, Some(number))
        }
        (_, Json::Bool(flag)) => Target::SchemeValue(None, Some(f64::from(u8::from(*flag)))),
        (_, Json::Array(_) | Json::Object(_)) => {
            return Err(invalid("cannot hold a list or an object"));
        }
    })
}

fn write_target(
    tx: &Transaction<'_>,
    issue_id: Uuid,
    field: &str,
    target: Target,
) -> Result<(), StoreError> {
    match target {
        Target::Column(column, value) => tx.execute(
            &format!("UPDATE issues SET {column} = ?1 WHERE id = ?2"),
            params![value, issue_id.as_bytes()],
        )?,
        Target::SchemeValue(txt, num) => tx.execute(
            "INSERT OR REPLACE INTO field_values (issue_id, field, txt, num)
             VALUES (?1, ?2, ?3, ?4)",
            params![issue_id.as_bytes(), field, txt, num],
        )?,
        Target::ClearSchemeValue => tx.execute(
            "DELETE FROM field_values WHERE issue_id = ?1 AND field = ?2",
            params![issue_id.as_bytes(), field],
        )?,
    };
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

pub(crate) fn issue_project(
    conn: &Connection,
    issue_id: Uuid,
) -> Result<Option<ProjectId>, StoreError> {
    let text: Option<String> = conn
        .query_row(
            "SELECT project FROM issues WHERE id = ?1",
            [issue_id.as_bytes()],
            |row| row.get(0),
        )
        .optional()?;
    text.map(|text| project_from(&text)).transpose()
}

pub(crate) fn project_from(text: &str) -> Result<ProjectId, StoreError> {
    text.parse()
        .map_err(|_| StoreError::Corrupt(format!("{text:?} is not a Project id")))
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
