//! Checks a [`Query`] against the Scheme and turns it into typed conditions.
//! Names and enumerated values match without case and come out in their
//! canonical spelling, so a store compares them exactly.

use uuid::Uuid;

use super::suggest::did_you_mean;
use super::{Direction, FilterError, Query, Span, Term, Value};
use crate::scheme::{Category, Field, FieldType, PRIORITY_RANKS, Scheme};

/// Who is asking, for `@me`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context<'a> {
    pub me: Option<&'a str>,
}

/// Where a condition reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A column on `issues`, NULL while unset.
    Column(&'static str),
    /// An integer column on `issues` that always holds a value.
    IntColumn(&'static str),
    /// A Scheme field's value row.
    SchemeValue(String),
    /// A set field's elements.
    Set(String),
    /// The Short-list an Issue is on, as ADR 0016's read rule gives it: a
    /// live list in the Issue's Project, with a rank.
    Shortlist,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Text(String),
    /// A person, compared without case: people are typed by hand.
    Name(String),
    Number(f64),
    Integer(i64),
    /// An Issue, compared by id.
    Issue(Uuid),
    /// A Short-list, compared by id.
    Shortlist(Uuid),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// The target holds one of `values`; negated, it holds none of them,
    /// including when it holds nothing.
    Match {
        target: Target,
        values: Vec<Literal>,
        negated: bool,
    },
    Present {
        target: Target,
        present: bool,
    },
    /// The Issue's (type, Status) pair is one of `pairs`. With
    /// `include_unknown`, a pair outside `known` also matches, since a Status
    /// the Scheme does not define reads as Triage.
    Category {
        pairs: Vec<(String, String)>,
        known: Vec<(String, String)>,
        include_unknown: bool,
        negated: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortTarget {
    Column(&'static str),
    /// An integer rank whose 0 means unset, so it sorts last either way.
    Rank(&'static str),
    /// A person column, ordered without case as people are matched.
    Name(&'static str),
    /// A Scheme person field, ordered without case.
    SchemeName(String),
    SchemeNumber(String),
    SchemeText(String),
    /// A select field, ordered as the Scheme lists its options.
    SchemeOrder {
        field: String,
        order: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sort {
    pub target: SortTarget,
    pub descending: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Resolved {
    /// All must hold.
    pub conditions: Vec<Condition>,
    pub sorts: Vec<Sort>,
    /// Archived Issues are left out unless the filter names `archived`.
    pub archived_named: bool,
}

/// How a field can be filtered and sorted.
enum Kind {
    /// A fixed list of values; each option is its id and what the store compares.
    Enumerated {
        target: Target,
        options: Vec<(String, Literal)>,
    },
    Person(Target),
    Boolean(Target),
    Number(String),
    Date(String),
    Select(String, Vec<String>),
    MultiSelect(String, Vec<String>),
    Labels,
    Category,
    /// An Issue, named by id; a store turns Keys into ids before resolving.
    Issue(Target),
    /// A Short-list, named by id; a store turns names into ids before
    /// resolving.
    Shortlist,
    /// Known, but filtering by value arrives later; `has:`/`no:` still work
    /// when there is a target.
    Later(&'static str, Option<Target>),
}

struct Filterable {
    name: String,
    kind: Kind,
    /// The sort target and whether it runs descending by default.
    sort: Option<(SortTarget, bool)>,
}

/// Every field the filter language accepts. The error that lists valid fields
/// and the lookup both read this, so they cannot disagree.
fn filterable(scheme: &Scheme) -> Vec<Filterable> {
    let field = |name: &str, kind, sort| Filterable {
        name: name.to_owned(),
        kind,
        sort,
    };
    let text = |id: &str| (id.to_owned(), Literal::Text(id.to_owned()));
    let mut statuses: Vec<String> = Vec::new();
    for status in scheme
        .workflows
        .iter()
        .flat_map(|workflow| &workflow.statuses)
    {
        if !statuses.contains(&status.id) {
            statuses.push(status.id.clone());
        }
    }
    let mut fields = vec![
        field(
            "type",
            Kind::Enumerated {
                target: Target::Column("type"),
                options: scheme.types.iter().map(|t| text(&t.id)).collect(),
            },
            Some((SortTarget::Column("type"), false)),
        ),
        field(
            "title",
            Kind::Later("text search", Some(Target::Column("title"))),
            Some((SortTarget::Column("title"), false)),
        ),
        field(
            "description",
            Kind::Later("text search", Some(Target::Column("description"))),
            None,
        ),
        field(
            "status",
            Kind::Enumerated {
                target: Target::Column("status"),
                options: statuses.iter().map(|id| text(id)).collect(),
            },
            Some((SortTarget::Column("status"), false)),
        ),
        field("category", Kind::Category, None),
        field(
            "priority",
            Kind::Enumerated {
                target: Target::IntColumn("priority"),
                options: PRIORITY_RANKS
                    .iter()
                    .map(|(id, rank)| ((*id).to_owned(), Literal::Integer(*rank)))
                    .collect(),
            },
            Some((SortTarget::Rank("priority"), true)),
        ),
        field(
            "assignee",
            Kind::Person(Target::Column("assignee")),
            Some((SortTarget::Name("assignee"), false)),
        ),
        field(
            "reporter",
            Kind::Person(Target::Column("reporter")),
            Some((SortTarget::Name("reporter"), false)),
        ),
        field("parent", Kind::Issue(Target::Column("parent_id")), None),
        field("shortlist", Kind::Shortlist, None),
        field("key", Kind::Later("filtering by Key", None), None),
        field("agent", Kind::Later("filtering by Agent", None), None),
        field("claim", Kind::Later("filtering by Claim", None), None),
        field("labels", Kind::Labels, None),
        field(
            "archived",
            Kind::Boolean(Target::IntColumn("archived")),
            None,
        ),
        field(
            "created",
            Kind::Later("filtering by timestamp", None),
            Some((SortTarget::Column("created_ms"), true)),
        ),
        field(
            "updated",
            Kind::Later("filtering by timestamp", None),
            Some((SortTarget::Column("updated_ms"), true)),
        ),
    ];
    fields.extend(
        scheme
            .fields
            .iter()
            .filter(|def| !def.builtin)
            .map(scheme_field),
    );
    fields
}

fn scheme_field(def: &Field) -> Filterable {
    let name = def.name.clone();
    let options = || def.options.iter().map(|choice| choice.id.clone()).collect();
    let value = || Target::SchemeValue(name.clone());
    let (kind, sort) = match def.field_type {
        FieldType::Number => (
            Kind::Number(name.clone()),
            Some((SortTarget::SchemeNumber(name.clone()), false)),
        ),
        FieldType::Date => (
            Kind::Date(name.clone()),
            Some((SortTarget::SchemeText(name.clone()), true)),
        ),
        FieldType::SingleSelect => (
            Kind::Select(name.clone(), options()),
            Some((
                SortTarget::SchemeOrder {
                    field: name.clone(),
                    order: options(),
                },
                false,
            )),
        ),
        FieldType::MultiSelect => (Kind::MultiSelect(name.clone(), options()), None),
        FieldType::Boolean => (Kind::Boolean(value()), None),
        FieldType::Person => (
            Kind::Person(value()),
            Some((SortTarget::SchemeName(name.clone()), false)),
        ),
        FieldType::Datetime => (Kind::Later("filtering by timestamp", Some(value())), None),
        FieldType::Text | FieldType::RichText => (Kind::Later("text search", Some(value())), None),
        // `add_field` refuses these for Scheme fields.
        FieldType::Key
        | FieldType::Labels
        | FieldType::IssueType
        | FieldType::Status
        | FieldType::Priority
        | FieldType::IssueReference
        | FieldType::Shortlist
        | FieldType::Rank => (Kind::Later("this field type", Some(value())), None),
    };
    Filterable { name, kind, sort }
}

/// Checks `query` against `scheme`. A `parent:` value must already be an
/// Issue's id: the caller looks Keys up, as `Store::find` does.
///
/// # Errors
///
/// [`FilterError::UnknownField`] or [`FilterError::UnknownValue`] with the
/// valid choices, [`FilterError::InvalidValue`] for a value of the wrong
/// type, [`FilterError::Unsupported`] for a field that cannot be filtered or
/// sorted that way yet, or [`FilterError::NoIdentity`] for `@me` with no one
/// to resolve it to.
pub fn resolve(
    query: &Query,
    scheme: &Scheme,
    context: Context<'_>,
) -> Result<Resolved, FilterError> {
    let fields = filterable(scheme);
    let mut resolved = Resolved::default();
    for term in &query.terms {
        let condition = match term {
            Term::Match {
                field,
                negated,
                values,
            } => {
                let def = lookup(&fields, field)?;
                resolved.archived_named |= def.name == "archived";
                match_condition(def, values, *negated, scheme, context)?
            }
            Term::Has { field, present } => {
                let def = lookup(&fields, field)?;
                resolved.archived_named |= def.name == "archived";
                Condition::Present {
                    target: present_target(def, field.1)?,
                    present: *present,
                }
            }
        };
        resolved.conditions.push(condition);
    }
    for key in &query.sorts {
        let def = lookup(&fields, &key.field)?;
        let (target, natural_desc) = def.sort.clone().ok_or_else(|| FilterError::Unsupported {
            construct: format!("sorting by {}", def.name),
            span: key.field.1,
        })?;
        let descending = key
            .direction
            .map_or(natural_desc, |direction| direction == Direction::Descending);
        resolved.sorts.push(Sort { target, descending });
    }
    Ok(resolved)
}

fn lookup<'a>(
    fields: &'a [Filterable],
    (name, span): &(String, Span),
) -> Result<&'a Filterable, FilterError> {
    fields
        .iter()
        .find(|def| def.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            let names = fields.iter().map(|def| def.name.as_str());
            FilterError::UnknownField {
                field: name.clone(),
                span: *span,
                valid: names.clone().map(str::to_owned).collect(),
                did_you_mean: did_you_mean(name, names),
            }
        })
}

fn present_target(def: &Filterable, span: Span) -> Result<Target, FilterError> {
    match &def.kind {
        // Priority's `none` is its unset value; archived always holds one.
        Kind::Boolean(Target::IntColumn(_)) | Kind::Category | Kind::Later(_, None) => {
            Err(FilterError::Unsupported {
                construct: format!("has: and no: on {}, which always holds a value", def.name),
                span,
            })
        }
        Kind::Enumerated { target, .. }
        | Kind::Person(target)
        | Kind::Boolean(target)
        | Kind::Issue(target)
        | Kind::Later(_, Some(target)) => Ok(target.clone()),
        Kind::Number(field) | Kind::Date(field) | Kind::Select(field, _) => {
            Ok(Target::SchemeValue(field.clone()))
        }
        Kind::MultiSelect(field, _) => Ok(Target::Set(field.clone())),
        Kind::Labels => Ok(Target::Set("labels".into())),
        Kind::Shortlist => Ok(Target::Shortlist),
    }
}

fn match_condition(
    def: &Filterable,
    values: &[(Value, Span)],
    negated: bool,
    scheme: &Scheme,
    context: Context<'_>,
) -> Result<Condition, FilterError> {
    let target = match &def.kind {
        Kind::Enumerated { target, .. }
        | Kind::Person(target)
        | Kind::Boolean(target)
        | Kind::Issue(target) => target.clone(),
        Kind::Number(field) | Kind::Date(field) | Kind::Select(field, _) => {
            Target::SchemeValue(field.clone())
        }
        Kind::MultiSelect(field, _) => Target::Set(field.clone()),
        Kind::Labels => Target::Set("labels".into()),
        Kind::Shortlist => Target::Shortlist,
        Kind::Category => return category(def, values, negated, scheme),
        Kind::Later(construct, _) => {
            return Err(FilterError::Unsupported {
                construct: (*construct).to_owned(),
                span: values.first().map_or(Span(0, 0), |(_, span)| *span),
            });
        }
    };
    let values = values
        .iter()
        .map(|(value, span)| literal(def, value, *span, context))
        .collect::<Result<_, _>>()?;
    Ok(Condition::Match {
        target,
        values,
        negated,
    })
}

/// One value of a `field:value` term, as the store compares it.
fn literal(
    def: &Filterable,
    value: &Value,
    span: Span,
    context: Context<'_>,
) -> Result<Literal, FilterError> {
    let text = match (&def.kind, value) {
        (Kind::Person(_), Value::Me) => {
            return context
                .me
                .map(|me| Literal::Name(me.to_owned()))
                .ok_or(FilterError::NoIdentity { span });
        }
        (Kind::Issue(_), Value::Me) => {
            return Err(invalid(def, "@me", span, "an Issue's Key or id"));
        }
        (Kind::Shortlist, Value::Me) => {
            return Err(invalid(def, "@me", span, SHORTLIST_VALUE));
        }
        (_, Value::Me) => return Err(invalid(def, "@me", span, "a value, not a person")),
        (_, Value::Text(text)) if text.trim().is_empty() => {
            return Err(invalid(def, text, span, "a non-empty value"));
        }
        (_, Value::Text(text)) => text,
    };
    match &def.kind {
        Kind::Person(_) => Ok(Literal::Name(text.clone())),
        Kind::Enumerated { options, .. } => {
            let ids: Vec<String> = options.iter().map(|(id, _)| id.clone()).collect();
            let id = choose(def, text, span, &ids)?;
            Ok(options
                .iter()
                .find(|(option, _)| *option == id)
                .map_or(Literal::Text(id), |(_, literal)| literal.clone()))
        }
        Kind::Boolean(target) => {
            let flag = match text.to_ascii_lowercase().as_str() {
                "true" => 1,
                "false" => 0,
                _ => return Err(invalid(def, text, span, "true or false")),
            };
            Ok(match target {
                Target::IntColumn(_) => Literal::Integer(flag),
                _ => Literal::Number(f64::from(u8::from(flag == 1))),
            })
        }
        Kind::Number(_) => exact_number(text)
            .map(Literal::Number)
            .ok_or_else(|| invalid(def, text, span, "a number tm can store exactly")),
        Kind::Date(_) if is_date(text) => Ok(Literal::Text(text.clone())),
        Kind::Date(_) => Err(invalid(def, text, span, "a date written YYYY-MM-DD")),
        Kind::Select(_, ids) | Kind::MultiSelect(_, ids) => {
            Ok(Literal::Text(choose(def, text, span, ids)?))
        }
        Kind::Labels => Ok(Literal::Text(text.trim().to_lowercase())),
        Kind::Issue(_) => Uuid::parse_str(text.trim())
            .map(Literal::Issue)
            .map_err(|_| invalid(def, text, span, "an Issue's Key or id")),
        Kind::Shortlist => Uuid::parse_str(text.trim())
            .map(Literal::Shortlist)
            .map_err(|_| invalid(def, text, span, SHORTLIST_VALUE)),
        Kind::Category | Kind::Later(..) => Err(invalid(def, text, span, "a supported field")),
    }
}

const SHORTLIST_VALUE: &str = "a Short-list's name or id";

/// The id in `ids` that `text` names, ignoring case.
fn choose(def: &Filterable, text: &str, span: Span, ids: &[String]) -> Result<String, FilterError> {
    ids.iter()
        .find(|id| id.eq_ignore_ascii_case(text))
        .cloned()
        .ok_or_else(|| FilterError::UnknownValue {
            field: def.name.clone(),
            value: text.to_owned(),
            span,
            valid: ids.to_vec(),
            did_you_mean: did_you_mean(text, ids.iter().map(String::as_str)),
        })
}

fn invalid(def: &Filterable, value: &str, span: Span, expected: &str) -> FilterError {
    FilterError::InvalidValue {
        field: def.name.clone(),
        value: value.to_owned(),
        span,
        expected: expected.to_owned(),
    }
}

fn category(
    def: &Filterable,
    values: &[(Value, Span)],
    negated: bool,
    scheme: &Scheme,
) -> Result<Condition, FilterError> {
    let ids: Vec<String> = Category::ALL.iter().map(|c| c.id().to_owned()).collect();
    let mut chosen = Vec::new();
    for (value, span) in values {
        let Value::Text(text) = value else {
            return Err(FilterError::InvalidValue {
                field: def.name.clone(),
                value: "@me".into(),
                span: *span,
                expected: "a Status category".into(),
            });
        };
        let category = Category::ALL
            .into_iter()
            .find(|category| category.id().eq_ignore_ascii_case(text))
            .ok_or_else(|| FilterError::UnknownValue {
                field: def.name.clone(),
                value: text.clone(),
                span: *span,
                valid: ids.clone(),
                did_you_mean: did_you_mean(text, ids.iter().map(String::as_str)),
            })?;
        chosen.push(category);
    }
    let mut pairs = Vec::new();
    let mut known = Vec::new();
    for issue_type in &scheme.types {
        let Some(workflow) = scheme.workflow(&issue_type.id) else {
            continue;
        };
        for status in &workflow.statuses {
            let pair = (issue_type.id.clone(), status.id.clone());
            if chosen.contains(&status.category) {
                pairs.push(pair.clone());
            }
            known.push(pair);
        }
    }
    Ok(Condition::Category {
        pairs,
        known,
        include_unknown: chosen.contains(&Category::Triage),
        negated,
    })
}

/// A real calendar date in the one spelling the store keeps.
fn is_date(text: &str) -> bool {
    crate::scheme::is_date(text)
}

/// The largest integer an `f64` holds exactly; writes refuse anything larger.
const MAX_EXACT: f64 = 9_007_199_254_740_992.0;

/// A finite number within the range writes accept. It rounds to the nearest
/// `f64` exactly as a write of the same text would, so equal text matches.
fn exact_number(text: &str) -> Option<f64> {
    let integer = text.parse::<i128>().ok();
    let number = text.parse::<f64>().ok().filter(|n| n.is_finite())?;
    let in_range = match integer {
        Some(integer) => integer.unsigned_abs() <= 1 << 53,
        None => number.fract() != 0.0 || number.abs() < MAX_EXACT,
    };
    in_range.then_some(number)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::parse;
    use crate::scheme::Choice;

    fn scheme() -> Scheme {
        let mut scheme = Scheme::built_in();
        scheme
            .add_field(Field::custom(
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
            ))
            .unwrap();
        let tags = vec![Choice {
            id: "x".into(),
            label: "Ex".into(),
        }];
        for field in [
            Field::custom("area", "Area", FieldType::Number, Vec::new()),
            Field::custom("tags", "Tags", FieldType::MultiSelect, tags),
            Field::custom("flag", "Flag", FieldType::Boolean, Vec::new()),
            Field::custom("due", "Due", FieldType::Date, Vec::new()),
            Field::custom("owner", "Owner", FieldType::Person, Vec::new()),
        ] {
            scheme.add_field(field).unwrap();
        }
        scheme
    }

    fn run(filter: &str) -> Result<Resolved, FilterError> {
        resolve(
            &parse(filter).unwrap(),
            &scheme(),
            Context { me: Some("jace") },
        )
    }

    #[test]
    fn an_unknown_field_lists_every_valid_field() {
        let err = run("stauts:todo").unwrap_err();
        let FilterError::UnknownField {
            valid,
            did_you_mean,
            span,
            ..
        } = err
        else {
            panic!("{err:?}");
        };
        for field in ["status", "labels", "category", "priority", "tier", "area"] {
            assert!(
                valid.contains(&field.to_owned()),
                "{field} missing from {valid:?}"
            );
        }
        assert_eq!(did_you_mean.first().map(String::as_str), Some("status"));
        assert_eq!(span, Span(0, 6));
    }

    #[test]
    fn built_in_fields_without_a_filter_yet_say_so() {
        for filter in ["key:CPL-1", "agent:codex", "claim:x"] {
            let err = run(filter).unwrap_err();
            assert!(
                matches!(err, FilterError::Unsupported { .. }),
                "{filter}: {err:?}"
            );
        }
        for filter in ["has:archived", "no:created", "has:key", "has:category"] {
            let err = run(filter).unwrap_err();
            assert!(
                matches!(err, FilterError::Unsupported { .. }),
                "{filter}: {err:?}"
            );
        }
    }

    #[test]
    fn parent_takes_an_issue_id_and_refuses_anything_else() {
        let id = Uuid::from_u128(7);
        assert_eq!(
            run(&format!("-parent:{id}")).unwrap().conditions,
            [Condition::Match {
                target: Target::Column("parent_id"),
                values: vec![Literal::Issue(id)],
                negated: true,
            }]
        );
        for filter in ["parent:x", "parent:@me"] {
            let err = run(filter).unwrap_err();
            let FilterError::InvalidValue { expected, .. } = &err else {
                panic!("{filter}: {err:?}");
            };
            assert_eq!(expected, "an Issue's Key or id", "{filter}");
        }
    }

    #[test]
    fn did_you_mean_ignores_case_and_offers_at_most_three() {
        let err = run("STAUTS:todo").unwrap_err();
        let FilterError::UnknownField { did_you_mean, .. } = err else {
            panic!("{err:?}");
        };
        assert_eq!(did_you_mean.first().map(String::as_str), Some("status"));
        let err = run("s:todo").unwrap_err();
        let FilterError::UnknownField { did_you_mean, .. } = err else {
            panic!("{err:?}");
        };
        assert!(did_you_mean.len() <= 3, "{did_you_mean:?}");
    }

    #[test]
    fn has_and_no_read_where_each_kind_is_stored() {
        let cases = [
            ("has:assignee", Target::Column("assignee")),
            ("has:priority", Target::IntColumn("priority")),
            ("has:tier", Target::SchemeValue("tier".into())),
            ("has:area", Target::SchemeValue("area".into())),
            ("has:labels", Target::Set("labels".into())),
            ("has:tags", Target::Set("tags".into())),
            ("has:title", Target::Column("title")),
            ("has:parent", Target::Column("parent_id")),
        ];
        for (filter, target) in cases {
            assert_eq!(
                run(filter).unwrap().conditions,
                [Condition::Present {
                    target,
                    present: true
                }],
                "{filter}"
            );
        }
    }

    #[test]
    fn enumerated_and_boolean_values_ignore_case() {
        let resolved = run("Type:BUG archived:FALSE flag:True").unwrap();
        assert_eq!(
            resolved.conditions,
            [
                Condition::Match {
                    target: Target::Column("type"),
                    values: vec![Literal::Text("bug".into())],
                    negated: false,
                },
                Condition::Match {
                    target: Target::IntColumn("archived"),
                    values: vec![Literal::Integer(0)],
                    negated: false,
                },
                Condition::Match {
                    target: Target::SchemeValue("flag".into()),
                    values: vec![Literal::Number(1.0)],
                    negated: false,
                },
            ]
        );
    }

    #[test]
    fn blank_and_unrepresentable_values_are_refused() {
        for filter in [
            r#"assignee:"""#,
            r#"labels:"  ""#,
            "area:NaN",
            "area:inf",
            "area:9007199254740993",
            "due:2026-02-31",
            "due:x",
        ] {
            let err = run(filter).unwrap_err();
            assert!(
                matches!(err, FilterError::InvalidValue { .. }),
                "{filter}: {err:?}"
            );
        }
    }

    #[test]
    fn value_errors_point_at_the_value() {
        let err = run("status:todo,reveiw").unwrap_err();
        let FilterError::UnknownValue { span, .. } = err else {
            panic!("{err:?}");
        };
        assert_eq!(span, Span(12, 18));
    }

    #[test]
    fn the_unknown_field_error_serializes_for_agents() {
        let err = run("stauts:todo").unwrap_err();
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["error"], "unknown_field");
        assert_eq!(json["span"], serde_json::json!([0, 6]));
        assert!(json["valid"].as_array().unwrap().contains(&"status".into()));
    }

    #[test]
    fn an_unknown_value_lists_the_options() {
        let err = run("status:reveiw").unwrap_err();
        let FilterError::UnknownValue {
            valid,
            did_you_mean,
            ..
        } = err
        else {
            panic!("{err:?}");
        };
        assert!(valid.contains(&"in_review".to_owned()));
        assert_eq!(did_you_mean, ["in_review"]);
    }

    #[test]
    fn names_and_enumerated_values_ignore_case() {
        let resolved = run("STATUS:In_Review Tier:GOLD labels:UI priority:High").unwrap();
        assert_eq!(
            resolved.conditions,
            [
                Condition::Match {
                    target: Target::Column("status"),
                    values: vec![Literal::Text("in_review".into())],
                    negated: false,
                },
                Condition::Match {
                    target: Target::SchemeValue("tier".into()),
                    values: vec![Literal::Text("gold".into())],
                    negated: false,
                },
                Condition::Match {
                    target: Target::Set("labels".into()),
                    values: vec![Literal::Text("ui".into())],
                    negated: false,
                },
                Condition::Match {
                    target: Target::IntColumn("priority"),
                    values: vec![Literal::Integer(3)],
                    negated: false,
                },
            ]
        );
    }

    #[test]
    fn at_me_is_the_human_asking() {
        let resolved = run("assignee:@me").unwrap();
        assert_eq!(
            resolved.conditions[0],
            Condition::Match {
                target: Target::Column("assignee"),
                values: vec![Literal::Name("jace".into())],
                negated: false,
            }
        );
        assert_eq!(
            run("owner:@ME").unwrap().conditions[0],
            Condition::Match {
                target: Target::SchemeValue("owner".into()),
                values: vec![Literal::Name("jace".into())],
                negated: false,
            }
        );
        let err = resolve(
            &parse("assignee:@me").unwrap(),
            &scheme(),
            Context::default(),
        )
        .unwrap_err();
        assert!(matches!(err, FilterError::NoIdentity { .. }), "{err:?}");
        let err = run("status:@me").unwrap_err();
        assert!(matches!(err, FilterError::InvalidValue { .. }), "{err:?}");
    }

    #[test]
    fn values_of_the_wrong_type_are_refused() {
        for filter in ["area:many", "archived:maybe"] {
            let err = run(filter).unwrap_err();
            assert!(
                matches!(err, FilterError::InvalidValue { .. }),
                "{filter}: {err:?}"
            );
        }
    }

    #[test]
    fn text_and_date_filters_wait_for_later_phases() {
        for filter in ["title:crash", "created:2026-01-01", "description:x"] {
            let err = run(filter).unwrap_err();
            assert!(
                matches!(err, FilterError::Unsupported { .. }),
                "{filter}: {err:?}"
            );
        }
        assert!(run("has:title no:description").is_ok());
    }

    #[test]
    fn a_category_covers_its_statuses_and_triage_covers_unknown_ones() {
        let resolved = run("category:active").unwrap();
        let Condition::Category {
            pairs,
            include_unknown,
            ..
        } = &resolved.conditions[0]
        else {
            panic!("{resolved:?}");
        };
        assert!(pairs.contains(&("bug".into(), "in_review".into())));
        assert!(!pairs.iter().any(|(_, status)| status == "todo"));
        assert!(!include_unknown);
        let resolved = run("category:Triage").unwrap();
        assert!(matches!(
            resolved.conditions[0],
            Condition::Category {
                include_unknown: true,
                ..
            }
        ));
    }

    #[test]
    fn sorts_take_natural_directions_unless_told() {
        let resolved = run("sort:priority,created,title,tier-desc").unwrap();
        let sorts: Vec<bool> = resolved.sorts.iter().map(|sort| sort.descending).collect();
        assert_eq!(sorts, [true, true, false, true]);
        assert!(matches!(
            &resolved.sorts[3].target,
            SortTarget::SchemeOrder { order, .. } if order == &["gold", "free"]
        ));
        let err = run("sort:labels").unwrap_err();
        assert!(matches!(err, FilterError::Unsupported { .. }), "{err:?}");
    }

    #[test]
    fn naming_archived_lifts_the_default() {
        assert!(!run("status:todo").unwrap().archived_named);
        assert!(run("archived:true").unwrap().archived_named);
        assert!(run("-archived:true").unwrap().archived_named);
    }

    #[test]
    fn dates_must_be_real_and_canonical() {
        assert!(is_date("2026-10-17"));
        assert!(is_date("2024-02-29"));
        for bad in [
            "2026-13-01",
            "2026-1-01",
            "17/10/2026",
            "2026-10-00",
            "2026-02-31",
            "2026-10-32",
            "20261017",
            "2026-10-17T00:00",
            "-000001-01-01",
            "+002026-10-17",
        ] {
            assert!(!is_date(bad), "{bad}");
        }
    }

    #[test]
    fn numbers_must_compare_exactly() {
        assert_eq!(exact_number("3"), Some(3.0));
        assert_eq!(exact_number("-2.5"), Some(-2.5));
        assert_eq!(exact_number("9007199254740992"), Some(MAX_EXACT));
        for bad in [
            "9007199254740993",
            "1e16",
            "9007199254740993e0",
            "NaN",
            "inf",
            "x",
        ] {
            assert_eq!(exact_number(bad), None, "{bad}");
        }
    }
}
