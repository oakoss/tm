//! `tm history`: what happened to an Issue, oldest first, a write at a time.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::Change;
use tm_core::op::ClaimAction;
use tm_store::{HistoryEntry, Store, short_session};
use uuid::Uuid;

use crate::edit::{self, Changed};
use crate::error::CliError;
use crate::here;
use crate::identity;
use crate::mention;
use crate::output::{Output, local_offset_min, local_time, printable, short, span, time_at};

/// One write: the operations of one batch, or one operation another Replica
/// wrote.
struct Write<'a> {
    first: &'a HistoryEntry,
    lines: Vec<String>,
    changes: Vec<Value>,
    /// Each change's Issue, and which Short-list field it sets, if either.
    membership: Vec<(Uuid, Option<&'a str>)>,
}

impl Write<'_> {
    /// Drops a rank change written with its Issue's list change: the list
    /// line says it joined or left, and the rank is never shown (ADR 0016).
    fn without_joining_ranks(&mut self) {
        let listed: Vec<Uuid> = self
            .membership
            .iter()
            .filter(|(_, field)| *field == Some(SHORTLIST))
            .map(|(issue, _)| *issue)
            .collect();
        let keep: Vec<bool> = self
            .membership
            .iter()
            .map(|(issue, field)| *field != Some(RANK) || !listed.contains(issue))
            .collect();
        let mut kept = keep.iter();
        self.lines.retain(|_| *kept.next().unwrap_or(&true));
        let mut kept = keep.iter();
        self.changes.retain(|_| *kept.next().unwrap_or(&true));
    }
}

const SHORTLIST: &str = "shortlist";
const RANK: &str = "shortlist_rank";

pub(crate) fn run(reference: &str, limit: Option<usize>, all: bool) -> Result<Output, CliError> {
    let here = here::read()?;
    // Resolved only for a write at another offset, since it may run git.
    let me = OnceCell::new();
    let me = || {
        me.get_or_init(|| {
            let viewer = identity::resolve(identity::env, || identity::git_email(&here.found.dir));
            viewer.ok().map(|identity| {
                let actor = identity.actor;
                actor.on_behalf_of().unwrap_or(actor.name()).to_owned()
            })
        })
        .clone()
    };
    let shown = here.issue(reference);
    match shown.and_then(|id| render(&here.store, id, limit, all, &me)) {
        Ok(mut output) => {
            output.warnings = here.warnings;
            Ok(output)
        }
        Err(err) => Err(err.warned(here.warnings)),
    }
}

/// An Issue's history as `tm history` prints it, `me` naming the viewer, whose
/// own writes show in local time.
pub(crate) fn render(
    store: &Store,
    id: Uuid,
    limit: Option<usize>,
    all: bool,
    me: &dyn Fn() -> Option<String>,
) -> Result<Output, CliError> {
    let view = store.issue(id)?.ok_or_else(|| {
        CliError::new("not_found", format!("Issue {id} does not exist"))
            .with("kind", "issue")
            .with("id", id.to_string())
    })?;
    let entries = store.history(id)?;
    let mut names = Names::new(store, &entries)?;
    // A new Issue is unarchived without an operation saying so.
    let unarchived = Before {
        stored: Value::Bool(false),
        shown: Value::Bool(false),
    };
    let mut before = HashMap::from([("archived".to_owned(), unarchived)]);
    let mut writes: Vec<Write<'_>> = Vec::new();
    let mut created = None;
    for entry in &entries {
        let renewal = matches!(
            entry.change,
            Change::Claim {
                action: ClaimAction::Renew { .. }
            }
        );
        if entry.change == Change::Create {
            created = entry.batch;
        }
        let creating = entry.batch.is_some() && entry.batch == created;
        let (line, stored) = describe(id, entry, creating, &mut before, &mut names)?;
        let line = names.mentions.shown(&line);
        // Renewals repeat with every write and heartbeat.
        if renewal && !all {
            continue;
        }
        let field = match &entry.change {
            Change::SetField { field, .. } if field == SHORTLIST || field == RANK => {
                Some(field.as_str())
            }
            _ => None,
        };
        let change = if field == Some(RANK) {
            json!({ "op_id": entry.op_id, "subject": entry.subject, "line": line })
        } else {
            json!({
                "op_id": entry.op_id,
                "subject": entry.subject,
                "change": entry.change,
                "before": stored,
                "line": line,
                "unapplied": entry.unapplied,
                "lost": entry.lost,
            })
        };
        match writes.last_mut() {
            Some(write) if same_write(write.first, entry) => {
                write.lines.push(line);
                write.changes.push(change);
                write.membership.push((entry.subject, field));
            }
            _ => writes.push(Write {
                first: entry,
                lines: vec![line],
                changes: vec![change],
                membership: vec![(entry.subject, field)],
            }),
        }
    }
    for write in &mut writes {
        write.without_joining_ranks();
    }
    let total = writes.len();
    let shown = &writes[total - limit.unwrap_or(total).min(total)..];
    let name = view.key.clone().unwrap_or_else(|| id.to_string());
    let title = view.fields.get("title").and_then(Value::as_str);
    let mut text = format!("{name}  {}\n", printable(title.unwrap_or_default()));
    for write in shown {
        let _ = writeln!(text, "{}", header(write.first, me));
        for line in &write.lines {
            let _ = writeln!(text, "  {line}");
        }
    }
    if shown.len() < total {
        let _ = writeln!(text, "({} of {total} writes shown)", shown.len());
    }
    let writes: Vec<Value> = shown.iter().map(write_json).collect();
    let data = json!({ "id": id, "key": view.key, "total": total, "writes": writes });
    mention::named(store, Output::new(text, data))
}

