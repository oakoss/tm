//! `undo`: one batch's writes reverted by appending operations that restore
//! what each touched field and set element held before it (ADR 0010).

use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};
use tm_core::op::{COMMENTS_FIELD, ClaimAction};
use tm_core::{Actor, Change, Op, ProjectId, Scheme, link};
use uuid::Uuid;

use crate::apply::{Version, issue_project, uuid_from};
use crate::{Draft, Store, StoreError};

/// One field or set element as undo finds it and would leave it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Restore {
    pub issue: Uuid,
    pub field: String,
    /// The set element, for a set field, Link or comment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elem: Option<String>,
    /// What it holds now: a field's value, or whether the element is present.
    pub from: Value,
    pub to: Value,
}

/// What undoing a batch writes, and the operations that write it.
#[derive(Debug, Clone, PartialEq)]
pub struct UndoPlan {
    pub batch: Uuid,
    pub restores: Vec<Restore>,
    changes: Vec<(ProjectId, Uuid, Change)>,
}

impl Store {
    /// What undoing `batch` as `actor` would write. The write is made and
    /// rolled back, so a plan that passes is one the undo can carry out.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownBatch`], [`StoreError::UndoConflict`] naming every
    /// field or element a later batch wrote, [`StoreError::CannotUndo`] for a
    /// Project write, a Claim release or a synced batch, or as
    /// [`Store::append_all`].
    pub fn undo_plan(&mut self, actor: &Actor, batch: Uuid) -> Result<UndoPlan, StoreError> {
        self.undo_with(actor, batch, false).map(|(plan, _)| plan)
    }

    /// Undoes `batch` as `actor`, all or nothing: the plan is made again
    /// inside the write, so a batch written in between is a conflict.
    ///
    /// # Errors
    ///
    /// As [`Store::undo_plan`].
    pub fn undo(&mut self, actor: &Actor, batch: Uuid) -> Result<(UndoPlan, Vec<Op>), StoreError> {
        self.undo_with(actor, batch, true)
    }

    fn undo_with(
        &mut self,
        actor: &Actor,
        batch: Uuid,
        commit: bool,
    ) -> Result<(UndoPlan, Vec<Op>), StoreError> {
        let planned = self.read(|store| plan(&store.conn, &store.scheme, batch))?;
        let drafts = planned
            .changes
            .iter()
            .map(|(project, subject, change)| Draft {
                actor: actor.clone(),
                project: *project,
                subject: *subject,
                change: change.clone(),
            })
            .collect();
        let scheme = self.scheme.clone();
        let guard = |tx: &rusqlite::Transaction<'_>| match plan(tx, &scheme, batch)? {
            again if again == planned => Ok(()),
            again => Err(StoreError::UndoConflict {
                batch,
                conflicts: again.restores,
            }),
        };
        let ops = self.append_with(drafts, guard, commit)?;
        Ok((planned, ops))
    }
}

/// A logged operation of the batch.
struct BatchOp {
    seq: i64,
    op_id: Uuid,
    issue: Uuid,
    change: Change,
}

fn plan(conn: &Connection, scheme: &Scheme, batch: Uuid) -> Result<UndoPlan, StoreError> {
    let ops = batch_ops(conn, batch)?;
    let first_seq = ops.first().ok_or(StoreError::UnknownBatch(batch))?.seq;
    let in_batch: BTreeSet<Uuid> = ops.iter().map(|op| op.op_id).collect();
    let created: BTreeSet<Uuid> = ops
        .iter()
        .filter(|op| matches!(op.change, Change::Create))
        .map(|op| op.issue)
        .collect();
    let cannot = |reason: &str| StoreError::CannotUndo {
        batch,
        reason: reason.to_owned(),
    };

    let mut targets: BTreeSet<(Uuid, String, String)> = BTreeSet::new();
    let mut plan = UndoPlan {
        batch,
        restores: Vec::new(),
        changes: Vec::new(),
    };
    let mut conflicts = Vec::new();
    for issue in &created {
        archive(conn, *issue, &mut plan)?;
    }
    for op in &ops {
        if created.contains(&op.issue) {
            continue;
        }
        match &op.change {
            Change::CreateProject { .. } | Change::RenameProject { .. } => {
                return Err(cannot("it wrote a Project, which undo does not revert"));
            }
            Change::Create => unreachable!("created Issues are archived above"),
            Change::Claim {
                action: ClaimAction::Release { .. },
            } => {
                return Err(cannot(
                    "it released a Claim; take the Issue again with tm claim",
                ));
            }
            Change::Claim {
                action: ClaimAction::Take { .. },
            } => release(conn, op, &mut plan, &mut conflicts)?,
            Change::Comment { .. } => {
                targets.insert((op.issue, COMMENTS_FIELD.into(), op.op_id.to_string()));
            }
            other => {
                let field = other.field().expect("every other Issue kind names a field");
                let elem = other
                    .elem()
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default();
                targets.insert((op.issue, field.to_owned(), elem));
            }
        }
    }
    let batch_ops = BatchContext {
        first_seq,
        in_batch: &in_batch,
        scheme,
    };
    for (issue, field, elem) in targets {
        restore(
            conn,
            (issue, &field, &elem),
            &batch_ops,
            &mut plan,
            &mut conflicts,
        )?;
    }
    if !conflicts.is_empty() {
        return Err(StoreError::UndoConflict { batch, conflicts });
    }
    if plan.changes.is_empty() {
        return Err(cannot(
            "everything it wrote already holds what it held before",
        ));
    }
    Ok(plan)
}

