//! Record import of Short-lists (ADR 0016). Each list a file names, on a
//! project line or an Issue record, is found by its source, on the list it
//! names or on the create or source operation an earlier import wrote, then
//! by name in its Project; a list neither finds is created, its create
//! carrying the source. One matched by name gains the source in an operation
//! of its own. Its name, size and Actors are then compared three ways, as
//! ADR 0013 compares an Issue's fields.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};
use tm_core::shortlist::{self as core, Field};
use tm_core::{Actor, Change, ProjectId, Scheme, rank};
use uuid::Uuid;

use crate::apply::{issue_project, uuid_from};
use crate::import::{Conflict, Decision, Parsed, RecordOutcome, adopted, base, decide, refused};
use crate::import_record::{BySource, Planned, by_source, drafter, sourced, three_way};
use crate::membership::PassedOver;
use crate::record::{ListRecord, TM_SOURCE};
use crate::{Draft, Provenance, Store, StoreError, WallTime};

/// What an import did with one Short-list the file names.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListOutcome {
    /// The line that first names it.
    pub line: usize,
    pub source: String,
    pub list: Uuid,
    /// The list's name after the import.
    pub name: Option<String>,
    pub created: bool,
    /// Matched by name, so the import recorded the source on the list.
    pub by_name: bool,
    /// Deleted here, so nothing is written to it, and each record placing
    /// an Issue on it is a conflict.
    pub deleted: bool,
    pub batches: Vec<Uuid>,
    /// Each field set, or `actors/<name>` for an Actor added.
    pub written: Vec<String>,
    /// Each Actor removed, as `actors/<name>`.
    pub removed: Vec<String>,
    /// Fields both tm and the source changed; tm's value was kept.
    pub conflicts: Vec<Conflict>,
    /// The Issues it put on the list, and those it took off, moved to
    /// another list included.
    pub placed: Vec<Uuid>,
    pub taken_off: Vec<Uuid>,
    /// Each member after the import that `tm next` passes over; an import
    /// places what an add would refuse.
    pub passed_over: Vec<Passed>,
    /// The members after the import, which can pass its size.
    pub members: usize,
    pub size: u64,
}

/// A member `tm next` passes over, with why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Passed {
    pub issue: Uuid,
    pub key: Option<String>,
    pub reasons: Vec<PassedOver>,
}

/// A list the file names: its project line if it has one, else what the
/// first record naming it says.
struct Wanted<'a> {
    line: usize,
    project: &'a str,
    name: Option<&'a str>,
    record: Option<&'a ListRecord>,
}

impl Planned for ListPlan {
    fn drafts(&self) -> &[(Draft, Provenance)] {
        &self.drafts
    }
}

/// One list's own writes as one batch, and what they do.
#[derive(PartialEq)]
struct ListPlan {
    list: Uuid,
    created: bool,
    by_name: bool,
    deleted: bool,
    drafts: Vec<(Draft, Provenance)>,
    written: Vec<String>,
    removed: Vec<String>,
    conflicts: Vec<Conflict>,
}

/// Where a record wants its Issue, decided as ADR 0013 decides a field.
struct Want<'a> {
    record: usize,
    line: usize,
    source: &'a str,
    issue: Uuid,
    list: Option<Uuid>,
    tm: Option<Uuid>,
    position: u64,
    finished: bool,
    deleted: bool,
    decision: Decision,
}

/// One batch of places: the records it put on its list, moved from
/// another, took off theirs, or wrote no change for.
#[derive(PartialEq)]
struct Places {
    drafts: Vec<(Draft, Provenance)>,
    placed: Vec<usize>,
    moved: Vec<(usize, Uuid)>,
    taken_off: Vec<(usize, Uuid)>,
    touched: Vec<usize>,
}

/// How a list was found.
enum Found {
    Source(Uuid),
    Name(Uuid),
    Deleted(Uuid),
    None,
}