/// Whether `entry` belongs to the write `first` began: the same batch, by the
/// same Actor at the same time, as an import's batch can carry a record's
/// comments by other authors.
fn same_write(first: &HistoryEntry, entry: &HistoryEntry) -> bool {
    entry.batch.is_some()
        && first.batch == entry.batch
        && first.actor == entry.actor
        && first.on_behalf_of == entry.on_behalf_of
        && first.source == entry.source
        && first.session == entry.session
        && first.utc_ms == entry.utc_ms
}

fn write_json(write: &Write<'_>) -> Value {
    json!({
        "batch": write.first.batch,
        "undid": write.first.undid,
        "undone_by": write.first.undone_by,
        "actor": write.first.actor,
        "on_behalf_of": write.first.on_behalf_of,
        "source": write.first.source,
        "session": write.first.session,
        "utc_ms": write.first.utc_ms,
        "offset_min": write.first.offset_min,
        "changes": write.changes,
    })
}

/// When, who, and the batch to name to `tm undo`, with how undo touched it.
/// A write for `me` shows in this machine's zone, another's at the offset it
/// was written at (ADR 0006); the two agree when the offsets do.
fn header(entry: &HistoryEntry, me: &dyn Fn() -> Option<String>) -> String {
    let human = entry.on_behalf_of.as_deref().unwrap_or(&entry.actor);
    let here = local_offset_min(entry.utc_ms) == Some(entry.offset_min);
    let when = if here || me().as_deref() == Some(human) {
        local_time(entry.utc_ms)
    } else {
        time_at(entry.utc_ms, entry.offset_min)
    };
    let mut line = format!("{when}  {}", printable(&entry.actor));
    if let Some(human) = &entry.on_behalf_of {
        let _ = write!(line, " for {}", printable(human));
    }
    if let Some(session) = &entry.session {
        let _ = write!(line, " in session {}", printable(&short_session(session)));
    }
    let mut marks = Vec::new();
    if let Some(batch) = entry.batch {
        marks.push(format!("batch {}", short(batch)));
    }
    if let Some(undid) = entry.undid {
        marks.push(format!("undoes {}", short(undid)));
    }
    if let Some(undone_by) = entry.undone_by {
        marks.push(format!("undone by {}", short(undone_by)));
    }
    if !marks.is_empty() {
        let _ = write!(line, "  ({})", marks.join(", "));
    }
    line
}

/// A Link added or removed, named from `issue`'s end.
fn link_line(
    issue: Uuid,
    entry: &HistoryEntry,
    (link_type, target, present): (&str, Uuid, bool),
    names: &mut Names<'_>,
) -> Result<String, CliError> {
    let known = tm_core::link::link_type(link_type);
    let outward = known.map_or(link_type, |known| known.outward);
    let inward = known.map_or(link_type, |known| known.inward);
    let (other, name) = if entry.subject == issue {
        (target, outward)
    } else {
        (entry.subject, inward)
    };
    let verb = if present { "linked" } else { "unlinked" };
    Ok(format!("{verb}: {name} {}", names.key(other)?))
}

