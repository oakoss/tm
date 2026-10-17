//! The first-cut filter AST and error shape every probe parser produces, and
//! the inputs they all run.

use std::ops::Range;

pub type Span = Range<usize>;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(String),
    /// `@me`, `@current`; arguments are reserved for `@next-release(x)`.
    Function(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Term {
    pub negated: bool,
    pub field: String,
    pub field_span: Span,
    pub values: Vec<(Value, Span)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SortKey {
    pub field: String,
    pub descending: bool,
    pub span: Span,
}

/// Terms are ANDed; `OR` and parentheses will wrap terms in a tree later.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    pub terms: Vec<Term>,
    pub sort: Vec<SortKey>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub code: &'static str,
    pub message: String,
    pub span: Span,
}

pub const CASES: &[&str] = &[
    "status:review,testing -type:spike",
    "label:\"good first issue\" assignee:@me sort:updated-desc",
    "area:cli label:bug -label:wontfix",
    "status:",
    "label:\"open",
    "status:review,",
    ":foo",
    "fix login",
    "status:review OR status:testing",
    "(status:review)",
    "priority:>=high",
    "iteration:@next(cli)",
];

pub fn report(name: &str, parse: impl Fn(&str) -> Result<Query, ParseError>) {
    println!("## {name}");
    for input in CASES {
        match parse(input) {
            Ok(q) => {
                let terms: Vec<String> = q
                    .terms
                    .iter()
                    .map(|t| {
                        let vals: Vec<String> =
                            t.values.iter().map(|(v, _)| format!("{v:?}")).collect();
                        format!(
                            "{}{}={}",
                            if t.negated { "-" } else { "" },
                            t.field,
                            vals.join("|")
                        )
                    })
                    .collect();
                let sort: Vec<String> = q
                    .sort
                    .iter()
                    .map(|s| format!("{}{}", s.field, if s.descending { " desc" } else { "" }))
                    .collect();
                println!(
                    "ok   {input:<55} terms [{}] sort [{}]",
                    terms.join(", "),
                    sort.join(", ")
                );
            }
            Err(e) => {
                println!("err  {input:<55} {} at {:?}: {}", e.code, e.span, e.message);
                println!("     {input}");
                let width = e.span.end.saturating_sub(e.span.start).max(1);
                println!("     {}{}", " ".repeat(e.span.start), "^".repeat(width));
            }
        }
    }
}

/// Shared post-processing: `sort:` terms become sort keys.
pub fn finish(terms: Vec<Term>) -> Result<Query, ParseError> {
    let mut query = Query::default();
    for term in terms {
        if term.field != "sort" {
            query.terms.push(term);
            continue;
        }
        for (value, span) in term.values {
            let Value::Text(text) = value else {
                return Err(ParseError {
                    code: "invalid_sort",
                    message: "sort takes a field name".into(),
                    span,
                });
            };
            let (field, descending) = match text.rsplit_once('-') {
                Some((f, "desc")) => (f.to_owned(), true),
                Some((f, "asc")) => (f.to_owned(), false),
                _ => (text, false),
            };
            query.sort.push(SortKey {
                field,
                descending,
                span,
            });
        }
    }
    Ok(query)
}