impl Store {
    /// Finds or creates each Short-list `parsed` names, before any record is
    /// written, so the records can be placed on them; each list's writes are
    /// one batch, added to `written` for a later refusal to name.
    pub(crate) fn import_lists(
        &mut self,
        parsed: &Parsed,
        runner: &Actor,
        now: WallTime,
        written: &mut Vec<Uuid>,
    ) -> Result<Vec<ListOutcome>, StoreError> {
        let wanted = wanted(parsed)?;
        let mut outcomes = Vec::with_capacity(wanted.len());
        for (source, wanted) in &wanted {
            let outcome = self
                .import_list(source, wanted, runner, now)
                .map_err(|cause| refused(wanted.line, source, cause, (written, &[])))?;
            written.extend(&outcome.batches);
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }

    fn import_list(
        &mut self,
        source: &str,
        wanted: &Wanted<'_>,
        runner: &Actor,
        now: WallTime,
    ) -> Result<ListOutcome, StoreError> {
        let project = self.import_project(wanted.project)?;
        let new = Uuid::now_v7();
        let writer = (runner, now);
        let plan = |conn: &Connection| plan_list(conn, (project, source, wanted), new, writer);
        let planned = plan(&self.conn)?;
        let batches = self.write_planned(&planned, plan, ("shortlist", source))?;
        Ok(ListOutcome {
            line: wanted.line,
            source: source.to_owned(),
            list: planned.list,
            name: None,
            created: planned.created,
            by_name: planned.by_name,
            deleted: planned.deleted,
            batches,
            written: planned.written,
            removed: planned.removed,
            conflicts: planned.conflicts,
            placed: Vec::new(),
            taken_off: Vec::new(),
            passed_over: Vec::new(),
            members: 0,
            size: 0,
        })
    }

    /// Pass three: each record's place, once its list and Issue exist. A
    /// list's places are one batch, and taking Issues off their lists one
    /// more. An Issue joins where the file puts it among the members the
    /// file and the list share; the rest keep tm's order. `cleared` holds
    /// each record whose finish in pass one took its Issue off a list.
    pub(crate) fn import_places(
        &mut self,
        parsed: &Parsed,
        (records, lists, cleared): (&mut [RecordOutcome], &mut [ListOutcome], &[(usize, Uuid)]),
        (runner, now): (&Actor, WallTime),
        written: &mut Vec<Uuid>,
    ) -> Result<(), StoreError> {
        for &(index, from) in cleared {
            if let Some(outcome) = lists.iter_mut().find(|outcome| outcome.list == from) {
                let record = &records[index];
                outcome.taken_off.push(record.issue);
                name(&mut outcome.batches, record.batches.first().copied());
            }
        }
        let by_source: BTreeMap<String, Uuid> = lists
            .iter()
            .map(|outcome| (outcome.source.clone(), outcome.list))
            .collect();
        let issues: Vec<Uuid> = records.iter().map(|outcome| outcome.issue).collect();
        let scheme = self.scheme.clone();
        let decided = |conn: &Connection| {
            wants(conn, &scheme, parsed, &issues, &by_source).map_err(|failed| failed.1)
        };
        let first = wants(&self.conn, &scheme, parsed, &issues, &by_source).map_err(|failed| {
            let (index, cause) = *failed;
            let (line, held) = &parsed.issues[index];
            refused(*line, &held.source, cause, (written, records))
        })?;
        for want in &first {
            check_project(&self.conn, want, lists)
                .map_err(|cause| refused(want.line, want.source, cause, (written, records)))?;
        }
        for want in first.iter().filter(|w| w.decision == Decision::Conflict) {
            let reason = match (want.deleted, want.finished) {
                (true, _) => Some("the source's list is deleted here"),
                (_, true) => Some("the Issue is finished"),
                _ => None,
            };
            let conflict = place_value(&self.conn, want.tm).and_then(|tm| {
                Ok(Conflict {
                    field: "shortlist".into(),
                    tm,
                    source: place_value(&self.conn, want.list)?,
                    reason,
                })
            });
            let conflict = conflict
                .map_err(|cause| refused(want.line, want.source, cause, (written, records)))?;
            records[want.record].conflicts.push(conflict);
        }
        // Lists in source order, then the Issues the file takes off any list.
        let targets: Vec<_> = lists
            .iter()
            .map(|outcome| (Some(outcome.list), outcome.source.clone(), outcome.line))
            .chain([(None, String::new(), 0)])
            .collect();
        for (list, source, line) in targets {
            let writer = (runner, source.as_str(), now);
            let plan = |conn: &Connection| places(conn, list, &decided(conn)?, writer);
            let planned = plan(&self.conn)
                .map_err(|cause| refused(line, &source, cause, (written, records)))?;
            if planned.drafts.is_empty() {
                continue;
            }
            let (line, source) = match (list, planned.touched.first()) {
                (None, Some(&index)) => (parsed.issues[index].0, records[index].source.clone()),
                _ => (line, source.clone()),
            };
            let subject = planned.drafts[0].0.subject;
            let ops = self
                .append_imported_guarded(planned.drafts.clone(), |tx| {
                    if plan(tx)? == planned {
                        Ok(())
                    } else {
                        Err(StoreError::ImportChanged(subject.into()))
                    }
                })
                .map_err(|cause| refused(line, &source, cause, (written, records)))?;
            let batch = ops.first().map(|op| op.op_id);
            written.extend(batch);
            report(&planned, batch, list, (records, lists));
        }
        for outcome in lists.iter_mut() {
            self.settle(outcome).map_err(|cause| {
                refused(outcome.line, &outcome.source, cause, (written, records))
            })?;
        }
        Ok(())
    }

    /// Reads back what a list holds after the import.
    fn settle(&self, outcome: &mut ListOutcome) -> Result<(), StoreError> {
        let list = crate::shortlist::read(&self.conn, outcome.list)?
            .ok_or(StoreError::UnknownShortlist(outcome.list))?;
        outcome.name = list.name;
        outcome.size = list.size;
        let members: Vec<Uuid> = crate::membership::ranked(&self.conn, outcome.list)?
            .into_iter()
            .map(|(issue, _)| issue)
            .collect();
        outcome.members = members.len();
        outcome.passed_over = self
            .eligibility(&members)?
            .into_iter()
            .filter(|member| !member.passed_over.is_empty())
            .map(|member| Passed {
                issue: member.issue.id,
                key: member.issue.key,
                reasons: member.passed_over,
            })
            .collect();
        Ok(())
    }
}

/// Files a written batch of places under the records and lists it touched.
fn report(
    planned: &Places,
    batch: Option<Uuid>,
    list: Option<Uuid>,
    (records, lists): (&mut [RecordOutcome], &mut [ListOutcome]),
) {
    fn list_of(lists: &mut [ListOutcome], id: Uuid) -> Option<&mut ListOutcome> {
        lists.iter_mut().find(|outcome| outcome.list == id)
    }
    for &index in &planned.placed {
        records[index].written.push("shortlist".into());
    }
    for &(index, from) in planned.moved.iter().chain(&planned.taken_off) {
        if let Some(outcome) = list_of(lists, from) {
            outcome.taken_off.push(records[index].issue);
            name(&mut outcome.batches, batch);
        }
    }
    for &(index, _) in &planned.taken_off {
        records[index].removed.push("shortlist".into());
    }
    for &index in &planned.touched {
        let record = &mut records[index];
        let mut said = record.written.iter().chain(&record.removed);
        if !said.any(|f| f == "shortlist") {
            record.written.push("shortlist".into());
        }
    }
    for &index in planned.placed.iter().chain(&planned.touched) {
        records[index].batches.extend(batch);
    }
    if let Some(outcome) = list.and_then(|list| list_of(lists, list)) {
        name(&mut outcome.batches, batch);
        let placed = planned.placed.iter().map(|&index| records[index].issue);
        outcome.placed.extend(placed);
    }
}

/// Adds `batch` to `batches` once, as one batch can move several Issues.
fn name(batches: &mut Vec<Uuid>, batch: Option<Uuid>) {
    if let Some(batch) = batch
        && !batches.contains(&batch)
    {
        batches.push(batch);
    }
}

/// Refuses a record whose Issue is in another Project than the list it
/// names, which the read rule would hide (ADR 0016).
fn check_project(
    conn: &Connection,
    want: &Want<'_>,
    lists: &[ListOutcome],
) -> Result<(), StoreError> {
    let Some(list) = want.list else {
        return Ok(());
    };
    let held = crate::shortlist::read(conn, list)?.ok_or(StoreError::UnknownShortlist(list))?;
    let project = issue_project(conn, want.issue)?.ok_or(StoreError::UnknownIssue(want.issue))?;
    if project == held.project {
        return Ok(());
    }
    let name = lists
        .iter()
        .find(|outcome| outcome.list == list)
        .map_or(list.to_string(), |outcome| outcome.source.clone());
    Err(StoreError::Invalid {
        field: "shortlist".into(),
        reason: format!("names {name}, a Short-list in another Project than its Issue"),
        valid: Vec::new(),
    })
}

/// Each record's wanted place. With nothing imported before, an Issue this
/// import's source made was on no list there; for one it found by its `tm:`
/// id, a record naming no list says nothing of tm's. A finished Issue, or
/// one on a list deleted here, is not placed (ADR 0016), and a record
/// asking for it is a conflict.
fn wants<'a>(
    conn: &Connection,
    scheme: &Scheme,
    parsed: &'a Parsed,
    issues: &[Uuid],
    by_source: &BTreeMap<String, Uuid>,
) -> Result<Vec<Want<'a>>, Box<(usize, StoreError)>> {
    let mut wants = Vec::with_capacity(issues.len());
    for (record, ((line, held), &issue)) in parsed.issues.iter().zip(issues).enumerate() {
        let fail = |cause: StoreError| Box::new((record, cause));
        let list = match &held.shortlist {
            Some(member) => Some(*by_source.get(&member.source).ok_or_else(|| {
                fail(StoreError::Corrupt(format!(
                    "Short-list {} was not imported",
                    member.source
                )))
            })?),
            None => None,
        };
        let tm = place_of(conn, issue).map_err(fail)?;
        let mut base = imported_place(conn, issue, &held.source).map_err(fail)?;
        if base.is_none() && !adopted(conn, &held.source, issue).map_err(fail)? {
            base = Some(None);
        }
        let mut decision = match (&base, list) {
            (None, None) => Decision::Keep,
            _ => decide(&list, base.as_ref(), &tm),
        };
        let finished = crate::shortlist::finished(conn, scheme, issue).map_err(fail)?;
        let deleted = match list {
            Some(id) => crate::shortlist::read(conn, id)
                .map_err(fail)?
                .is_some_and(|held| held.deleted),
            None => false,
        };
        if (finished || deleted) && list.is_some() && list != tm {
            decision = Decision::Conflict;
        }
        wants.push(Want {
            record,
            line: *line,
            source: &held.source,
            issue,
            list,
            tm,
            position: held.shortlist.as_ref().map_or(0, |member| member.position),
            finished,
            deleted,
            decision,
        });
    }
    Ok(wants)
}