/// One operation as a line; a field set as the Issue is `creating` shows no
/// earlier value. `before` holds each field's value so far, which an
/// operation that was not applied or lost its merge leaves as it was.
fn describe(
    issue: Uuid,
    entry: &HistoryEntry,
    creating: bool,
    before: &mut HashMap<String, Before>,
    names: &mut Names<'_>,
) -> Result<(String, Option<Value>), CliError> {
    let mut stored = comment_text(entry, before);
    let line = match &entry.change {
        Change::Create => "created".to_owned(),
        Change::SetField { field, .. } if field == RANK => "Short-list: reordered".to_owned(),
        Change::SetField {
            field,
            label,
            value,
            value_label,
        } => {
            let after = match (field.as_str(), value) {
                ("parent", Value::String(parent)) => names.parent(parent)?,
                _ => match value_label {
                    Some(label) => Value::String(label.clone()),
                    None => value.clone(),
                },
            };
            let prior = before.get(field).cloned().unwrap_or_default();
            if entry.unapplied.is_none() && !entry.lost {
                let now = Before {
                    stored: value.clone(),
                    shown: after.clone(),
                };
                before.insert(field.clone(), now);
            }
            stored = Some(prior.stored);
            let name = shown_name(field, label);
            if creating && prior.shown.is_null() {
                first_value(name, &after)
            } else {
                edit::summary(&Changed {
                    field: name.to_owned(),
                    before: prior.shown,
                    after,
                })
            }
        }
        Change::Add {
            field,
            label,
            elem,
            elem_label,
        } => {
            let shown = elem_label.as_ref().unwrap_or(elem);
            printable(&format!("{}: + {shown}", shown_name(field, label)))
        }
        Change::Remove {
            field,
            label,
            elem,
            elem_label,
        } => {
            let shown = elem_label.as_ref().unwrap_or(elem);
            printable(&format!("{}: - {shown}", shown_name(field, label)))
        }
        Change::Comment { body } => format!("commented: {}", names.first(body)),
        Change::Ask { body, .. } => format!("asked: {}", names.first(body)),
        Change::Answer { body } => format!("answered: {}", names.first(body)),
        Change::Withdraw { body } => format!("withdrew the question: {}", names.first(body)),
        Change::Retract {
            retracted: true, ..
        } => "retracted a comment".to_owned(),
        Change::Retract { .. } => "showed a retracted comment again".to_owned(),
        Change::EditComment { comment, body } => {
            format!("edited comment {}: {}", short(*comment), names.first(body))
        }
        Change::Link {
            link_type,
            target,
            present,
        } => link_line(issue, entry, (link_type, *target, *present), names)?,
        Change::Claim { action } => claim_line(action, entry, before),
        Change::PassedClaim { holder, fields, .. } => printable(&format!(
            "wrote {} past {holder}'s Claim",
            fields.join(", ")
        )),
        Change::CreateProject { .. }
        | Change::RenameProject { .. }
        | Change::CreateShortlist
        | Change::SetShortlist { .. }
        | Change::ShortlistActor { .. }
        | Change::ShortlistSource
        | Change::CreateMemory
        | Change::SetMemory { .. }
        | Change::MemorySource
        | Change::SetName { .. }
        | Change::SetRead { .. } => entry.change.kind().to_owned(),
    };
    let line = match &entry.unapplied {
        Some(reason) => format!("{line}  (not applied: {})", printable(reason)),
        None if entry.lost => format!("{line}  (lost to a concurrent change)"),
        None => line,
    };
    Ok((line, stored))
}

/// Keeps each comment's text in `before` under `comment/<id>`, and gives an
/// edit the text it replaced.
fn comment_text(entry: &HistoryEntry, before: &mut HashMap<String, Before>) -> Option<Value> {
    let (comment, body) = match &entry.change {
        Change::Comment { body }
        | Change::Ask { body, .. }
        | Change::Answer { body }
        | Change::Withdraw { body } => (entry.op_id, body),
        Change::EditComment { comment, body } => (*comment, body),
        _ => return None,
    };
    let key = format!("comment/{comment}");
    let edit = matches!(entry.change, Change::EditComment { .. });
    let prior = edit.then(|| {
        before
            .get(&key)
            .map_or(Value::Null, |prior| prior.stored.clone())
    });
    // A comment replayed after its edit keeps the edit's text.
    if entry.unapplied.is_none() && !entry.lost && (edit || !before.contains_key(&key)) {
        let text = Value::String(body.clone());
        let now = Before {
            stored: text.clone(),
            shown: text,
        };
        before.insert(key, now);
    }
    prior
}

