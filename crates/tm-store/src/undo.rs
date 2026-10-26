//! `undo`: one batch's writes reverted by appending operations that restore
//! what each touched field and set element held before it (ADR 0010).

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use serde_json::{Value, json};
use tm_core::op::{COMMENTS_FIELD, ClaimAction};
use tm_core::{Actor, Change, Hlc, Op, ProjectId, Scheme, Subject, link, shortlist};
use uuid::Uuid;

use crate::apply::{Version, issue_project, uuid_from};
use crate::{Draft, Provenance, Store, StoreError, WallTime};

/// One field or set element as undo finds it and would leave it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Restore {
    #[serde(flatten)]
    pub target: Target,
    pub field: String,
    /// The set element, for a set field, Link or comment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elem: Option<String>,
    /// What it holds now: a field's value, or whether the element is present.
    pub from: Value,
    pub to: Value,
}

/// The record a [`Restore`] writes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Target {
    Issue {
        issue: Uuid,
    },
    Shortlist {
        shortlist: Uuid,
        /// None when a merge left the list without one.
        name: Option<String>,
    },
}

/// What undoing a batch writes, and the operations that write it.
#[derive(Debug, Clone, PartialEq)]
pub struct UndoPlan {
    pub batch: Uuid,
    pub restores: Vec<Restore>,
    /// Issues whose Claim the batch released alongside its other writes and
    /// that no Claim holds now; only an Agent can take the Issue again.
    pub released: Vec<Uuid>,
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
    /// Project write, a batch that only released a Claim or a synced batch, or as
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

    /// Whether undoing `batch` would succeed. For the batch this Store just
    /// wrote, the undo was tried inside its write; for any other, only the
    /// plan is read, so a check the write makes can still refuse it.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn can_undo(&self, batch: Uuid) -> Result<bool, StoreError> {
        if let Some(last) = self.last_write
            && last.batch == batch
            && self.write_mark()? == (last.data_version, last.max_seq)
        {
            return Ok(last.undoable);
        }
        match self.read(|store| plan(&store.conn, &store.scheme, batch)) {
            Ok(_) => Ok(true),
            Err(
                StoreError::UnknownBatch(_)
                | StoreError::UndoConflict { .. }
                | StoreError::CannotUndo { .. },
            ) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// The batches whose id starts or ends with `part`, hex digits without
    /// case and with hyphens ignored. Ids begin with their time, so only the
    /// end tells batches written close together apart.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn batches_matching(&self, part: &str) -> Result<Vec<Uuid>, StoreError> {
        let hex: String = part.chars().filter(|c| *c != '-').collect();
        if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT batch FROM op_batches
             WHERE substr(hex(batch), 1, length(?1)) = ?1 OR substr(hex(batch), -length(?1)) = ?1
             LIMIT 5",
        )?;
        let rows = stmt.query_map([hex.to_ascii_uppercase()], |row| row.get::<_, Vec<u8>>(0))?;
        rows.map(|batch| uuid_from(&batch?)).collect()
    }

    fn undo_with(
        &mut self,
        actor: &Actor,
        batch: Uuid,
        commit: bool,
    ) -> Result<(UndoPlan, Vec<Op>), StoreError> {
        let planned = self.read(|store| plan(&store.conn, &store.scheme, batch))?;
        let drafts = planned.drafts(actor);
        let scheme = self.scheme.clone();
        let guard = |tx: &rusqlite::Transaction<'_>| match plan(tx, &scheme, batch)? {
            again if again == planned => Ok(Vec::new()),
            again => Err(StoreError::UndoConflict {
                batch,
                conflicts: again.restores,
            }),
        };
        let ops = self.append_with(drafts, |tx, _| guard(tx), commit, Some(batch))?;
        Ok((planned, ops))
    }

    /// The batches a bare undo by `human` may revert, newest first, at most
    /// `limit`: written by `human` as themself, not an undo, not undone, with
    /// no Project write (ADR 0012) and nothing imported (ADR 0013).
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn undo_candidates(&self, human: &str, limit: usize) -> Result<Vec<Uuid>, StoreError> {
        let undone = undone(&self.conn)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT b.batch FROM op_batches b JOIN ops o ON o.op_id = b.op_id
             WHERE b.op_id = b.batch
               AND b.batch NOT IN (SELECT batch FROM undo_batches)
               AND NOT EXISTS (
                 SELECT 1 FROM op_batches p JOIN ops po ON po.op_id = p.op_id
                 WHERE p.batch = b.batch
                   AND (po.actor <> ?1 OR po.on_behalf_of IS NOT NULL OR po.source IS NOT NULL
                        OR po.kind IN ('create_project', 'rename_project')))
             ORDER BY o.seq DESC",
        )?;
        let mut rows = stmt.query([human])?;
        let mut candidates = Vec::new();
        while candidates.len() < limit {
            let Some(row) = rows.next()? else { break };
            let batch = uuid_from(&row.get::<_, Vec<u8>>(0)?)?;
            if !undone.contains_key(&batch) {
                candidates.push(batch);
            }
        }
        Ok(candidates)
    }
}

