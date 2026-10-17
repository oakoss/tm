use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;
use tm_core::Change;
use tm_store::Store;
use uuid::Uuid;

use crate::support::{ManualClock, TempDir, draft, dump, open, set_field};

const ISSUES: usize = 3;
const FIELDS: [&str; 3] = ["status", "priority", "area"];
const SET_FIELDS: [&str; 2] = ["labels", "file_paths"];
const ELEMS: [&str; 3] = ["a", "b", "c"];

#[derive(Debug, Clone)]
enum Action {
    Set {
        issue: usize,
        field: usize,
        value: Option<i64>,
    },
    Add {
        issue: usize,
        field: usize,
        elem: usize,
    },
    Remove {
        issue: usize,
        field: usize,
        elem: usize,
    },
    Comment {
        issue: usize,
        body: String,
    },
    /// Zero leaves the clock in place, so writes share a millisecond and
    /// exercise the counter.
    Tick(u8),
}

fn action() -> impl Strategy<Value = Action> {
    let issue = 0..ISSUES;
    prop_oneof![
        (
            issue.clone(),
            0..FIELDS.len(),
            proptest::option::of(-3i64..3)
        )
            .prop_map(|(issue, field, value)| Action::Set {
                issue,
                field,
                value
            }),
        (issue.clone(), 0..SET_FIELDS.len(), 0..ELEMS.len())
            .prop_map(|(issue, field, elem)| Action::Add { issue, field, elem }),
        (issue.clone(), 0..SET_FIELDS.len(), 0..ELEMS.len())
            .prop_map(|(issue, field, elem)| Action::Remove { issue, field, elem }),
        (issue, "[a-z ]{0,8}").prop_map(|(issue, body)| Action::Comment { issue, body }),
        (0u8..3).prop_map(Action::Tick),
    ]
}

fn change(action: &Action) -> Option<(usize, Change)> {
    let member = |field: usize, elem: usize| (SET_FIELDS[field].to_owned(), ELEMS[elem].to_owned());
    Some(match action {
        Action::Set {
            issue,
            field,
            value,
        } => (
            *issue,
            set_field(
                FIELDS[*field],
                value.map_or(serde_json::Value::Null, Into::into),
            ),
        ),
        Action::Add { issue, field, elem } => {
            let (field, elem) = member(*field, *elem);
            (
                *issue,
                Change::Add {
                    field,
                    label: String::new(),
                    elem,
                },
            )
        }
        Action::Remove { issue, field, elem } => {
            let (field, elem) = member(*field, *elem);
            (
                *issue,
                Change::Remove {
                    field,
                    label: String::new(),
                    elem,
                },
            )
        }
        Action::Comment { issue, body } => (*issue, Change::Comment { body: body.clone() }),
        Action::Tick(_) => return None,
    })
}

/// What one Replica's writes should produce, computed without the store: the
/// last write to each field or set element wins.
#[derive(Debug, Default, PartialEq, Eq)]
struct Model {
    fields: BTreeMap<usize, BTreeMap<String, i64>>,
    members: BTreeSet<(usize, String, String)>,
    comments: BTreeMap<usize, usize>,
}

impl Model {
    fn record(&mut self, action: &Action) {
        match action {
            Action::Set {
                issue,
                field,
                value,
            } => {
                let fields = self.fields.entry(*issue).or_default();
                match value {
                    Some(v) => fields.insert(FIELDS[*field].to_owned(), *v),
                    None => fields.remove(FIELDS[*field]),
                };
                if fields.is_empty() {
                    self.fields.remove(issue);
                }
            }
            Action::Add { issue, field, elem } => {
                self.members.insert((
                    *issue,
                    SET_FIELDS[*field].to_owned(),
                    ELEMS[*elem].to_owned(),
                ));
            }
            Action::Remove { issue, field, elem } => {
                self.members.remove(&(
                    *issue,
                    SET_FIELDS[*field].to_owned(),
                    ELEMS[*elem].to_owned(),
                ));
            }
            Action::Comment { issue, .. } => *self.comments.entry(*issue).or_default() += 1,
            Action::Tick(_) => {}
        }
    }

    fn observed(store: &Store, issues: &[Uuid]) -> Self {
        let conn = store.connection();
        let index = |id: &[u8]| issues.iter().position(|i| i.as_bytes() == id).unwrap();
        let mut model = Self::default();
        let mut stmt = conn.prepare("SELECT id, fields FROM issues").unwrap();
        for row in stmt
            .query_map([], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?))
            })
            .unwrap()
        {
            let (id, fields) = row.unwrap();
            let fields: BTreeMap<String, i64> = serde_json::from_str(&fields).unwrap();
            if !fields.is_empty() {
                model.fields.insert(index(&id), fields);
            }
        }
        let mut stmt = conn
            .prepare("SELECT issue_id, field, elem FROM set_members")
            .unwrap();
        for row in stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .unwrap()
        {
            let (id, field, elem) = row.unwrap();
            model.members.insert((index(&id), field, elem));
        }
        let mut stmt = conn
            .prepare("SELECT issue_id, count(*) FROM comments GROUP BY issue_id")
            .unwrap();
        for row in stmt
            .query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)))
            .unwrap()
        {
            let (id, count) = row.unwrap();
            model
                .comments
                .insert(index(&id), usize::try_from(count).unwrap());
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
        let issues: Vec<Uuid> = (0..ISSUES).map(|_| Uuid::now_v7()).collect();
        for issue in &issues {
            store.append(draft(*issue, Change::Create)).unwrap();
        }
        let mut expected = Model::default();
        for action in &actions {
            if let Action::Tick(ms) = action {
                clock.advance(i64::from(*ms));
            } else if let Some((issue, change)) = change(action) {
                store.append(draft(issues[issue], change)).unwrap();
                expected.record(action);
            }
        }
        prop_assert_eq!(Model::observed(&store, &issues), expected);
        let incremental = dump(&store);
        store.rebuild().unwrap();
        prop_assert_eq!(dump(&store), incremental);
    }
}