/// The place the last import from `source` wrote for `issue`, a list or
/// none; none at all when it wrote no place.
#[allow(clippy::option_option)]
fn imported_place(
    conn: &Connection,
    issue: Uuid,
    source: &str,
) -> Result<Option<Option<Uuid>>, StoreError> {
    let corrupt = |reason: String| StoreError::Corrupt(format!("an imported place: {reason}"));
    match imported(conn, issue, source, "shortlist", None)? {
        None => Ok(None),
        Some(Change::SetField {
            value: Value::Null, ..
        }) => Ok(Some(None)),
        Some(Change::SetField {
            value: Value::String(id),
            ..
        }) => {
            let list = Uuid::parse_str(&id).map_err(|err| corrupt(err.to_string()))?;
            Ok(Some(Some(list)))
        }
        Some(other) => Err(corrupt(format!("not a list: {other:?}"))),
    }
}

/// The list `issue` reads as a member of (ADR 0016).
pub(crate) fn place_of(conn: &Connection, issue: Uuid) -> Result<Option<Uuid>, StoreError> {
    let list: Option<Vec<u8>> = conn
        .query_row(
            "SELECT i.shortlist FROM issues i
             JOIN shortlists l ON l.id = i.shortlist AND l.deleted = 0 AND l.project = i.project
             WHERE i.id = ?1 AND i.shortlist_rank IS NOT NULL",
            [issue.as_bytes()],
            |row| row.get(0),
        )
        .optional()?;
    list.map(|id| uuid_from(&id)).transpose()
}