fn batch_ops(conn: &Connection, batch: Uuid) -> Result<Vec<BatchOp>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT o.seq, o.op_id, o.issue_id, o.payload, o.server_seq IS NOT NULL FROM ops o
         JOIN op_batches b ON b.op_id = o.op_id
         WHERE b.batch = ?1 ORDER BY o.seq",
    )?;
    let mut rows = stmt.query([batch.as_bytes()])?;
    let mut ops = Vec::new();
    while let Some(row) = rows.next()? {
        // ADR 0010: undo after sync is designed with sync.
        if row.get::<_, bool>(4)? {
            return Err(StoreError::CannotUndo {
                batch,
                reason:
                    "the server has acknowledged it, and undo of synced writes is not designed yet"
                        .into(),
            });
        }
        let payload: String = row.get(3)?;
        ops.push(BatchOp {
            seq: row.get(0)?,
            op_id: uuid_from(&row.get::<_, Vec<u8>>(1)?)?,
            issue: uuid_from(&row.get::<_, Vec<u8>>(2)?)?,
            change: serde_json::from_str(&payload)
                .map_err(|err| StoreError::Corrupt(format!("a logged payload: {err}")))?,
        });
    }
    Ok(ops)
}

/// A created Issue is archived, which keeps its Key and everything since.
fn archive(conn: &Connection, issue: Uuid, plan: &mut UndoPlan) -> Result<(), StoreError> {
    let archived: bool = conn.query_row(
        "SELECT archived FROM issues WHERE id = ?1",
        [issue.as_bytes()],
        |row| row.get(0),
    )?;
    if archived {
        return Ok(());
    }
    let project = issue_project(conn, issue)?.ok_or(StoreError::UnknownIssue(issue))?;
    plan.restores.push(Restore {
        issue,
        field: "archived".into(),
        elem: None,
        from: json!(false),
        to: json!(true),
    });
    plan.changes
        .push((project, issue, set_field("archived", json!(true))));
    Ok(())
}

/// A take is undone by releasing its Claim, if it still holds the Issue.
fn release(
    conn: &Connection,
    op: &BatchOp,
    plan: &mut UndoPlan,
    conflicts: &mut Vec<Restore>,
) -> Result<(), StoreError> {
    let current = crate::claim::current(conn, op.issue)?.map(|held| held.id);
    if current != Some(op.op_id) {
        conflicts.push(Restore {
            issue: op.issue,
            field: tm_core::op::CLAIM_FIELD.into(),
            elem: None,
            from: json!(current.map(|_| "held by a later Claim")),
            to: Value::Null,
        });
        return Ok(());
    }
    let project = issue_project(conn, op.issue)?.ok_or(StoreError::UnknownIssue(op.issue))?;
    plan.restores.push(Restore {
        issue: op.issue,
        field: tm_core::op::CLAIM_FIELD.into(),
        elem: None,
        from: json!(true),
        to: json!(false),
    });
    plan.changes.push((
        project,
        op.issue,
        Change::Claim {
            action: ClaimAction::Release { claim: op.op_id },
        },
    ));
    Ok(())
}

/// What every target of one batch is restored against.
struct BatchContext<'a> {
    first_seq: i64,
    in_batch: &'a BTreeSet<Uuid>,
    scheme: &'a Scheme,
}

/// One target's operations, unapplied ones aside, with their versions.
struct TargetOp {
    seq: i64,
    version: Version,
    change: Change,
}

/// Restores one field or element to the winner among the operations before
/// the batch, by the rule replay uses. A later operation on it from outside
/// the batch is a conflict.
fn restore(
    conn: &Connection,
    (issue, field, elem): (Uuid, &str, &str),
    batch: &BatchContext<'_>,
    plan: &mut UndoPlan,
    conflicts: &mut Vec<Restore>,
) -> Result<(), StoreError> {
    let current = current_state(conn, batch.scheme, issue, field, elem)?;
    let mut prior: Option<TargetOp> = None;
    let mut written_since = false;
    for target_op in target_ops(conn, issue, field, elem)? {
        if batch.in_batch.contains(&target_op.version.op_id()) {
            continue;
        }
        if target_op.seq > batch.first_seq {
            written_since = true;
        } else if prior
            .as_ref()
            .is_none_or(|best| target_op.version.beats(&best.version))
        {
            prior = Some(target_op);
        }
    }
    let elem_out = (!elem.is_empty()).then(|| elem.to_owned());
    if written_since {
        conflicts.push(Restore {
            issue,
            field: field.to_owned(),
            elem: elem_out,
            from: current,
            to: Value::Null,
        });
        return Ok(());
    }
    // A comment the batch wrote has no earlier state; the batch's own comment
    // is undone by retracting it, a retraction by showing the comment again.
    let wrote_comment = field == COMMENTS_FIELD
        && Uuid::parse_str(elem).is_ok_and(|comment| batch.in_batch.contains(&comment));
    let (to, change) = inverse(field, elem, prior.map(|op| op.change), wrote_comment)?;
    if to == current {
        return Ok(());
    }
    let project = issue_project(conn, issue)?.ok_or(StoreError::UnknownIssue(issue))?;
    plan.restores.push(Restore {
        issue,
        field: field.to_owned(),
        elem: elem_out,
        from: current,
        to,
    });
    plan.changes.push((project, issue, change));
    Ok(())
}