/// A field's value before an operation: as stored, and as the line shows it.
#[derive(Clone, Default)]
struct Before {
    stored: Value,
    shown: Value,
}

/// A field by the label its operation recorded, which stays readable after
/// the Scheme renames it (ADR 0007), or by its id when none was recorded.
fn shown_name<'a>(field: &'a str, label: &'a str) -> &'a str {
    if label.is_empty() { field } else { label }
}

fn first_line(body: &str) -> String {
    let first = body.trim().lines().next().unwrap_or_default();
    let cut: String = first.chars().take(60).collect();
    let more = if cut == body.trim() { "" } else { "…" };
    format!("{}{more}", printable(&cut))
}

/// A field's value as the Issue was created with it.
fn first_value(field: &str, value: &Value) -> String {
    let shown = edit::shown(value);
    let line = if edit::long(&shown) {
        let lines = shown.lines().count();
        let plural = if lines == 1 { "" } else { "s" };
        format!("{field}: {lines} line{plural}")
    } else {
        format!("{field}: {shown}")
    };
    printable(&line)
}

/// Whether a take is by the Agent and session that held the Claim it
/// replaced, read from the holder `before` keeps as history is walked in
/// order, since a take names only the Claim's id; then records the holder
/// `action` leaves, when it `applied`.
fn holding(action: &ClaimAction, applied: bool, before: &mut HashMap<String, Before>) -> bool {
    let holder = |agent: &str, session: &Option<String>| json!([agent, session]);
    let again = match action {
        ClaimAction::Take {
            agent,
            session,
            prev: Some(_),
            ..
        } => before
            .get(CLAIM_HOLDER)
            .is_some_and(|held| session.is_some() && held.stored == holder(agent, session)),
        _ => false,
    };
    let left = match action {
        ClaimAction::Take { agent, session, .. } => holder(agent, session),
        ClaimAction::Hand { agent, .. } => holder(agent, &None),
        ClaimAction::Release { .. } => Value::Null,
        ClaimAction::Renew { .. } => return again,
    };
    if !applied {
        return again;
    }
    before.insert(
        CLAIM_HOLDER.to_owned(),
        Before {
            stored: left.clone(),
            shown: left,
        },
    );
    again
}

/// The key under which [`holding`] keeps the Claim's holder.
const CLAIM_HOLDER: &str = "\0claim holder";

/// `entry`'s session is the write's own, which its header names already;
/// `before` holds the Claim's holder so far, which [`holding`] keeps.
fn claim_line(
    action: &ClaimAction,
    entry: &HistoryEntry,
    before: &mut HashMap<String, Before>,
) -> String {
    let written_in = entry.session.as_deref();
    let again = holding(action, entry.unapplied.is_none() && !entry.lost, before);
    let lasts = |ttl_ms: Option<u64>| match ttl_ms.and_then(|ttl| i64::try_from(ttl).ok()) {
        Some(ttl) => format!(" for {}", span(ttl)),
        None => ", with no expiry".to_owned(),
    };
    match action {
        ClaimAction::Take {
            agent,
            ttl_ms,
            prev,
            session,
            branch,
            paths,
        } => {
            let mut who = printable(agent);
            if let Some(session) = session
                .as_deref()
                .filter(|session| Some(*session) != written_in)
            {
                let session = printable(&short_session(session));
                let _ = write!(who, " (session {session})");
            }
            if let Some(branch) = branch {
                let _ = write!(who, " on {}", printable(branch));
            }
            let verb = match prev {
                Some(_) if again => "claimed again",
                Some(_) => "took over the Claim",
                None => "claimed",
            };
            let covering = if paths.is_empty() {
                String::new()
            } else {
                format!(", covering {}", printable(&paths.join(", ")))
            };
            format!("{verb}: {who}{}{covering}", lasts(*ttl_ms))
        }
        ClaimAction::Renew { ttl_ms, .. } => format!("Claim renewed{}", lasts(Some(*ttl_ms))),
        ClaimAction::Release { .. } => "Claim released".to_owned(),
        ClaimAction::Hand { agent, ttl_ms, .. } => {
            format!("Claim handed to {}{}", printable(agent), lasts(*ttl_ms))
        }
    }
}

