//! The filter language: `key:value` qualifiers, lists, negation, `no:`,
//! `has:`, `is:`, `@me` and `sort:`. [`parse`] turns text into a [`Query`] without
//! the Scheme; [`resolve`] checks it against the Scheme and returns typed
//! conditions a store can compile.

mod parse;
mod resolve;
mod suggest;

use serde::Serialize;

use crate::project::ProjectId;

pub use parse::parse;
pub use resolve::{
    Condition, Context, IssueField, Literal, Resolved, Sort, SortTarget, Target, resolve,
    takes_issue,
};

/// The open Issues: not done or cancelled.
pub const OPEN: &str = "-category:done,cancelled";

/// `filter` narrowed to the open work, unless `all` is asked for, the filter
/// names a state, which the default would contradict, or it does not parse,
/// so its own error is reported. The default goes last so error spans still
/// point into what was typed.
#[must_use]
pub fn open_unless_named(filter: &str, all: bool) -> String {
    match parse(filter) {
        Ok(query) if !all && !names_state(&query) => format!("{filter} {OPEN}"),
        _ => filter.to_owned(),
    }
}

/// Whether `filter` parses and names a status, category or archived, so it
/// may admit finished work.
#[must_use]
pub fn names_a_state(filter: &str) -> bool {
    parse(filter).is_ok_and(|query| names_state(&query))
}

/// `filter` narrowed to `project`'s Issues unless it names a Project or does
/// not parse, as [`open_unless_named`] narrows to the open work.
#[must_use]
pub fn in_project_unless_named(filter: &str, project: ProjectId) -> String {
    match parse(filter) {
        Ok(query) if !names(&query, &["project"]) => format!("{filter} project:{project}"),
        _ => filter.to_owned(),
    }
}

/// Whether a term asks about status, category or archived.
fn names_state(query: &Query) -> bool {
    names(query, &["status", "category", "archived"])
}

fn names(query: &Query, fields: &[&str]) -> bool {
    query.terms.iter().any(|term| {
        let (Term::Match { field, .. } | Term::Has { field, .. }) = term;
        fields.iter().any(|name| same_field(&field.0, name))
    })
}

/// Whether two field names are the same, ignoring case and reading `-` as
/// `_`, since Scheme field names never hold `-`.
#[must_use]
pub fn same_field(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes().zip(b.bytes()).all(|(a, b)| {
            let fold = |c: u8| {
                if c == b'-' {
                    b'_'
                } else {
                    c.to_ascii_lowercase()
                }
            };
            fold(a) == fold(b)
        })
}

/// A byte range in the filter text, serialized as `[start, end]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Span(pub usize, pub usize);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    /// All must hold.
    pub terms: Vec<Term>,
    pub sorts: Vec<SortKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    /// `field:a,b` holds when the field holds any listed value; negated, when
    /// it holds none of them, including when it holds nothing.
    Match {
        field: (String, Span),
        negated: bool,
        values: Vec<(Value, Span)>,
    },
    /// `has:field` or `no:field`.
    Has {
        field: (String, Span),
        present: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Text(String),
    /// `@me`: the human the filter runs for.
    Me,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortKey {
    pub field: (String, Span),
    /// `None` takes the field's natural direction.
    pub direction: Option<Direction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Ascending,
    Descending,
}

/// Every error names the span it concerns, so a UI can underline it.
#[derive(Debug, Clone, PartialEq, Serialize, thiserror::Error)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum FilterError {
    #[error("{message}")]
    Syntax { message: String, span: Span },
    #[error("not supported yet: {construct}")]
    Unsupported { construct: String, span: Span },
    #[error("unknown field {field:?}; valid fields: {}", valid.join(", "))]
    UnknownField {
        field: String,
        span: Span,
        valid: Vec<String>,
        did_you_mean: Vec<String>,
    },
    #[error("{field} has no value {value:?}; valid: {}", valid.join(", "))]
    UnknownValue {
        field: String,
        value: String,
        span: Span,
        valid: Vec<String>,
        did_you_mean: Vec<String>,
    },
    #[error("{field} needs {expected}, not {value:?}")]
    InvalidValue {
        field: String,
        value: String,
        span: Span,
        expected: String,
    },
    #[error("@me needs to know who is asking")]
    NoIdentity { span: Span },
    #[error("{field}: no Issue is {value}")]
    NoIssue {
        field: String,
        value: String,
        span: Span,
    },
    #[error("no Short-list is named {value}")]
    NoShortlist { value: String, span: Span },
    #[error("no Project is {value}")]
    NoProject { value: String, span: Span },
    /// A merge left the name on several lists in one Project (ADR 0016).
    #[error("{count} Short-lists in one Project are named {value}; name one by its id")]
    ShortlistNameShared {
        value: String,
        span: Span,
        count: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_open_default_yields_to_all_a_named_state_and_a_parse_error() {
        assert_eq!(
            open_unless_named("label:x", false),
            format!("label:x {OPEN}")
        );
        assert_eq!(open_unless_named("", false), format!(" {OPEN}"));
        assert_eq!(open_unless_named("label:x", true), "label:x");
        for named in ["Status:todo", "-category:done", "has:archived"] {
            assert_eq!(open_unless_named(named, false), named);
        }
        assert_eq!(open_unless_named("status:", false), "status:");
    }

    #[test]
    fn the_project_default_yields_to_a_named_project_and_a_parse_error() {
        let project = ProjectId::from_uuid(uuid::Uuid::nil());
        assert_eq!(
            in_project_unless_named("label:x", project),
            format!("label:x project:{project}")
        );
        for named in ["Project:WEB", "-project:TM", "has:project", "status:"] {
            assert_eq!(in_project_unless_named(named, project), named);
        }
    }
}
