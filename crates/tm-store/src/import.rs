//! Record import (ADR 0013): `issue` records written as ordinary, validated
//! writes, each operation naming its source. Pass one creates or updates each
//! Issue's fields and comments; pass two sets parents and Links once every
//! target exists. Each record is one batch per pass.
//!
//! A re-import compares each field three ways: what the last import from the
//! record's source wrote (the winner among the operations carrying it), the
//! record now, and tm now. A field changed only in the source is written; one
//! changed in tm keeps tm's value, and is a conflict if the source changed it
//! too.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::BufRead;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tm_core::op::COMMENTS_FIELD;
use tm_core::scheme::{FieldType, WAITING_ON};
use tm_core::{Actor, Change, Op, ProjectId, Scheme, link};
use uuid::Uuid;

use crate::apply::{Version, issue_project, uuid_from};
use crate::export::{FORMAT, VERSION, unreadable};
use crate::import_list::{ListOutcome, place_of};
use crate::read::{self, IssueView};
use crate::record::{CommentRecord, IssueRecord, ListRecord, TM_SOURCE};
use crate::{Draft, Provenance, Store, StoreError, WallTime};

/// Fields a record holds one value of, read from the Issue's own columns.
const SCALARS: [&str; 7] = [
    "type",
    "title",
    "description",
    "status",
    "priority",
    "assignee",
    "archived",
];

/// Fields an import writes that are compared elsewhere, or not by value.
const NOT_VALUES: [&str; 6] = [
    "parent",
    link::FIELD,
    "comments",
    "claim",
    "shortlist",
    "shortlist_rank",
];

/// What an import did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordsImported {
    /// The keys of the Projects it created.
    pub projects: Vec<String>,
    /// Each Short-list the file names, as the import found or created it.
    pub shortlists: Vec<ListOutcome>,
    pub records: Vec<RecordOutcome>,
}

/// What an import did with one record.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordOutcome {
    pub line: usize,
    pub source: String,
    pub issue: Uuid,
    pub created: bool,
    /// The batches it wrote, which a bare undo skips; `tm undo <batch>`
    /// reverts one.
    pub batches: Vec<Uuid>,
    /// Each field set, or `field/element` for a set member, a Link or a
    /// comment added.
    pub written: Vec<String>,
    /// Each set member or Link removed, or comment retracted, as
    /// `field/element`.
    pub removed: Vec<String>,
    /// Fields both tm and the source changed; tm's value was kept.
    pub conflicts: Vec<Conflict>,
    /// Comments whose words changed at the source, which are not rewritten.
    pub changed_comments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conflict {
    pub field: String,
    pub tm: Value,
    pub source: Value,
    /// Why tm's value stands when it is not that tm changed it too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
}

/// A Project the file names, created when no Project holds its key, with
/// the Short-lists it holds.
#[derive(Debug, Deserialize)]
pub(crate) struct ProjectLine {
    pub(crate) key: String,
    name: String,
    #[serde(default)]
    pub(crate) shortlists: Vec<ListRecord>,
}

/// A file's lines, each with its line number.
pub(crate) struct Parsed {
    pub(crate) projects: Vec<(usize, ProjectLine)>,
    pub(crate) issues: Vec<(usize, IssueRecord)>,
}

impl Parsed {
    pub(crate) const fn issues(issues: Vec<(usize, IssueRecord)>) -> Self {
        Self {
            projects: Vec::new(),
            issues,
        }
    }
}

/// The writes one record needs in one pass, and what they mean.
#[derive(Debug, Default, PartialEq)]
struct Plan {
    drafts: Vec<(Draft, Provenance)>,
    written: Vec<String>,
    removed: Vec<String>,
    conflicts: Vec<Conflict>,
    changed_comments: Vec<String>,
}

impl Plan {
    fn push(&mut self, draft: Draft, source: &str, at: WallTime) {
        let provenance = Provenance {
            source: source.to_owned(),
            at,
        };
        self.drafts.push((draft, provenance));
    }

    /// Records a write as an addition or a removal.
    fn note(&mut self, name: String, added: bool) {
        if added {
            self.written.push(name);
        } else {
            self.removed.push(name);
        }
    }
}

/// Who and when a record's writes are: its `updated_by`, else for a new
/// Issue its reporter, else the runner; at its `updated`, else for a new
/// Issue its `created`, else now.
struct Writer {
    actor: Actor,
    at: WallTime,
}