/// Every batch an undo standing against it has undone, with that batch's
/// first seq. An undo stands while it applied in full and no undo of it
/// stands; it is newer than what it undid, so the newest is settled first.
fn undone(conn: &Connection) -> Result<BTreeMap<Uuid, i64>, StoreError> {
    let standing = standing_undos(conn)?.into_iter();
    Ok(standing.map(|(undid, (_, seq))| (undid, seq)).collect())
}

/// [`undone`], with the undo that stands against each batch.
pub(crate) fn standing_undos(conn: &Connection) -> Result<BTreeMap<Uuid, (Uuid, i64)>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT u.batch, u.undid, d.seq FROM undo_batches u
         JOIN ops o ON o.op_id = u.batch
         JOIN ops d ON d.op_id = u.undid
         WHERE NOT EXISTS (
           SELECT 1 FROM op_batches x JOIN unapplied n ON n.op_id = x.op_id
           WHERE x.batch = u.batch)
         ORDER BY o.seq DESC",
    )?;
    let mut rows = stmt.query([])?;
    let mut undone = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let undo = uuid_from(&row.get::<_, Vec<u8>>(0)?)?;
        if !undone.contains_key(&undo) {
            undone.insert(uuid_from(&row.get::<_, Vec<u8>>(1)?)?, (undo, row.get(2)?));
        }
    }
    Ok(undone)
}

/// The later batches whose writes a conflict check ignores: each undone batch
/// newer than `first_seq`, with every undo of it, which together change
/// nothing (ADR 0012). An undo of it that is itself undone is newer, so it is
/// in the first set already.
fn ignored_since(conn: &Connection, first_seq: i64) -> Result<BTreeSet<Uuid>, StoreError> {
    let undone = undone(conn)?;
    let later = |batch: &Uuid| undone.get(batch).is_some_and(|seq| *seq > first_seq);
    let mut ignored: BTreeSet<Uuid> = undone
        .keys()
        .filter(|batch| later(batch))
        .copied()
        .collect();
    let mut stmt = conn.prepare_cached("SELECT batch, undid FROM undo_batches")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let undo = uuid_from(&row.get::<_, Vec<u8>>(0)?)?;
        let undid = uuid_from(&row.get::<_, Vec<u8>>(1)?)?;
        if later(&undid) {
            ignored.insert(undo);
        }
    }
    Ok(ignored)
}

impl UndoPlan {
    /// The undo's writes, which carry no source: an undo is the undoer's own.
    fn drafts(&self, actor: &Actor) -> Vec<(Draft, Option<Provenance>)> {
        self.changes
            .iter()
            .map(|(project, subject, change)| {
                let draft = Draft {
                    actor: actor.clone(),
                    project: *project,
                    subject: *subject,
                    change: change.clone(),
                };
                (draft, None)
            })
            .collect()
    }
}

/// Whether `actor` could undo `batch` now: the undo is written inside `tx`
/// with every check it would meet, then rolled back. A refusal is false; only
/// a failure of the store itself fails the write being probed.
pub(crate) fn probe(
    tx: &Transaction<'_>,
    scheme: &Scheme,
    actor: &Actor,
    batch: Uuid,
    now: (WallTime, u64),
    hlc: Hlc,
) -> Result<bool, StoreError> {
    tx.execute_batch("SAVEPOINT undo_probe")?;
    let tried = plan(tx, scheme, batch).and_then(|plan| {
        let drafts = plan.drafts(actor);
        for (draft, _) in &drafts {
            draft.change.validate()?;
        }
        // Unforced, as the undo it offers would run.
        crate::claim::protected(tx, &drafts, now.0.utc_ms, false)?;
        crate::write_drafts(tx, scheme, drafts, (now, true), hlc)
    });
    tx.execute_batch("ROLLBACK TO undo_probe; RELEASE undo_probe")?;
    match tried {
        Ok(_) => Ok(true),
        Err(err) if failed(&err) => Err(err),
        Err(_) => Ok(false),
    }
}