fn target_ops(
    conn: &Connection,
    issue: Uuid,
    field: &str,
    elem: &str,
) -> Result<Vec<TargetOp>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT seq, hlc, replica_id, op_id, base_seq, server_seq, server_received_ms, payload
         FROM ops WHERE issue_id = ?1 AND field = ?2 AND coalesce(elem, '') = ?3
           AND op_id NOT IN (SELECT op_id FROM unapplied)
         ORDER BY server_seq IS NULL, server_seq, hlc, replica_id, op_id",
    )?;
    let mut rows = stmt.query(params![issue.as_bytes(), field, elem])?;
    let mut ops = Vec::new();
    while let Some(row) = rows.next()? {
        let payload: String = row.get(7)?;
        ops.push(TargetOp {
            seq: row.get(0)?,
            version: Version::from_row(row, 1)?,
            change: serde_json::from_str(&payload)
                .map_err(|err| StoreError::Corrupt(format!("a logged payload: {err}")))?,
        });
    }
    Ok(ops)
}

/// What a field holds now, or whether an element is present (a comment:
/// whether it is retracted).
fn current_state(
    conn: &Connection,
    scheme: &Scheme,
    issue: Uuid,
    field: &str,
    elem: &str,
) -> Result<Value, StoreError> {
    if !elem.is_empty() {
        let present: Option<bool> = conn
            .query_row(
                "SELECT present FROM field_versions
                 WHERE issue_id = ?1 AND field = ?2 AND elem = ?3",
                params![issue.as_bytes(), field, elem],
                |row| row.get(0),
            )
            .optional()?;
        return Ok(json!(present.unwrap_or(false)));
    }
    let view = crate::read::issues(conn, scheme, &[issue])?;
    Ok(view
        .first()
        .and_then(|view| view.fields.get(field).cloned())
        .unwrap_or(Value::Null))
}

/// The state the prior winner left and the change that writes it back; with
/// no prior winner, the field is cleared and the element absent.
fn inverse(
    field: &str,
    elem: &str,
    prior: Option<Change>,
    wrote_comment: bool,
) -> Result<(Value, Change), StoreError> {
    let restored = match prior {
        Some(Change::SetField { value, .. }) => (value.clone(), set_field(field, value)),
        // Written with its display spelling, which the write folds back to
        // the stored element, so a restored label reads as it was added.
        Some(Change::Add {
            field,
            label,
            elem,
            elem_label,
        }) => (
            json!(true),
            Change::Add {
                field,
                label,
                elem: elem_label.clone().unwrap_or(elem),
                elem_label,
            },
        ),
        Some(change @ Change::Remove { .. }) => (json!(false), change),
        Some(change @ Change::Link { present, .. }) => (json!(present), change),
        Some(change @ Change::Retract { retracted, .. }) => (json!(retracted), change),
        Some(other) => {
            return Err(StoreError::Corrupt(format!(
                "a {} operation filed under field {field}",
                other.kind()
            )));
        }
        None if field == COMMENTS_FIELD => {
            let comment = Uuid::parse_str(elem)
                .map_err(|_| StoreError::Corrupt(format!("comment element {elem:?}")))?;
            (
                json!(wrote_comment),
                Change::Retract {
                    comment,
                    retracted: wrote_comment,
                },
            )
        }
        None if field == link::FIELD => {
            let (link_type, target) = elem
                .split_once('/')
                .and_then(|(link_type, target)| {
                    Some((link_type.to_owned(), Uuid::parse_str(target).ok()?))
                })
                .ok_or_else(|| StoreError::Corrupt(format!("Link element {elem:?}")))?;
            (
                json!(false),
                Change::Link {
                    link_type,
                    target,
                    present: false,
                },
            )
        }
        None if elem.is_empty() => {
            // Priority and archived always hold a value; unset, they read as these.
            let unset = match field {
                "priority" => json!("none"),
                "archived" => json!(false),
                _ => Value::Null,
            };
            (unset.clone(), set_field(field, unset))
        }
        None => (
            json!(false),
            Change::Remove {
                field: field.to_owned(),
                label: String::new(),
                elem: elem.to_owned(),
                elem_label: None,
            },
        ),
    };
    Ok(restored)
}

fn set_field(field: &str, value: Value) -> Change {
    Change::SetField {
        field: field.to_owned(),
        label: String::new(),
        value,
        value_label: None,
    }
}