/// Other Issues' Keys, read once each, and how each mention is shown.
struct Names<'a> {
    store: &'a Store,
    keys: HashMap<Uuid, String>,
    mentions: mention::Names,
}

impl<'a> Names<'a> {
    /// Names for `entries`, whose mentions' names are read only when one
    /// of their texts holds a token.
    fn new(store: &'a Store, entries: &[HistoryEntry]) -> Result<Self, CliError> {
        let texts = entries.iter().filter_map(|entry| match &entry.change {
            Change::Comment { body }
            | Change::Ask { body, .. }
            | Change::Answer { body }
            | Change::Withdraw { body }
            | Change::EditComment { body, .. } => Some(body.as_str()),
            Change::SetField { value, .. } => value.as_str(),
            _ => None,
        });
        Ok(Self {
            store,
            keys: HashMap::new(),
            mentions: mention::Names::of(store, texts)?,
        })
    }

    /// A text's first line, its mentions shown by name before it is cut.
    fn first(&self, body: &str) -> String {
        first_line(&self.mentions.shown(body))
    }

    /// `issue`'s Key, or its id when it has none or no longer exists.
    fn key(&mut self, issue: Uuid) -> Result<String, CliError> {
        if let Some(key) = self.keys.get(&issue) {
            return Ok(key.clone());
        }
        let key = self.store.issue(issue)?.and_then(|view| view.key);
        let key = key.unwrap_or_else(|| issue.to_string());
        self.keys.insert(issue, key.clone());
        Ok(key)
    }