/// An error the store hit, as opposed to a refusal of what was asked.
const fn failed(err: &StoreError) -> bool {
    matches!(
        err,
        StoreError::Busy(_)
            | StoreError::Sqlite(_)
            | StoreError::Corrupt(_)
            | StoreError::Clock(_)
            | StoreError::Replay { .. }
            | StoreError::NewerStore { .. }
            | StoreError::NotAStore(_)
            | StoreError::ReadOnly
    )
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
    let cannot = |reason: &str| StoreError::CannotUndo {
        batch,
        reason: reason.to_owned(),
    };

    let mut targets: BTreeSet<(Subject, Uuid, String, String)> = BTreeSet::new();
    let mut plan = UndoPlan {
        batch,
        restores: Vec::new(),
        released: Vec::new(),
        changes: Vec::new(),
    };
    let mut conflicts = Vec::new();
    let created = remove_created(conn, &ops, &mut plan)?;
    for op in &ops {
        if created.contains(&op.issue) {
            continue;
        }
        match &op.change {
            Change::CreateProject { .. } | Change::RenameProject { .. } => {
                return Err(cannot("it wrote a Project, which undo does not revert"));
            }
            Change::Create | Change::CreateShortlist => {
                unreachable!("created Issues and Short-lists are handled above")
            }
            // A Claim taken since holds the Issue, so nothing stays released.
            Change::Claim {
                action: ClaimAction::Release { .. },
            } if ops.len() > 1 => {
                if crate::claim::current(conn, op.issue)?.is_none() {
                    plan.released.push(op.issue);
                }
            }
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
            // A renewal records that the session was active (ADR 0009), and a
            // list's source where it came from; undoing the batch makes
            // neither untrue, so later imports still find the list.
            Change::Claim {
                action: ClaimAction::Renew { .. },
            }
            | Change::ShortlistSource => {}
            Change::Comment { .. } => {
                targets.insert((
                    Subject::Issue,
                    op.issue,
                    COMMENTS_FIELD.into(),
                    op.op_id.to_string(),
                ));
            }
            other => {
                let field = other.field().expect("every other kind names a field");
                let elem = other
                    .elem()
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default();
                targets.insert((other.subject(), op.issue, field.to_owned(), elem));
            }
        }
    }
    let ignored = ignored_since(conn, first_seq)?;
    let deleted: BTreeSet<Uuid> = ops
        .iter()
        .filter(|op| deletes(&op.change, true))
        .map(|op| op.issue)
        .collect();
    let batch_ops = BatchContext {
        batch,
        deleted: &deleted,
        first_seq,
        in_batch: &in_batch,
        ignored: &ignored,
        scheme,
    };
    for (subject, id, field, elem) in targets {
        restore(
            conn,
            (subject, id, &field, &elem),
            &batch_ops,
            &mut plan,
            &mut conflicts,
        )?;
    }
    if !conflicts.is_empty() {
        return Err(StoreError::UndoConflict { batch, conflicts });
    }
    plan.changes
        .sort_by_key(|(_, _, change)| write_order(change));
    if plan.changes.is_empty() {
        return Err(cannot(
            "everything it wrote already holds what it held before",
        ));
    }
    Ok(plan)
}

/// Writes refuse a deleted list and one left with no Actor, so restores and
/// Actor adds go first and deletes last.
fn write_order(change: &Change) -> u8 {
    match change {
        restore if deletes(restore, false) => 0,
        Change::ShortlistActor { present: true, .. } => 1,
        delete if deletes(delete, true) => 3,
        _ => 2,
    }
}

/// Whether `change` sets a Short-list's deleted mark to `deleted`.
fn deletes(change: &Change, deleted: bool) -> bool {
    matches!(change, Change::SetShortlist { field: shortlist::Field::Deleted, value }
        if value == &json!(deleted))
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

/// Archives each Issue and deletes each Short-list the batch created, and
/// returns them: their other writes in the batch need no undo of their own.
fn remove_created(
    conn: &Connection,
    ops: &[BatchOp],
    plan: &mut UndoPlan,
) -> Result<BTreeSet<Uuid>, StoreError> {
    let mut created = BTreeSet::new();
    for op in ops {
        match op.change {
            Change::Create if created.insert(op.issue) => archive(conn, op.issue, plan)?,
            Change::CreateShortlist if created.insert(op.issue) => {
                tombstone(conn, op.issue, plan)?;
            }
            _ => {}
        }
    }
    Ok(created)
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
        target: Target::Issue { issue },
        field: "archived".into(),
        elem: None,
        from: json!(false),
        to: json!(true),
    });
    plan.changes
        .push((project, issue, set_field("archived", json!(true))));
    Ok(())
}