/// The writes that bring `list` to the records' places, or with no list,
/// take each Issue whose record left its list off it. A joining Issue goes
/// after the nearest Issue the file puts before it that stays on the list,
/// else before the nearest after it, else at the end; a gap with no rank
/// left respaces the whole list.
fn places(
    conn: &Connection,
    list: Option<Uuid>,
    wants: &[Want<'_>],
    (runner, list_source, now): (&Actor, &str, WallTime),
) -> Result<Places, StoreError> {
    let write = |want: &&Want<'_>| want.decision == Decision::Write;
    let mut planned = Places {
        drafts: Vec::new(),
        placed: Vec::new(),
        moved: Vec::new(),
        taken_off: Vec::new(),
        touched: Vec::new(),
    };
    let push = |issue: Uuid, field: &str, value: Value, source: &str| {
        let project = issue_project(conn, issue)?.ok_or(StoreError::UnknownIssue(issue))?;
        let draft = Draft {
            actor: runner.clone(),
            project,
            subject: issue,
            change: Change::SetField {
                field: field.into(),
                label: String::new(),
                value,
                value_label: None,
            },
        };
        let provenance = Provenance {
            source: source.to_owned(),
            at: now,
        };
        Ok::<_, StoreError>((draft, provenance))
    };
    let drafts = &mut planned.drafts;
    let Some(list) = list else {
        // A source that left a list tm already left is still written, so
        // the next import compares from it.
        for want in wants.iter().filter(|w| write(w) && w.list.is_none()) {
            // Both, as a membership the read rule hides can still hold a rank.
            let (issue, source) = (want.issue, want.source);
            drafts.push(push(issue, "shortlist", Value::Null, source)?);
            drafts.push(push(issue, "shortlist_rank", Value::Null, source)?);
            if let Some(from) = want.tm {
                planned.taken_off.push((want.record, from));
            }
            planned.touched.push(want.record);
        }
        return Ok(planned);
    };
    let leaving: BTreeSet<Uuid> = wants
        .iter()
        .filter(|want| write(want) && want.tm == Some(list) && want.list != Some(list))
        .map(|want| want.issue)
        .collect();
    let mut named: Vec<&Want<'_>> = wants.iter().filter(|w| w.list == Some(list)).collect();
    named.sort_by_key(|want| (want.position, want.line));
    let order: Vec<(Uuid, String)> = crate::membership::ranked(conn, list)?
        .into_iter()
        .filter(|(issue, _)| !leaving.contains(issue))
        .collect();
    let at = |issue: Uuid| order.iter().position(|(member, _)| *member == issue);
    let mut gaps: BTreeMap<usize, Vec<Uuid>> = BTreeMap::new();
    for (index, want) in named.iter().enumerate() {
        if !write(want) || want.tm == Some(list) {
            continue;
        }
        let gap = named[..index]
            .iter()
            .rev()
            .find_map(|before| at(before.issue).map(|i| i + 1))
            .or_else(|| named[index + 1..].iter().find_map(|after| at(after.issue)))
            .unwrap_or(order.len());
        gaps.entry(gap).or_default().push(want.issue);
    }
    let joining: BTreeMap<Uuid, &Want<'_>> = named
        .iter()
        .filter(|want| gaps.values().flatten().any(|issue| *issue == want.issue))
        .map(|want| (want.issue, *want))
        .collect();
    let id = json!(list.to_string());
    for (issue, rank) in ranks(&order, &gaps)? {
        let rank = json!(rank);
        match joining.get(&issue) {
            Some(want) => {
                drafts.push(push(issue, "shortlist", id.clone(), want.source)?);
                drafts.push(push(issue, "shortlist_rank", rank, want.source)?);
                planned.placed.push(want.record);
                if let Some(from) = want.tm {
                    planned.moved.push((want.record, from));
                }
            }
            None => drafts.push(push(issue, "shortlist_rank", rank, list_source)?),
        }
    }
    // A source that moved to tm's place is still written, so the next
    // import compares from it.
    for want in named.iter().filter(|w| write(w) && w.tm == Some(list)) {
        drafts.push(push(want.issue, "shortlist", id.clone(), want.source)?);
        planned.touched.push(want.record);
    }
    Ok(planned)
}

/// The ranks that put each gap's Issues before the member at that index of
/// `order`, the end's after the last; with no rank left in a gap, every
/// rank that respacing the whole list changes.
fn ranks(
    order: &[(Uuid, String)],
    gaps: &BTreeMap<usize, Vec<Uuid>>,
) -> Result<Vec<(Uuid, String)>, StoreError> {
    let mut ranks = Vec::new();
    for (gap, joining) in gaps {
        let prev = gap.checked_sub(1).map(|i| order[i].1.as_str());
        let next = order.get(*gap).map(|member| member.1.as_str());
        let Some(spread) = rank::spread(prev, next, joining.len()) else {
            ranks.clear();
            break;
        };
        ranks.extend(joining.iter().copied().zip(spread));
    }
    if ranks.len() == gaps.values().map(Vec::len).sum::<usize>() {
        return Ok(ranks);
    }
    let mut full = Vec::new();
    for (index, (member, _)) in order.iter().enumerate() {
        full.extend(gaps.get(&index).into_iter().flatten());
        full.push(*member);
    }
    full.extend(gaps.get(&order.len()).into_iter().flatten());
    let spread = rank::spread(None, None, full.len()).ok_or_else(|| StoreError::Invalid {
        field: "shortlist".into(),
        reason: "the import makes the list too long to rank".into(),
        valid: Vec::new(),
    })?;
    Ok(full
        .into_iter()
        .zip(spread)
        .filter(|(issue, rank)| !order.contains(&(*issue, rank.clone())))
        .collect())
}

/// A place as a conflict shows it: the list's id and name, or null.
fn place_value(conn: &Connection, list: Option<Uuid>) -> Result<Value, StoreError> {
    let Some(id) = list else {
        return Ok(Value::Null);
    };
    let name = crate::shortlist::read(conn, id)?.and_then(|held| held.name);
    Ok(json!({"id": id.to_string(), "name": name}))
}

/// Refuses, before anything is written, a file whose lines disagree about a
/// list or give a place no list holds.
pub(crate) fn check_lists(parsed: &Parsed) -> Result<(), StoreError> {
    wanted(parsed).map(|_| ())
}

/// Each list the file names, by source, in source order.
fn wanted(parsed: &Parsed) -> Result<BTreeMap<&str, Wanted<'_>>, StoreError> {
    let mut wanted: BTreeMap<&str, Wanted<'_>> = BTreeMap::new();
    let invalid = |line: usize, reason: String| StoreError::InvalidExport { line, reason };
    let unreadable = |line: usize, source: &str| {
        let id = source.strip_prefix(TM_SOURCE)?;
        let bad = Uuid::parse_str(id).is_err();
        bad.then(|| invalid(line, format!("Short-list source {source} names no tm id")))
    };
    for (line, project) in &parsed.projects {
        for record in &project.shortlists {
            if let Some(err) = unreadable(*line, &record.source) {
                return Err(err);
            }
            let list = Wanted {
                line: *line,
                project: &project.key,
                name: record.name.as_deref(),
                record: Some(record),
            };
            if wanted.insert(&record.source, list).is_some() {
                let reason = format!("Short-list {} is named twice", record.source);
                return Err(invalid(*line, reason));
            }
        }
    }
    for (line, record) in &parsed.issues {
        let Some(member) = &record.shortlist else {
            continue;
        };
        if member.position == 0 {
            return Err(invalid(*line, "a Short-list position starts at 1".into()));
        }
        if let Some(err) = unreadable(*line, &member.source) {
            return Err(err);
        }
        let entry = wanted.entry(&member.source).or_insert(Wanted {
            line: *line,
            project: &record.project,
            name: member.name.as_deref(),
            record: None,
        });
        if !entry.project.eq_ignore_ascii_case(&record.project) {
            let reason = format!(
                "Short-list {} is in Project {}, not {}",
                member.source, entry.project, record.project
            );
            return Err(invalid(*line, reason));
        }
        if let (Some(first), Some(name)) = (entry.name, member.name.as_deref())
            && first != name
        {
            let reason = format!(
                "Short-list {} is named {name}, but line {} names it {first}",
                member.source, entry.line
            );
            return Err(invalid(*line, reason));
        }
    }
    Ok(wanted)
}