    /// A parent as `edit::summary` shows one, by Key where it has one.
    fn parent(&mut self, parent: &str) -> Result<Value, CliError> {
        Ok(match Uuid::parse_str(parent) {
            Ok(id) => json!({ "id": parent, "key": self.key(id)? }),
            Err(_) => Value::String(parent.to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn priority(value: &str, unapplied: Option<&str>) -> HistoryEntry {
        HistoryEntry {
            op_id: Uuid::now_v7(),
            batch: None,
            undid: None,
            undone_by: None,
            actor: "codex".into(),
            on_behalf_of: None,
            source: None,
            session: None,
            utc_ms: 0,
            offset_min: 330,
            subject: Uuid::nil(),
            change: Change::SetField {
                field: "priority".into(),
                label: "Priority".into(),
                value: json!(value),
                value_label: None,
            },
            unapplied: unapplied.map(str::to_owned),
            lost: false,
        }
    }

    #[test]
    fn a_write_splits_where_its_author_changes() {
        let batch = Some(Uuid::now_v7());
        let first = HistoryEntry {
            batch,
            ..priority("high", None)
        };
        let same = HistoryEntry {
            batch,
            ..priority("low", None)
        };
        assert!(same_write(&first, &same));
        let other = HistoryEntry {
            batch,
            actor: "ana".into(),
            ..priority("low", None)
        };
        assert!(!same_write(&first, &other), "another author");
        let elsewhere = priority("low", None);
        assert!(!same_write(&elsewhere, &priority("low", None)), "no batch");
        let others = [
            HistoryEntry {
                on_behalf_of: Some("ana".into()),
                ..same.clone()
            },
            HistoryEntry {
                source: Some("records.jsonl:2".into()),
                ..same.clone()
            },
            HistoryEntry {
                utc_ms: 1,
                ..same.clone()
            },
        ];
        for other in &others {
            assert!(!same_write(&first, other), "{other:?}");
        }
    }

    #[test]
    fn your_writes_show_in_your_zone_and_others_at_their_offset() {
        // An offset no zone this test runs in uses at 1970.
        let entry = HistoryEntry {
            offset_min: 345,
            ..priority("high", None)
        };
        let theirs = header(&entry, &|| Some("jace".into()));
        assert_eq!(theirs, "1970-01-01 05:45 +05:45  codex");
        let yours = header(&entry, &|| Some("codex".into()));
        assert_eq!(yours, format!("{}  codex", local_time(0)));
        let for_you = HistoryEntry {
            on_behalf_of: Some("jace".into()),
            ..entry.clone()
        };
        let agents = header(&for_you, &|| Some("jace".into()));
        assert_eq!(agents, format!("{}  codex for jace", local_time(0)));
        let here = HistoryEntry {
            offset_min: local_offset_min(0).unwrap(),
            ..entry
        };
        let unasked = header(&here, &|| panic!("the viewer is read only when needed"));
        assert_eq!(unasked, format!("{}  codex", local_time(0)));
    }

    #[test]
    fn an_unapplied_or_lost_change_leaves_the_value_the_next_one_changes() {
        let dir = std::env::temp_dir().join(format!("tm-history-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("tm.db")).unwrap();
        let mut names = Names::new(&store, &[]).unwrap();
        let mut before = HashMap::new();
        let lost = HistoryEntry {
            lost: true,
            ..priority("medium", None)
        };
        let entries = [
            priority("high", None),
            priority("bogus", Some("no such priority")),
            lost,
            priority("low", None),
        ];
        let lines: Vec<String> = entries
            .iter()
            .map(|entry| {
                describe(Uuid::nil(), entry, false, &mut before, &mut names)
                    .unwrap()
                    .0
            })
            .collect();
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            lines,
            [
                "Priority: (none) → high",
                "Priority: high → bogus  (not applied: no such priority)",
                "Priority: high → medium  (lost to a concurrent change)",
                "Priority: high → low",
            ]
        );
        assert_eq!(shown_name("f1", "Risk score"), "Risk score");
        assert_eq!(shown_name("f1", ""), "f1", "no label recorded");
    }

    #[test]
    fn a_take_by_the_session_holding_the_claim_reads_as_claiming_it_again() {
        let take = |agent: &str, session: &str, prev: bool, unapplied: Option<&'static str>| {
            let action = ClaimAction::Take {
                agent: agent.into(),
                ttl_ms: None,
                prev: prev.then(Uuid::now_v7),
                session: Some(session.into()),
                branch: None,
                paths: Vec::new(),
            };
            (action, unapplied)
        };
        let renew = ClaimAction::Renew {
            claim: Uuid::now_v7(),
            ttl_ms: 1,
        };
        let actions = [
            take("claude", "s1", false, None),
            (renew, None),
            take("claude", "s1", true, None),
            take("claude", "s2", true, Some("lost")),
            take("claude", "s1", true, None),
            take("codex", "s1", true, None),
        ];
        let mut before = HashMap::new();
        let verbs: Vec<String> = actions
            .into_iter()
            .map(|(action, unapplied)| {
                let entry = HistoryEntry {
                    unapplied: unapplied.map(str::to_owned),
                    ..priority("high", None)
                };
                let line = claim_line(&action, &entry, &mut before);
                line.split(':').next().unwrap().to_owned()
            })
            .collect();
        assert_eq!(
            verbs,
            [
                "claimed",
                "Claim renewed for under a minute",
                "claimed again",
                "took over the Claim",
                "claimed again",
                "took over the Claim",
            ]
        );
    }

    #[test]
    fn a_recorded_label_cannot_reach_the_terminal_raw() {
        let dir = std::env::temp_dir().join(format!("tm-history-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("tm.db")).unwrap();
        let mut names = Names::new(&store, &[]).unwrap();
        let mut before = HashMap::new();
        let hostile = "L\u{1b}[2J";
        let add = Change::Add {
            field: "labels".into(),
            label: hostile.into(),
            elem: "x".into(),
            elem_label: None,
        };
        let long = Change::SetField {
            field: "description".into(),
            label: hostile.into(),
            value: json!("one\ntwo"),
            value_label: None,
        };
        let lines: Vec<String> = [(add, true), (long.clone(), true), (long, false)]
            .into_iter()
            .map(|(change, creating)| {
                let entry = HistoryEntry {
                    change,
                    ..priority("x", None)
                };
                let shown = describe(Uuid::nil(), &entry, creating, &mut before, &mut names);
                shown.unwrap().0
            })
            .collect();
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
        let raw = lines.iter().any(|line| line.contains('\u{1b}'));
        assert!(!raw, "{lines:?}");
    }
}