/// A created Short-list is deleted, which keeps its record and everything since.
fn tombstone(conn: &Connection, list: Uuid, plan: &mut UndoPlan) -> Result<(), StoreError> {
    let read = crate::shortlist::read(conn, list)?.ok_or(StoreError::UnknownShortlist(list))?;
    if read.deleted {
        return Ok(());
    }
    plan.restores.push(Restore {
        target: Target::Shortlist {
            shortlist: list,
            name: read.name,
        },
        field: shortlist::Field::Deleted.as_str().into(),
        elem: None,
        from: json!(false),
        to: json!(true),
    });
    plan.changes.push((
        read.project,
        list,
        Change::SetShortlist {
            field: shortlist::Field::Deleted,
            value: json!(true),
        },
    ));
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
            target: Target::Issue { issue: op.issue },
            field: tm_core::op::CLAIM_FIELD.into(),
            elem: None,
            from: json!(current.map(|_| "held by a later Claim")),
            to: Value::Null,
        });
        return Ok(());
    }
    let project = issue_project(conn, op.issue)?.ok_or(StoreError::UnknownIssue(op.issue))?;
    plan.restores.push(Restore {
        target: Target::Issue { issue: op.issue },
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
    batch: Uuid,
    /// The Short-lists the batch deleted, which its undo restores first.
    deleted: &'a BTreeSet<Uuid>,
    first_seq: i64,
    in_batch: &'a BTreeSet<Uuid>,
    ignored: &'a BTreeSet<Uuid>,
    scheme: &'a Scheme,
}

/// One target's operations, unapplied ones aside, with their versions.
struct TargetOp {
    seq: i64,
    batch: Option<Uuid>,
    version: Version,
    change: Change,
}

/// Restores one field or element to the winner among the operations before
/// the batch, by the rule replay uses. A later operation on it from outside
/// the batch is a conflict, unless its batch is ignored.
fn restore(
    conn: &Connection,
    (subject, issue, field, elem): (Subject, Uuid, &str, &str),
    batch: &BatchContext<'_>,
    plan: &mut UndoPlan,
    conflicts: &mut Vec<Restore>,
) -> Result<(), StoreError> {
    let list = match subject {
        Subject::Shortlist => {
            Some(crate::shortlist::read(conn, issue)?.ok_or(StoreError::UnknownShortlist(issue))?)
        }
        Subject::Issue | Subject::Project => None,
    };
    // A list deleted since takes no writes until its delete is undone.
    if let Some(list) = &list
        && list.deleted
        && field != shortlist::Field::Deleted.as_str()
        && !batch.deleted.contains(&issue)
    {
        return Err(StoreError::CannotUndo {
            batch: batch.batch,
            reason: format!(
                "Short-list {} has been deleted since; undo its delete first",
                list.name.as_deref().unwrap_or("without a name")
            ),
        });
    }
    let current = match &list {
        Some(_) => list_state(conn, issue, field, elem)?,
        None => current_state(conn, batch.scheme, issue, field, elem)?,
    };
    let mut prior: Option<TargetOp> = None;
    let mut written_since = false;
    for target_op in target_ops(conn, issue, field, elem)? {
        if batch.in_batch.contains(&target_op.version.op_id()) {
            continue;
        }
        if target_op.seq > batch.first_seq {
            written_since |= target_op
                .batch
                .is_none_or(|op_batch| !batch.ignored.contains(&op_batch));
        } else if prior
            .as_ref()
            .is_none_or(|best| target_op.version.beats(&best.version))
        {
            prior = Some(target_op);
        }
    }
    let elem_out = (!elem.is_empty()).then(|| elem.to_owned());
    let target = match &list {
        Some(list) => Target::Shortlist {
            shortlist: issue,
            name: list.name.clone(),
        },
        None => Target::Issue { issue },
    };
    if written_since {
        conflicts.push(Restore {
            target,
            field: field.to_owned(),
            elem: elem_out,
            from: current,
            to: Value::Null,
        });
        return Ok(());
    }
    let prior = prior.map(|op| op.change);
    let (to, change) = if list.is_some() {
        list_inverse(field, elem, prior).ok_or_else(|| StoreError::CannotUndo {
            batch: batch.batch,
            reason: format!("Short-list field {field} held no value before it to restore"),
        })?
    } else {
        // A comment the batch wrote has no earlier state; the batch's own comment
        // is undone by retracting it, a retraction by showing the comment again.
        let wrote_comment = field == COMMENTS_FIELD
            && Uuid::parse_str(elem).is_ok_and(|comment| batch.in_batch.contains(&comment));
        inverse(field, elem, prior, wrote_comment)?
    };
    if to == current {
        return Ok(());
    }
    let project = match &list {
        Some(list) => list.project,
        None => issue_project(conn, issue)?.ok_or(StoreError::UnknownIssue(issue))?,
    };
    plan.restores.push(Restore {
        target,
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
        "SELECT o.seq, o.hlc, o.replica_id, o.op_id, o.base_seq, o.server_seq,
                o.server_received_ms, o.payload, b.batch
         FROM ops o LEFT JOIN op_batches b ON b.op_id = o.op_id
         WHERE o.issue_id = ?1 AND o.field = ?2 AND coalesce(o.elem, '') = ?3
           AND o.op_id NOT IN (SELECT op_id FROM unapplied)
         ORDER BY o.server_seq IS NULL, o.server_seq, o.hlc, o.replica_id, o.op_id",
    )?;
    let mut rows = stmt.query(params![issue.as_bytes(), field, elem])?;
    let mut ops = Vec::new();
    while let Some(row) = rows.next()? {
        let payload: String = row.get(7)?;
        ops.push(TargetOp {
            seq: row.get(0)?,
            batch: row
                .get::<_, Option<Vec<u8>>>(8)?
                .map(|batch| uuid_from(&batch))
                .transpose()?,
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
    // Stored, not as read: a membership readers hide can still be undone.
    if let Some(stored) = crate::shortlist::stored(conn, issue, field)? {
        return Ok(stored);
    }
    let view = crate::read::issues(conn, scheme, &[issue])?;
    Ok(view
        .first()
        .and_then(|view| view.fields.get(field).cloned())
        .unwrap_or(Value::Null))
}

/// What a Short-list field holds now, or whether an Actor is on it.
fn list_state(conn: &Connection, list: Uuid, field: &str, elem: &str) -> Result<Value, StoreError> {
    if !elem.is_empty() {
        let present: Option<bool> = conn
            .query_row(
                "SELECT present FROM shortlist_versions
                 WHERE shortlist_id = ?1 AND field = ?2 AND elem = ?3",
                params![list.as_bytes(), field, elem],
                |row| row.get(0),
            )
            .optional()?;
        return Ok(json!(present.unwrap_or(false)));
    }
    let read = crate::shortlist::read(conn, list)?.ok_or(StoreError::UnknownShortlist(list))?;
    Ok(match field {
        "name" => json!(read.name),
        "size" => json!(read.size),
        "deleted" => json!(read.deleted),
        other => return Err(StoreError::Corrupt(format!("Short-list field {other:?}"))),
    })
}

/// [`inverse`] for a Short-list. With no prior winner, the list is not
/// deleted, an Actor is absent and the size is the default; a name has no
/// value to restore, which a merge that brought the list without one leaves,
/// so none is returned and the batch cannot be undone.
fn list_inverse(field: &str, elem: &str, prior: Option<Change>) -> Option<(Value, Change)> {
    let unset = |field, value: Value| Some((value.clone(), Change::SetShortlist { field, value }));
    match prior {
        Some(Change::SetShortlist { field, value }) => unset(field, value),
        Some(change @ Change::ShortlistActor { present, .. }) => Some((json!(present), change)),
        None if field == shortlist::ACTORS => Some((
            json!(false),
            Change::ShortlistActor {
                actor: elem.to_owned(),
                present: false,
            },
        )),
        None if field == shortlist::Field::Deleted.as_str() => {
            unset(shortlist::Field::Deleted, json!(false))
        }
        None if field == shortlist::Field::Size.as_str() => {
            unset(shortlist::Field::Size, json!(shortlist::DEFAULT_SIZE))
        }
        _ => None,
    }
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