/// One list's writes: create it with the source, or for a list found,
/// record the source if found by name and bring its fields to the file's.
fn plan_list(
    conn: &Connection,
    (project, source, wanted): (ProjectId, &str, &Wanted<'_>),
    new: Uuid,
    writer: (&Actor, WallTime),
) -> Result<ListPlan, StoreError> {
    let found = found(conn, project, source, wanted.name)?;
    let deleted = matches!(found, Found::Deleted(_));
    let (list, created, by_name) = match found {
        Found::Source(list) | Found::Deleted(list) => (list, false, false),
        Found::Name(list) => (list, false, true),
        Found::None => (new, true, false),
    };
    if let Some(held) = crate::shortlist::read(conn, list)?
        && held.project != project
    {
        return Err(StoreError::Invalid {
            field: "shortlist".into(),
            reason: format!("{source} is a Short-list in another Project here"),
            valid: Vec::new(),
        });
    }
    let draft = drafter(writer, project, list, source);
    let mut plan = ListPlan {
        list,
        created,
        by_name,
        deleted,
        drafts: Vec::new(),
        written: Vec::new(),
        removed: Vec::new(),
        conflicts: Vec::new(),
    };
    if deleted {
        return Ok(plan);
    }
    if created {
        let name = wanted.name.ok_or_else(|| StoreError::Invalid {
            field: "shortlist".into(),
            reason: format!("{source} has no name, so no list can be made for it"),
            valid: Vec::new(),
        })?;
        let (size, actors) = match wanted.record {
            Some(record) => (record.size, record.actors.clone()),
            None => (core::DEFAULT_SIZE, vec![writer.0.name().to_owned()]),
        };
        plan.drafts.push(draft(Change::CreateShortlist));
        plan.drafts.push(draft(set(Field::Name, json!(name))));
        plan.drafts.push(draft(set(Field::Size, json!(size))));
        for who in &actors {
            plan.drafts.push(draft(actor(who, true)));
        }
        plan.written.push("created".into());
        return Ok(plan);
    }
    if by_name {
        plan.drafts.push(draft(Change::ShortlistSource));
    }
    for change in compare(conn, source, wanted, &mut plan)? {
        plan.drafts.push(draft(change));
    }
    Ok(plan)
}

/// The live list `source` names, as its own id or on an earlier import's
/// create or source operation; else the one `name` names in `project`; else
/// a list of the source's deleted here, which is not made again.
fn found(
    conn: &Connection,
    project: ProjectId,
    source: &str,
    name: Option<&str>,
) -> Result<Found, StoreError> {
    let kinds = ("create_shortlist", Some("shortlist_source"));
    let deleted = |conn: &Connection, id| Ok(crate::shortlist::read(conn, id)?.map(|l| l.deleted));
    let unfound = match by_source(conn, source, kinds, deleted)? {
        BySource::Live(id) => return Ok(Found::Source(id)),
        BySource::Gone(id) => Found::Deleted(id),
        BySource::None => Found::None,
    };
    let Some(name) = name else {
        return Ok(unfound);
    };
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM shortlists WHERE project = ?1 AND deleted = 0
           AND name = ?2 COLLATE NOCASE ORDER BY id",
    )?;
    let named = stmt
        .query_map(params![project.to_string(), name], |row| row.get(0))?
        .map(|id: rusqlite::Result<Vec<u8>>| uuid_from(&id?))
        .collect::<Result<Vec<_>, StoreError>>()?;
    match named.as_slice() {
        [] => Ok(unfound),
        [id] => Ok(Found::Name(*id)),
        shared => Err(StoreError::ShortlistNameShared {
            name: name.to_owned(),
            count: shared.len(),
        }),
    }
}

