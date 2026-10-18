use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;
use serde_json::{Value, json};
use tm_core::Change;
use tm_store::Store;
use uuid::Uuid;

use crate::support::{ManualClock, TempDir, draft, dump, links, new_issue, open, set_field};

const ISSUES: usize = 3;
/// A required built-in, two enumerated built-ins and a Scheme field.
const FIELDS: [&str; 4] = ["title", "status", "priority", "area"];
const STATUSES: [&str; 6] = [
    "triage",
    "todo",
    "in_progress",
    "in_review",
    "done",
    "cancelled",
];
const PRIORITIES: [&str; 5] = ["urgent", "high", "medium", "low", "none"];
const ELEMS: [&str; 3] = ["a", "b", "c"];
/// A directional type that can close a cycle, and a symmetric one.
const LINK_TYPES: [&str; 2] = ["blocks", "relates_to"];

#[derive(Debug, Clone)]
enum Action {
    /// `pick` chooses the value; `None` clears the fields that may be cleared.
    Set {
        issue: usize,
        field: usize,
        pick: Option<u8>,
    },
    Add {
        issue: usize,
        elem: usize,
    },
    Remove {
        issue: usize,
        elem: usize,
    },
    Comment {
        issue: usize,
        body: String,
    },
    Link {
        from: usize,
        to: usize,
        link_type: usize,
        present: bool,
    },
    /// Zero leaves the clock in place, so writes share a millisecond and
    /// exercise the counter.
    Tick(u8),
}

fn action() -> impl Strategy<Value = Action> {
    let issue = 0..ISSUES;
    prop_oneof![
        (issue.clone(), 0..FIELDS.len(), proptest::option::of(0u8..6))
            .prop_map(|(issue, field, pick)| Action::Set { issue, field, pick }),
        (issue.clone(), 0..ELEMS.len()).prop_map(|(issue, elem)| Action::Add { issue, elem }),
        (issue.clone(), 0..ELEMS.len()).prop_map(|(issue, elem)| Action::Remove { issue, elem }),
        (issue.clone(), "[a-z ]{0,8}").prop_map(|(issue, body)| Action::Comment { issue, body }),
        (issue.clone(), issue, 0..LINK_TYPES.len(), any::<bool>()).prop_map(
            |(from, to, link_type, present)| Action::Link {
                from,
                to,
                link_type,
                present,
            }
        ),
        (0u8..3).prop_map(Action::Tick),
    ]
}

/// The value a `Set` writes and how the Issue reads it back. Title and Status
/// are required, so `None` sets them rather than clearing them.
fn value(field: &str, pick: Option<u8>) -> (Value, Value) {
    let index = usize::from(pick.unwrap_or(0));
    match (field, pick) {
        ("title", _) => (json!(format!("t{index}")), json!(format!("t{index}"))),
        ("status", _) => {
            let status = STATUSES[index % STATUSES.len()];
            (json!(status), json!(status))
        }
        ("priority", None) => (Value::Null, json!("none")),
        ("priority", Some(_)) => {
            let level = PRIORITIES[index % PRIORITIES.len()];
            (json!(level), json!(level))
        }
        (_, None) => (Value::Null, Value::Null),
        (_, Some(n)) => (json!(n), json!(n)),
    }
}

fn change(action: &Action) -> Option<(usize, Change)> {
    let member = |elem: usize| ("labels".to_owned(), ELEMS[elem].to_owned());
    Some(match action {
        Action::Set { issue, field, pick } => (
            *issue,
            set_field(FIELDS[*field], value(FIELDS[*field], *pick).0),
        ),
        Action::Add { issue, elem } => {
            let (field, elem) = member(*elem);
            (
                *issue,
                Change::Add {
                    field,
                    label: String::new(),
                    elem,
                    elem_label: None,
                },
            )
        }
        Action::Remove { issue, elem } => {
            let (field, elem) = member(*elem);
            (
                *issue,
                Change::Remove {
                    field,
                    label: String::new(),
                    elem,
                    elem_label: None,
                },
            )
        }
        Action::Comment { issue, body } => (*issue, Change::Comment { body: body.clone() }),
        Action::Link { .. } | Action::Tick(_) => return None,
    })
}

/// What one Replica's writes should produce, computed without the store: the
/// last write to each field or set element wins.
#[derive(Debug, PartialEq, Eq)]
struct Model {
    fields: Vec<BTreeMap<String, Value>>,
    labels: Vec<BTreeSet<String>>,
    comments: Vec<usize>,
    /// `(type, source, target)` in canonical form.
    links: BTreeSet<(String, Uuid, Uuid)>,
}

