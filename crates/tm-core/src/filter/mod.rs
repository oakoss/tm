//! The filter language: `key:value` qualifiers, lists, negation, `no:`,
//! `has:`, `@me` and `sort:`. [`parse`] turns text into a [`Query`] without
//! the Scheme; [`resolve`] checks it against the Scheme and returns typed
//! conditions a store can compile.

mod parse;
mod resolve;
mod suggest;

use serde::Serialize;

pub use parse::parse;
pub use resolve::{Condition, Context, Literal, Resolved, Sort, SortTarget, Target, resolve};

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
    #[error("{word:?} is not a qualifier; write field:value")]
    BareWord { word: String, span: Span },
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
    #[error("no Issue is {value}")]
    NoIssue { value: String, span: Span },
    #[error("no Short-list is named {value}")]
    NoShortlist { value: String, span: Span },
    /// A merge left the name on several lists in one Project (ADR 0016).
    #[error("{count} Short-lists in one Project are named {value}; name one by its id")]
    ShortlistNameShared {
        value: String,
        span: Span,
        count: usize,
    },
}