/// The writes that bring a found list's name, size and Actors to the
/// file's where only the source changed them since the last import from it;
/// what both changed is noted as a conflict, keeping tm's. Actors join
/// before any leave, so a list never passes through having none.
fn compare(
    conn: &Connection,
    source: &str,
    wanted: &Wanted<'_>,
    plan: &mut ListPlan,
) -> Result<Vec<Change>, StoreError> {
    let list = plan.list;
    let held = crate::shortlist::read(conn, list)?.ok_or(StoreError::UnknownShortlist(list))?;
    let mut changes = Vec::new();
    // The name it was matched by, in the source's case, is written so a later
    // change from the source is compared from it.
    if plan.by_name
        && let Some(name) = wanted.name
    {
        if held.name.as_deref() != Some(name) {
            plan.written.push(Field::Name.as_str().into());
        }
        changes.push(set(Field::Name, json!(name)));
    }
    let mut single = |field: Field, wanted: Value, tm: Value| -> Result<(), StoreError> {
        match three_way(conn, (list, source, field.as_str()), (&wanted, &tm))? {
            Decision::Keep => {}
            Decision::Write => {
                plan.written.push(field.as_str().into());
                changes.push(set(field, wanted));
            }
            Decision::Conflict => plan.conflicts.push(Conflict {
                field: field.as_str().into(),
                tm,
                source: wanted,
                reason: None,
            }),
        }
        Ok(())
    };
    if let Some(name) = wanted.name
        && !plan.by_name
    {
        single(Field::Name, json!(name), json!(held.name))?;
    }
    let Some(record) = wanted.record else {
        return Ok(changes);
    };
    single(Field::Size, json!(record.size), json!(held.size))?;
    // An Actor the source's own list never named was absent there.
    let made_here = sourced(conn, source, ("create_shortlist", None))?.contains(&list);
    let imported = base(conn, source, Some(list), core::ACTORS)?;
    let wanted: BTreeSet<String> = record.actors.iter().map(|a| core::actor_key(a)).collect();
    let mut all: BTreeSet<String> = wanted.iter().chain(&held.actors).cloned().collect();
    all.extend(imported.keys().map(|(_, who)| who.clone()));
    let mut leaving = Vec::new();
    for who in all {
        let (source_has, tm_has) = (wanted.contains(&who), held.actors.contains(&who));
        let base = match imported.get(&(list, who.clone())) {
            Some(Change::ShortlistActor { present, .. }) => Some(*present),
            _ => made_here.then_some(false),
        };
        let name = format!("{}/{who}", core::ACTORS);
        match decide(&source_has, base.as_ref(), &tm_has) {
            Decision::Keep => {}
            Decision::Write if source_has == tm_has => changes.push(actor(&who, source_has)),
            Decision::Write if source_has => {
                plan.written.push(name);
                changes.push(actor(&who, true));
            }
            Decision::Write => {
                plan.removed.push(name);
                leaving.push(actor(&who, false));
            }
            Decision::Conflict => plan.conflicts.push(Conflict {
                field: name,
                tm: json!(tm_has),
                source: json!(source_has),
                reason: None,
            }),
        }
    }
    changes.extend(leaving);
    Ok(changes)
}

/// What the last import from `source` wrote to `subject`'s `field` (and
/// `elem`): what the source held when it last came in.
fn imported(
    conn: &Connection,
    subject: Uuid,
    source: &str,
    field: &str,
    elem: Option<&str>,
) -> Result<Option<Change>, StoreError> {
    let mut winners = base(conn, source, Some(subject), field)?;
    Ok(winners.remove(&(subject, elem.unwrap_or_default().to_owned())))
}

fn set(field: Field, value: Value) -> Change {
    Change::SetShortlist { field, value }
}

fn actor(who: &str, present: bool) -> Change {
    Change::ShortlistActor {
        actor: core::actor_key(who),
        present,
    }
}