impl Model {
    /// The canonical form of a Link, or `None` when it must be refused: an
    /// Issue joined to itself, or a `blocks` add whose target already reaches
    /// its source.
    fn link(&self, from: Uuid, to: Uuid, link_type: &str, present: bool) -> Option<(Uuid, Uuid)> {
        if from == to {
            return None;
        }
        if link_type == "relates_to" {
            return Some((from.min(to), from.max(to)));
        }
        if present {
            let mut reached = BTreeSet::from([to]);
            let mut frontier = vec![to];
            while let Some(at) = frontier.pop() {
                for (kind, src, dst) in &self.links {
                    if kind == "blocks" && *src == at && reached.insert(*dst) {
                        frontier.push(*dst);
                    }
                }
            }
            if reached.contains(&from) {
                return None;
            }
        }
        Some((from, to))
    }

    /// Issues as `new_issue` creates them.
    fn new() -> Self {
        let created = BTreeMap::from([
            ("title".to_owned(), json!("t")),
            ("status".to_owned(), json!("todo")),
            ("priority".to_owned(), json!("none")),
        ]);
        Self {
            fields: vec![created; ISSUES],
            labels: vec![BTreeSet::new(); ISSUES],
            comments: vec![0; ISSUES],
            links: BTreeSet::new(),
        }
    }

    fn record(&mut self, action: &Action) {
        match action {
            Action::Set { issue, field, pick } => {
                let name = FIELDS[*field];
                let read = value(name, *pick).1;
                if read.is_null() {
                    self.fields[*issue].remove(name);
                } else {
                    self.fields[*issue].insert(name.to_owned(), read);
                }
            }
            Action::Add { issue, elem } => {
                self.labels[*issue].insert(ELEMS[*elem].to_owned());
            }
            Action::Remove { issue, elem } => {
                self.labels[*issue].remove(ELEMS[*elem]);
            }
            Action::Comment { issue, .. } => self.comments[*issue] += 1,
            Action::Link { .. } | Action::Tick(_) => {}
        }
    }

    fn observed(store: &Store, issues: &[Uuid]) -> Self {
        let mut model = Self {
            fields: Vec::new(),
            labels: Vec::new(),
            comments: Vec::new(),
            links: links(store).into_iter().collect(),
        };
        for issue in issues {
            let view = store.issue(*issue).unwrap().unwrap();
            model.fields.push(
                view.fields
                    .into_iter()
                    .filter(|(name, _)| FIELDS.contains(&name.as_str()))
                    .collect(),
            );
            model.labels.push(
                view.sets
                    .get("labels")
                    .map(|labels| labels.keys().cloned().collect())
                    .unwrap_or_default(),
            );
            let comments: i64 = store
                .connection()
                .query_row(
                    "SELECT count(*) FROM comments WHERE issue_id = ?1",
                    [issue.as_bytes()],
                    |r| r.get(0),
                )
                .unwrap();
            model.comments.push(usize::try_from(comments).unwrap());
        }
        model
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// ADR 0005: the projections are what replaying the log produces, both
    /// against an independent model and against a full rebuild.
    #[test]
    fn projections_match_the_log(actions in proptest::collection::vec(action(), 0..60)) {
        let dir = TempDir::new();
        let clock = ManualClock::at(1_000);
        let mut store = open(&dir.db(), &clock);
        let issues: Vec<Uuid> = (0..ISSUES).map(|_| new_issue(&mut store)).collect();
        let mut expected = Model::new();
        for action in &actions {
            if let Action::Tick(ms) = action {
                clock.advance(i64::from(*ms));
            } else if let Action::Link { from, to, link_type, present } = action {
                let (from, to, link_type) = (issues[*from], issues[*to], LINK_TYPES[*link_type]);
                let result = if *present {
                    store.link("jace", link_type, from, to)
                } else {
                    store.unlink("jace", link_type, from, to)
                };
                match expected.link(from, to, link_type, *present) {
                    Some((src, dst)) => {
                        result.unwrap();
                        let key = (link_type.to_owned(), src, dst);
                        if *present {
                            expected.links.insert(key);
                        } else {
                            expected.links.remove(&key);
                        }
                    }
                    None => {
                        result.unwrap_err();
                    }
                }
            } else if let Some((issue, change)) = change(action) {
                store.append(draft(issues[issue], change)).unwrap();
                expected.record(action);
            }
        }
        prop_assert_eq!(Model::observed(&store, &issues), expected);
        let incremental = dump(&store);
        prop_assert_eq!(store.rebuild().unwrap(), 0, "the store's own writes must all replay");
        prop_assert_eq!(dump(&store), incremental);
    }
}