impl Writer {
    fn of(
        record: &IssueRecord,
        created: bool,
        runner: &Actor,
        now: WallTime,
    ) -> Result<Self, StoreError> {
        let name = record
            .updated_by
            .as_deref()
            .or(record.reporter.as_deref().filter(|_| created));
        let fallback = if created {
            time("created", record.created.as_deref(), now)?
        } else {
            now
        };
        Ok(Self {
            actor: person(name, runner),
            at: time("updated", record.updated.as_deref(), fallback)?,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    Keep,
    Write,
    Conflict,
}

/// `source` is the record now, `base` what the last import wrote, `tm` the
/// Issue now. A source that moved to tm's value is still written, so the next
/// comparison starts from it. With no base, as for an Issue an import found
/// by its `tm:` id rather than created, any difference is a conflict, so
/// nothing tm wrote is overwritten.
pub(crate) fn decide<T: PartialEq>(source: &T, base: Option<&T>, tm: &T) -> Decision {
    match base {
        None if source == tm => Decision::Keep,
        Some(base) if source == base => Decision::Keep,
        Some(base) if tm == base || tm == source => Decision::Write,
        _ => Decision::Conflict,
    }
}

impl Store {
    /// Writes the `issue` records in `input` (ADR 0013), creating any Project
    /// a `project` line names that no Project holds. A tm export's header and
    /// operations are skipped, whatever their operation format. `runner` writes
    /// what a record names no person for.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvalidExport`] for a line that is not a record, and
    /// [`StoreError::RecordRefused`] for a record wrong on its own, before
    /// anything is written; `RecordRefused` too for a record a write refuses,
    /// after the records before it were written.
    pub fn import_records(
        &mut self,
        input: &mut dyn BufRead,
        runner: &Actor,
    ) -> Result<RecordsImported, StoreError> {
        let parsed = parse(input)?;
        self.write_records(&parsed, runner)
    }

    /// Writes records already parsed, as [`Store::import_records`] does.
    pub(crate) fn write_records(
        &mut self,
        parsed: &Parsed,
        runner: &Actor,
    ) -> Result<RecordsImported, StoreError> {
        for (line, record) in &parsed.issues {
            check_record(&self.scheme, record)
                .map_err(|cause| refused(*line, &record.source, cause, (&[], &[])))?;
        }
        crate::import_list::check_lists(parsed)?;
        let (now, _) = self.now()?;
        let (mut projects, mut project_batches) = (Vec::new(), Vec::new());
        for (line, project) in &parsed.projects {
            if self.project_by_key(&project.key)?.is_none() {
                let (_, op) = self
                    .create_project(runner, &project.key, &project.name)
                    .map_err(|cause| {
                        refused(*line, &project.key, cause, (&project_batches, &[]))
                    })?;
                projects.push(project.key.to_ascii_uppercase());
                project_batches.push(op.op_id);
            }
        }
        let shortlists = self.import_lists(parsed, runner, now, &mut project_batches)?;
        let mut records: Vec<RecordOutcome> = Vec::with_capacity(parsed.issues.len());
        let mut cleared = Vec::new();
        for (line, record) in &parsed.issues {
            let (outcome, left) =
                self.import_fields(*line, record, runner, now)
                    .map_err(|cause| {
                        refused(*line, &record.source, cause, (&project_batches, &records))
                    })?;
            cleared.extend(left.map(|list| (records.len(), list)));
            records.push(outcome);
        }
        // A Link one record still lists is kept when another drops it.
        let mut listed = BTreeSet::new();
        for (outcome, (line, record)) in records.iter().zip(&parsed.issues) {
            listed.extend(
                wanted_links(&self.conn, record, outcome.issue).map_err(|cause| {
                    refused(*line, &record.source, cause, (&project_batches, &records))
                })?,
            );
        }
        // Every detachment first, so a parent or Link that moves between two
        // records never meets the cycle check half moved.
        for detach in [true, false] {
            for (index, (line, record)) in parsed.issues.iter().enumerate() {
                let outcome = &mut records[index];
                if let Err(cause) =
                    self.import_relations(record, outcome, &listed, detach, (runner, now))
                {
                    let done = (project_batches.as_slice(), records.as_slice());
                    return Err(refused(*line, &record.source, cause, done));
                }
            }
        }
        let mut shortlists = shortlists;
        self.import_places(
            parsed,
            (&mut records, &mut shortlists, &cleared),
            (runner, now),
            &mut project_batches,
        )?;
        Ok(RecordsImported {
            projects,
            shortlists,
            records,
        })
    }

    /// Pass one: the Issue's fields, sets and comments, with the list its
    /// finish took it off.
    fn import_fields(
        &mut self,
        line: usize,
        record: &IssueRecord,
        runner: &Actor,
        now: WallTime,
    ) -> Result<(RecordOutcome, Option<Uuid>), StoreError> {
        let new = Uuid::now_v7();
        let scheme = self.scheme.clone();
        let plan = |conn: &Connection| -> Result<(Uuid, bool, Plan), StoreError> {
            Ok(match find(conn, &record.source)? {
                Some(issue) => (
                    issue,
                    false,
                    update_plan(conn, &scheme, record, issue, runner, now)?,
                ),
                None => (
                    new,
                    true,
                    create_plan(conn, &scheme, record, new, runner, now)?,
                ),
            })
        };
        let (issue, created, mut planned) = plan(&self.conn)?;
        // Read inside the write, so another writer's move is not taken for
        // the finish's.
        let held = Cell::new(None);
        let ops = self
            .write_plan(&planned, issue, |conn| {
                held.set(place_of(conn, issue)?);
                plan(conn).map(|(again, _, plan)| (again == issue).then_some(plan))
            })
            .map_err(|cause| {
                if created {
                    cause.sourced(issue, &record.source)
                } else {
                    cause
                }
            })?;
        let cleared = held.get().filter(|_| {
            ops.iter().any(|op| {
                op.subject == issue
                    && matches!(op.change(), Ok(Change::SetField { field, .. }) if field == "shortlist")
            })
        });
        if cleared.is_some() {
            planned.removed.push("shortlist".into());
        }
        let outcome = RecordOutcome {
            line,
            source: record.source.clone(),
            issue,
            created,
            batches: ops.first().map(|op| op.op_id).into_iter().collect(),
            written: planned.written,
            removed: planned.removed,
            conflicts: planned.conflicts,
            changed_comments: planned.changed_comments,
        };
        Ok((outcome, cleared))
    }

    /// Pass two: the Issue's parent and Links, whose targets now exist; only
    /// what they lose when `detach`.
    fn import_relations(
        &mut self,
        record: &IssueRecord,
        outcome: &mut RecordOutcome,
        listed: &BTreeSet<(Uuid, String)>,
        detach: bool,
        (runner, now): (&Actor, WallTime),
    ) -> Result<(), StoreError> {
        let (issue, created) = (outcome.issue, outcome.created);
        let scheme = self.scheme.clone();
        let plan = |conn: &Connection| {
            let writer = Writer::of(record, created, runner, now)?;
            let target = Relations {
                issue,
                listed,
                detach,
            };
            relations_plan(conn, &scheme, record, &target, &writer)
        };
        let planned = plan(&self.conn)?;
        let ops = self.write_plan(&planned, issue, |conn| plan(conn).map(Some))?;
        outcome.batches.extend(ops.first().map(|op| op.op_id));
        outcome.written.extend(planned.written);
        outcome.removed.extend(planned.removed);
        outcome.conflicts.extend(planned.conflicts);
        Ok(())
    }

    /// Writes a plan as one batch, once the same plan comes out of the write
    /// transaction, so a write made between reading and writing is never
    /// overwritten; returns the batch's operations, the first naming it.
    fn write_plan(
        &mut self,
        planned: &Plan,
        issue: Uuid,
        again: impl Fn(&Connection) -> Result<Option<Plan>, StoreError>,
    ) -> Result<Vec<Op>, StoreError> {
        if planned.drafts.is_empty() {
            return Ok(Vec::new());
        }
        self.append_imported_guarded(planned.drafts.clone(), |tx| {
            if again(tx)?.as_ref() == Some(planned) {
                Ok(())
            } else {
                Err(StoreError::ImportChanged(issue.into()))
            }
        })
    }
}

/// `done` is the Projects' batches and the records' outcomes written so far.
/// An Issue the import created is named by its record, so a refusal reads the
/// same whether the import ran or was a dry run.
pub(crate) fn refused(
    line: usize,
    record: &str,
    cause: StoreError,
    (projects, records): (&[Uuid], &[RecordOutcome]),
) -> StoreError {
    let cause = records
        .iter()
        .filter(|outcome| outcome.created)
        .fold(cause, |cause, outcome| {
            cause.sourced(outcome.issue, &outcome.source)
        });
    let records = records.iter().flat_map(|outcome| outcome.batches.iter());
    // A place batch is both a list's and each record's it placed.
    let mut seen = HashSet::new();
    StoreError::RecordRefused {
        line,
        record: record.to_owned(),
        cause: Box::new(cause),
        written: projects
            .iter()
            .chain(records)
            .copied()
            .filter(|batch| seen.insert(*batch))
            .collect(),
    }
}

/// What a refused import leaves written, for its message.
pub(crate) fn kept(written: &[Uuid]) -> String {
    if written.is_empty() {
        "nothing was written; fix the line and import the file again".into()
    } else {
        let batches: Vec<String> = written.iter().map(Uuid::to_string).collect();
        format!(
            "batches {} were written before it and stay; fix the line and import the file again",
            batches.join(", ")
        )
    }
}

/// The records in `input`, each checked as one before anything is written.
fn parse(input: &mut dyn BufRead) -> Result<Parsed, StoreError> {
    let invalid = |line: usize, reason: String| StoreError::InvalidExport { line, reason };
    let mut parsed = Parsed {
        projects: Vec::new(),
        issues: Vec::new(),
    };
    let mut sources: HashMap<String, usize> = HashMap::new();
    for (index, text) in input.lines().enumerate() {
        let line = index + 1;
        let text = text.map_err(StoreError::Io)?;
        if text.trim().is_empty() {
            continue;
        }
        let mut value: Value =
            serde_json::from_str(&text).map_err(|err| invalid(line, unreadable(&err)))?;
        let kind = value
            .as_object_mut()
            .and_then(|object| object.remove("_type"))
            .and_then(|kind| kind.as_str().map(str::to_owned))
            .ok_or_else(|| invalid(line, "the line has no _type".into()))?;
        let unfit = |err: serde_json::Error| invalid(line, unreadable(&err));
        match kind.as_str() {
            "header" if line == 1 => {
                let (format, version) = (value["format"].as_str(), value["version"].as_u64());
                if format != Some(FORMAT) || version != Some(u64::from(VERSION)) {
                    return Err(invalid(
                        line,
                        format!("this is not a {FORMAT} version {VERSION} file"),
                    ));
                }
            }
            "header" => return Err(invalid(line, "a header after the first line".into())),
            // Another operation format's lines need not parse as this one's.
            "op" => {}
            "project" => {
                let project: ProjectLine = serde_json::from_value(value).map_err(unfit)?;
                parsed.projects.push((line, project));
            }
            "issue" => {
                let record: IssueRecord = serde_json::from_value(value).map_err(unfit)?;
                if record.source.trim().is_empty() {
                    return Err(invalid(line, "the record's source is blank".into()));
                }
                if let Some(first) = sources.insert(record.source.clone(), line) {
                    return Err(invalid(
                        line,
                        format!("source {} is also line {first}'s", record.source),
                    ));
                }
                if let Some(id) = repeated_comment(&record) {
                    return Err(invalid(line, format!("comment id {id} appears twice")));
                }
                parsed.issues.push((line, record));
            }
            other => return Err(invalid(line, format!("{other:?} is not a record type"))),
        }
    }
    Ok(parsed)
}

/// A comment id the record gives twice, which a re-import could not tell
/// apart.
pub(crate) fn repeated_comment(record: &IssueRecord) -> Option<&str> {
    let mut ids = HashSet::new();
    record
        .comments
        .iter()
        .find(|comment| !ids.insert(&comment.id))
        .map(|comment| comment.id.as_str())
}

/// What a record gets wrong on its own, refused before anything is written.
fn check_record(scheme: &Scheme, record: &IssueRecord) -> Result<(), StoreError> {
    for (name, value) in &record.fields {
        let invalid = |reason: &str| StoreError::Invalid {
            field: name.clone(),
            reason: reason.to_owned(),
            valid: Vec::new(),
        };
        if scheme.field(name).is_some_and(|def| def.builtin) {
            return Err(invalid("is a built-in field, written outside `fields`"));
        }
        if name == WAITING_ON {
            return Err(invalid(
                "is set with its question by tm ask, not by an import",
            ));
        }
        let strings = value
            .as_array()
            .is_some_and(|elems| elems.iter().all(Value::is_string));
        if is_set(scheme, name) && !value.is_null() && !strings {
            return Err(invalid(
                "holds several values, written as a list of strings",
            ));
        }
    }
    for field in set_fields(scheme, record) {
        wanted_set(scheme, record, &field)?;
    }
    for record_link in &record.links {
        link_def(&record_link.link_type)?;
    }
    Ok(())
}

fn link_def(link_type: &str) -> Result<&'static link::LinkType, StoreError> {
    link::link_type(link_type).ok_or_else(|| StoreError::Invalid {
        field: link::FIELD.into(),
        reason: format!("{link_type:?} is not a Link type"),
        valid: link::BUILT_IN.iter().map(|def| def.id.to_owned()).collect(),
    })
}

/// The Issue whose `create` an import from `source` wrote.
fn created_by(conn: &Connection, source: &str) -> Result<Option<Uuid>, StoreError> {
    let created: Option<Vec<u8>> = conn
        .query_row(
            "SELECT issue_id FROM ops WHERE source = ?1 AND kind = 'create' ORDER BY seq LIMIT 1",
            [source],
            |row| row.get(0),
        )
        .optional()?;
    created.map(|id| uuid_from(&id)).transpose()
}

/// The Issue an import from `source` created; for a tm export's own source,
/// the Issue it names, which a re-import of an older export of this
/// Workspace finds without any source.
fn find(conn: &Connection, source: &str) -> Result<Option<Uuid>, StoreError> {
    if let Some(issue) = created_by(conn, source)? {
        return Ok(Some(issue));
    }
    match source.strip_prefix(TM_SOURCE).map(Uuid::parse_str) {
        Some(Ok(id)) if issue_project(conn, id)?.is_some() => Ok(Some(id)),
        _ => Ok(None),
    }
}

/// The Issue a reference names: as a source, then a Key, then an Issue id.
fn resolve(conn: &Connection, field: &str, reference: &str) -> Result<Uuid, StoreError> {
    if let Some(issue) = find(conn, reference)? {
        return Ok(issue);
    }
    let upper = reference.to_ascii_uppercase();
    if let Some((prefix, number)) = tm_core::project::parse_issue_key(&upper)
        && let Some(project) = crate::project::key_holder(conn, prefix)?
        && let Some(issue) = crate::project::issue_by_number(conn, project, number)?
    {
        return Ok(issue);
    }
    match Uuid::parse_str(reference) {
        Ok(id) if issue_project(conn, id)?.is_some() => Ok(id),
        _ => Err(StoreError::Invalid {
            field: field.to_owned(),
            reason: format!("{reference} names no Issue"),
            valid: Vec::new(),
        }),
    }
}

/// A record's time, or `fallback` when it has none.
fn time(field: &str, at: Option<&str>, fallback: WallTime) -> Result<WallTime, StoreError> {
    let Some(at) = at else { return Ok(fallback) };
    let instant: jiff::Timestamp = at.parse().map_err(|err| StoreError::Invalid {
        field: field.to_owned(),
        reason: format!("{at:?} is not an RFC 3339 time: {err}"),
        valid: Vec::new(),
    })?;
    Ok(WallTime {
        utc_ms: instant.as_millisecond(),
        offset_min: 0,
    })
}

fn person(name: Option<&str>, runner: &Actor) -> Actor {
    name.map_or_else(|| runner.clone(), Actor::human)
}

fn comment_actor(comment: &CommentRecord) -> Actor {
    comment.on_behalf_of.as_deref().map_or_else(
        || Actor::human(&comment.actor),
        |human| Actor::agent(&comment.actor, human),
    )
}

fn comment_source(record: &IssueRecord, comment: &CommentRecord) -> String {
    format!("{}#{}", record.source, comment.id)
}

/// The value a record gives a single-valued field, as [`normal`] reads it.
fn wanted(record: &IssueRecord, field: &str) -> Value {
    let text = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
    let value = match field {
        "type" => text(&record.issue_type),
        "title" => text(&record.title),
        "description" => text(&record.description),
        "status" => text(&record.status),
        "priority" => text(&record.priority),
        "assignee" => text(&record.assignee),
        "archived" => Value::Bool(record.archived),
        other => record.fields.get(other).cloned().unwrap_or(Value::Null),
    };
    normal(field, value)
}

/// A field's value as an Issue reads it: unset priority as `none`, unset
/// archived as false, and a whole number as an integer, however written.
#[allow(clippy::cast_possible_truncation)]
fn normal(field: &str, value: Value) -> Value {
    match (field, value) {
        ("priority", Value::Null) => json!("none"),
        ("archived", Value::Null) => json!(false),
        (_, Value::Number(number)) => match number.as_f64() {
            Some(float) if float.fract() == 0.0 && float.abs() <= 9_007_199_254_740_992.0 => {
                json!(float as i64)
            }
            _ => Value::Number(number),
        },
        (_, value) => value,
    }
}

fn is_set(scheme: &Scheme, field: &str) -> bool {
    field == "labels"
        || scheme
            .field(field)
            .is_some_and(|def| def.field_type.is_multi())
}

/// The single-valued fields a record speaks for: the built-in ones, then its
/// Scheme fields that are not sets.
fn single_fields(scheme: &Scheme, record: &IssueRecord) -> Vec<String> {
    let custom = record.fields.keys().filter(|field| !is_set(scheme, field));
    SCALARS
        .iter()
        .map(|field| (*field).to_owned())
        .chain(custom.cloned())
        .collect()
}

fn set_fields(scheme: &Scheme, record: &IssueRecord) -> Vec<String> {
    let custom = record.fields.keys().filter(|field| is_set(scheme, field));
    std::iter::once("labels".to_owned())
        .chain(custom.cloned())
        .collect()
}

/// The elements a record gives a set field, keyed as the store keys them,
/// each with the spelling to write; two spellings of one label are refused.
fn wanted_set(
    scheme: &Scheme,
    record: &IssueRecord,
    field: &str,
) -> Result<BTreeMap<String, String>, StoreError> {
    let elems: Vec<String> = if field == "labels" {
        record.labels.clone()
    } else {
        record
            .fields
            .get(field)
            .and_then(Value::as_array)
            .map(|elems| {
                elems
                    .iter()
                    .filter_map(|elem| elem.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let labels = field == "labels"
        || scheme
            .field(field)
            .is_some_and(|def| def.field_type == FieldType::Labels);
    let mut set = BTreeMap::new();
    for elem in elems {
        let key = if labels {
            elem.trim().to_lowercase()
        } else {
            elem.clone()
        };
        if let Some(first) = set.insert(key, elem.clone()) {
            return Err(StoreError::Invalid {
                field: field.to_owned(),
                reason: format!("lists {first:?} and {elem:?}, which are one element"),
                valid: Vec::new(),
            });
        }
    }
    Ok(set)
}

fn set(field: &str, value: Value) -> Change {
    Change::SetField {
        field: field.to_owned(),
        label: String::new(),
        value,
        value_label: None,
    }
}

fn member(field: &str, elem: &str, present: bool) -> Change {
    let (field, label, elem) = (field.to_owned(), String::new(), elem.to_owned());
    if present {
        Change::Add {
            field,
            label,
            elem,
            elem_label: None,
        }
    } else {
        Change::Remove {
            field,
            label,
            elem,
            elem_label: None,
        }
    }
}

/// What the last import from `source` wrote to each `(Issue, element)` of
/// `field`, folded by the merge rule in replay order; a single value's
/// element is empty.
pub(crate) fn base(
    conn: &Connection,
    source: &str,
    issue: Option<Uuid>,
    field: &str,
) -> Result<HashMap<(Uuid, String), Change>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT issue_id, coalesce(elem, ''), hlc, replica_id, op_id, base_seq, server_seq,
                server_received_ms, payload
         FROM ops
         WHERE source = ?1 AND field = ?2 AND (?3 IS NULL OR issue_id = ?3)
           AND op_id NOT IN (SELECT op_id FROM unapplied)
         ORDER BY server_seq IS NULL, server_seq, hlc, replica_id, op_id",
    )?;
    let issue = issue.map(|id| id.as_bytes().to_vec());
    let mut rows = stmt.query(params![source, field, issue])?;
    let mut winners: HashMap<(Uuid, String), (Version, Change)> = HashMap::new();
    while let Some(row) = rows.next()? {
        let key = (uuid_from(&row.get::<_, Vec<u8>>(0)?)?, row.get(1)?);
        let version = Version::from_row(row, 2)?;
        let payload: String = row.get(8)?;
        let change: Change = serde_json::from_str(&payload)
            .map_err(|err| StoreError::Corrupt(format!("a logged payload: {err}")))?;
        if winners
            .get(&key)
            .is_none_or(|(best, _)| version.beats(best))
        {
            winners.insert(key, (version, change));
        }
    }
    Ok(winners
        .into_iter()
        .map(|(key, (_, change))| (key, change))
        .collect())
}

/// What an import last wrote to a single value; with none, unset, or for an
/// adopted Issue, no base at all.
fn base_value(base: &HashMap<(Uuid, String), Change>, issue: Uuid, adopted: bool) -> Option<Value> {
    match base.get(&(issue, String::new())) {
        Some(Change::SetField { value, .. }) => Some(value.clone()),
        _ if adopted => None,
        _ => Some(Value::Null),
    }
}

/// Whether an import last added an element; with no write, absent, or for an
/// adopted Issue, no base at all.
fn base_member(
    base: &HashMap<(Uuid, String), Change>,
    key: &(Uuid, String),
    adopted: bool,
) -> Option<bool> {
    match base.get(key) {
        Some(change) => Some(matches!(
            change,
            Change::Add { .. } | Change::Link { present: true, .. }
        )),
        None if adopted => None,
        None => Some(false),
    }
}

/// Whether the record's Issue is one this import's source did not create,
/// found instead by its `tm:` id.
pub(crate) fn adopted(conn: &Connection, source: &str, issue: Uuid) -> Result<bool, StoreError> {
    Ok(created_by(conn, source)? != Some(issue))
}

fn view(conn: &Connection, scheme: &Scheme, issue: Uuid) -> Result<IssueView, StoreError> {
    read::issues(conn, scheme, &[issue])?
        .pop()
        .ok_or(StoreError::UnknownIssue(issue))
}

/// Every field an import from `source` has written on `issue`.
fn imported_fields(
    conn: &Connection,
    source: &str,
    issue: Uuid,
) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT field FROM ops
         WHERE source = ?1 AND issue_id = ?2 AND field IS NOT NULL ORDER BY field",
    )?;
    let rows = stmt.query_map(params![source, issue.as_bytes()], |row| row.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The Issue as the record has it, with its comments.
fn create_plan(
    conn: &Connection,
    scheme: &Scheme,
    record: &IssueRecord,
    issue: Uuid,
    runner: &Actor,
    now: WallTime,
) -> Result<Plan, StoreError> {
    let project = crate::project::key_holder(conn, &record.project.to_ascii_uppercase())?
        .ok_or_else(|| StoreError::Invalid {
            field: "project".into(),
            reason: format!("no Project holds key {}", record.project),
            valid: Vec::new(),
        })?;
    let created = time("created", record.created.as_deref(), now)?;
    let writer = Writer::of(record, true, runner, now)?;
    let draft = |actor: &Actor, change| Draft {
        actor: actor.clone(),
        project,
        subject: issue,
        change,
    };
    let source = record.source.as_str();
    let mut plan = Plan::default();
    let reporter = person(record.reporter.as_deref(), runner);
    plan.push(draft(&reporter, Change::Create), source, created);
    for field in single_fields(scheme, record) {
        let value = wanted(record, &field);
        if value != normal(&field, Value::Null) {
            plan.push(draft(&writer.actor, set(&field, value)), source, writer.at);
            plan.written.push(field);
        }
    }
    for field in set_fields(scheme, record) {
        for (key, elem) in wanted_set(scheme, record, &field)? {
            let change = member(&field, &elem, true);
            plan.push(draft(&writer.actor, change), source, writer.at);
            plan.written.push(format!("{field}/{key}"));
        }
    }
    comments_plan(conn, record, (project, issue), writer.at, now, &mut plan)?;
    Ok(plan)
}

/// The Issue's fields, sets and comments compared three ways.
fn update_plan(
    conn: &Connection,
    scheme: &Scheme,
    record: &IssueRecord,
    issue: Uuid,
    runner: &Actor,
    now: WallTime,
) -> Result<Plan, StoreError> {
    let current = view(conn, scheme, issue)?;
    let project = current.project;
    let writer = Writer::of(record, false, runner, now)?;
    let source = record.source.as_str();
    let adopted = adopted(conn, source, issue)?;
    let draft = |change| Draft {
        actor: writer.actor.clone(),
        project,
        subject: issue,
        change,
    };
    let mut plan = Plan::default();

    // A field the record dropped still compares, as unset.
    let mut singles = single_fields(scheme, record);
    let mut sets = set_fields(scheme, record);
    for field in imported_fields(conn, source, issue)? {
        let list = if is_set(scheme, &field) {
            &mut sets
        } else {
            &mut singles
        };
        if !list.contains(&field) && !NOT_VALUES.contains(&field.as_str()) {
            list.push(field);
        }
    }

    for field in singles {
        let wanted = wanted(record, &field);
        let base = base_value(&base(conn, source, Some(issue), &field)?, issue, adopted)
            .map(|value| normal(&field, value));
        let tm = normal(
            &field,
            current.fields.get(&field).cloned().unwrap_or(Value::Null),
        );
        match decide(&wanted, base.as_ref(), &tm) {
            Decision::Keep => {}
            Decision::Write => {
                plan.push(draft(set(&field, wanted)), source, writer.at);
                plan.written.push(field);
            }
            Decision::Conflict => plan.conflicts.push(Conflict {
                field,
                tm,
                source: wanted,
                reason: None,
            }),
        }
    }

    for field in sets {
        let wanted = wanted_set(scheme, record, &field)?;
        let base = base(conn, source, Some(issue), &field)?;
        let tm: BTreeSet<&String> = current
            .sets
            .get(&field)
            .map(|set| set.keys().collect())
            .unwrap_or_default();
        // tm's own members too, so an adopted Issue reports what tm added.
        let keys: BTreeSet<String> = wanted
            .keys()
            .cloned()
            .chain(base.keys().map(|(_, elem)| elem.clone()))
            .chain(tm.iter().map(|elem| (*elem).clone()))
            .collect();
        for key in keys {
            let want = wanted.contains_key(&key);
            let had = base_member(&base, &(issue, key.clone()), adopted);
            let has = tm.contains(&key);
            let name = format!("{field}/{key}");
            match decide(&want, had.as_ref(), &has) {
                Decision::Keep => {}
                Decision::Write => {
                    let elem = wanted.get(&key).unwrap_or(&key);
                    plan.push(draft(member(&field, elem, want)), source, writer.at);
                    plan.note(name, want);
                }
                Decision::Conflict => plan.conflicts.push(Conflict {
                    field: name,
                    tm: json!(has),
                    source: json!(want),
                    reason: None,
                }),
            }
        }
    }

    comments_plan(conn, record, (project, issue), writer.at, now, &mut plan)?;
    Ok(plan)
}

/// A comment new at the source is added at its own time, or `at`; one
/// already held is left, its changed words only reported; one gone from the
/// source is retracted as its own Actor at `now`, unless tm has shown or
/// hidden it since; one back at the source that the import hid is shown.
fn comments_plan(
    conn: &Connection,
    record: &IssueRecord,
    (project, issue): (ProjectId, Uuid),
    at: WallTime,
    now: WallTime,
    plan: &mut Plan,
) -> Result<(), StoreError> {
    let draft = |actor: Actor, change| Draft {
        actor,
        project,
        subject: issue,
        change,
    };
    let mut held = BTreeSet::new();
    for comment in &record.comments {
        let source = comment_source(record, comment);
        if let Some((id, body, retracted)) = held_comment(conn, record, issue, comment, &source)? {
            held.insert(id);
            if body != comment.body {
                plan.changed_comments.push(comment.id.clone());
            }
            let name = format!("comments/{}", comment.id);
            // Back at the source: shown again if the import hid it; one tm hid
            // stays hidden, as a field changed only in tm keeps tm's value.
            if retracted && retraction_source(conn, issue, id)?.as_deref() == Some(&source) {
                let change = Change::Retract {
                    comment: id,
                    retracted: false,
                };
                plan.push(draft(comment_author(conn, id)?, change), &source, now);
                plan.written.push(name);
            }
            continue;
        }
        let written = time("comments", comment.at.as_deref(), at)?;
        let change = Change::Comment {
            body: comment.body.clone(),
        };
        plan.push(draft(comment_actor(comment), change), &source, written);
        plan.written.push(format!("comments/{}", comment.id));
    }

    let prefix = format!("{}#", record.source);
    let gone = {
        let mut stmt = conn.prepare_cached(
            "SELECT op_id, source FROM ops
             WHERE issue_id = ?1 AND kind = 'comment' AND substr(source, 1, length(?2)) = ?2
             ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![issue.as_bytes(), prefix], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (id, source) in gone {
        let id = uuid_from(&id)?;
        // Hidden already, or shown or hidden by tm since, which stands.
        let decided = retraction_source(conn, issue, id)?;
        if held.contains(&id)
            || is_retracted(conn, id)?
            || decided.is_some_and(|decided| decided != source)
        {
            continue;
        }
        let change = Change::Retract {
            comment: id,
            retracted: true,
        };
        plan.push(draft(comment_author(conn, id)?, change), &source, now);
        let at_source = source.strip_prefix(&prefix).unwrap_or(&source);
        plan.removed.push(format!("comments/{at_source}"));
    }
    Ok(())
}

/// The source of the retraction or showing that decides whether a comment is
/// shown, by the merge rule; empty when it carried none, and none when no
/// such write applied.
fn retraction_source(
    conn: &Connection,
    issue: Uuid,
    comment: Uuid,
) -> Result<Option<String>, StoreError> {
    let source: Option<Option<String>> = conn
        .query_row(
            "SELECT o.source FROM field_versions v JOIN ops o ON o.op_id = v.op_id
             WHERE v.issue_id = ?1 AND v.field = ?2 AND v.elem = ?3",
            params![issue.as_bytes(), COMMENTS_FIELD, comment.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(source.map(Option::unwrap_or_default))
}

fn is_retracted(conn: &Connection, comment: Uuid) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT retracted FROM comments WHERE op_id = ?1",
        [comment.as_bytes()],
        |row| row.get(0),
    )?)
}

/// The Actor who wrote comment `comment`, who alone may retract it.
fn comment_author(conn: &Connection, comment: Uuid) -> Result<Actor, StoreError> {
    let (name, human): (String, Option<String>) = conn.query_row(
        "SELECT actor, on_behalf_of FROM ops WHERE op_id = ?1",
        [comment.as_bytes()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(human.map_or_else(|| Actor::human(&name), |human| Actor::agent(&name, &human)))
}

/// The comment already on the Issue for `comment`, with its words and whether
/// it is retracted: the one carrying its source, or for a tm export of this
/// Workspace, the comment its id names.
fn held_comment(
    conn: &Connection,
    record: &IssueRecord,
    issue: Uuid,
    comment: &CommentRecord,
    source: &str,
) -> Result<Option<(Uuid, String, bool)>, StoreError> {
    let by_source: Option<Vec<u8>> = conn
        .query_row(
            "SELECT op_id FROM ops WHERE source = ?1 AND kind = 'comment' AND issue_id = ?2",
            params![source, issue.as_bytes()],
            |row| row.get(0),
        )
        .optional()?;
    let id = match by_source {
        Some(id) => uuid_from(&id)?,
        None if record.source == format!("{TM_SOURCE}{issue}") => {
            match Uuid::parse_str(&comment.id) {
                Ok(id) => id,
                Err(_) => return Ok(None),
            }
        }
        None => return Ok(None),
    };
    let held: Option<(String, bool)> = conn
        .query_row(
            "SELECT body, retracted FROM comments WHERE issue_id = ?1 AND op_id = ?2",
            params![issue.as_bytes(), id.as_bytes()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(held.map(|(body, retracted)| (id, body, retracted)))
}

/// The record's Links, keyed as each is filed: under its canonical source,
/// which may be the other Issue.
fn wanted_links(
    conn: &Connection,
    record: &IssueRecord,
    issue: Uuid,
) -> Result<BTreeSet<(Uuid, String)>, StoreError> {
    let mut wanted = BTreeSet::new();
    for record_link in &record.links {
        let def = link_def(&record_link.link_type)?;
        let target = resolve(conn, link::FIELD, &record_link.target)?;
        let (from, to) = link::canonical(def, issue, target);
        wanted.insert((from, link::elem(def.id, to)));
    }
    Ok(wanted)
}

/// Which relations pass two compares, and how much of them it writes.
struct Relations<'a> {
    issue: Uuid,
    /// Every Link the file lists; one a record dropped that another lists is
    /// left to that one.
    listed: &'a BTreeSet<(Uuid, String)>,
    /// Only what the Issue loses, written before anything it gains.
    detach: bool,
}

/// The Links filed under `issue`: its outward ones, and symmetric ones whose
/// canonical source is the other Issue.
fn held_links(conn: &Connection, issue: Uuid) -> Result<BTreeSet<(Uuid, String)>, StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT type, src, dst FROM links WHERE src = ?1 OR dst = ?1")?;
    let rows = stmt.query_map([issue.as_bytes()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    let mut held = BTreeSet::new();
    for row in rows {
        let (link_type, src, dst) = row?;
        let (src, dst) = (uuid_from(&src)?, uuid_from(&dst)?);
        let symmetric = link::link_type(&link_type).is_some_and(|def| def.symmetric);
        if src == issue || symmetric {
            held.insert((src, link::elem(&link_type, dst)));
        }
    }
    Ok(held)
}

/// The Issue's parent and outward Links compared three ways.
fn relations_plan(
    conn: &Connection,
    scheme: &Scheme,
    record: &IssueRecord,
    target: &Relations<'_>,
    writer: &Writer,
) -> Result<Plan, StoreError> {
    let Relations {
        issue,
        listed,
        detach,
    } = *target;
    let source = record.source.as_str();
    let current = view(conn, scheme, issue)?;
    let adopted = adopted(conn, source, issue)?;
    let mut plan = Plan::default();
    let draft = |project, subject, change| Draft {
        actor: writer.actor.clone(),
        project,
        subject,
        change,
    };

    let wanted = match &record.parent {
        Some(parent) => json!(resolve(conn, "parent", parent)?.to_string()),
        None => Value::Null,
    };
    let base_parent = base_value(&base(conn, source, Some(issue), "parent")?, issue, adopted);
    let tm = current.fields.get("parent").cloned().unwrap_or(Value::Null);
    match decide(&wanted, base_parent.as_ref(), &tm) {
        // A parent moving to another Issue lets go of the old one first.
        Decision::Write if detach && !wanted.is_null() => {
            if !tm.is_null() {
                let change = set("parent", Value::Null);
                plan.push(draft(current.project, issue, change), source, writer.at);
            }
        }
        Decision::Conflict if detach => {}
        Decision::Keep => {}
        Decision::Write => {
            let change = set("parent", wanted);
            plan.push(draft(current.project, issue, change), source, writer.at);
            plan.written.push("parent".into());
        }
        Decision::Conflict => plan.conflicts.push(Conflict {
            field: "parent".into(),
            tm,
            source: wanted,
            reason: None,
        }),
    }

    let wanted = wanted_links(conn, record, issue)?;
    let base = base(conn, source, None, link::FIELD)?;
    let held = held_links(conn, issue)?;
    let keys: BTreeSet<&(Uuid, String)> = wanted.iter().chain(base.keys()).chain(&held).collect();
    for key @ (subject, elem) in keys {
        let Some((link_type, target)) = elem
            .split_once('/')
            .and_then(|(link_type, target)| Some((link_type, Uuid::parse_str(target).ok()?)))
        else {
            return Err(StoreError::Corrupt(format!("Link element {elem:?}")));
        };
        let want = wanted.contains(key);
        if !want && listed.contains(key) {
            continue;
        }
        let had = base_member(&base, key, adopted);
        let has = crate::validate::has_link(conn, link_type, *subject, target)?;
        let name = format!("{}/{elem}", link::FIELD);
        match decide(&want, had.as_ref(), &has) {
            Decision::Write if detach && want => {}
            Decision::Conflict if detach => {}
            Decision::Keep => {}
            Decision::Write => {
                let project =
                    issue_project(conn, *subject)?.ok_or(StoreError::UnknownIssue(*subject))?;
                let change = Change::Link {
                    link_type: link_type.to_owned(),
                    target,
                    present: want,
                };
                plan.push(draft(project, *subject, change), source, writer.at);
                plan.note(name, want);
            }
            Decision::Conflict => plan.conflicts.push(Conflict {
                field: name,
                tm: json!(has),
                source: json!(want),
                reason: None,
            }),
        }
    }
    Ok(plan)
}
