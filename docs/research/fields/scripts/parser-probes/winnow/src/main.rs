use filter_ast::{ParseError, Query, Term, Value, finish, report};
use winnow::{
    LocatingSlice, ModalResult as Result,
    combinator::{alt, cut_err, delimited, opt, preceded, repeat, separated},
    error::{ContextError, StrContext, StrContextValue},
    prelude::*,
    token::{none_of, one_of, take_while},
};

type In<'i> = LocatingSlice<&'i str>;

fn is_name(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}

fn is_bare(c: char) -> bool {
    !c.is_whitespace() && !matches!(c, ',' | '(' | ')' | '"' | ':' | '<' | '>' | '=')
}

fn expected(what: &'static str) -> StrContext {
    StrContext::Expected(StrContextValue::Description(what))
}

fn quoted(input: &mut In<'_>) -> Result<String> {
    preceded(
        '"',
        cut_err(
            (
                repeat(
                    0..,
                    alt((preceded('\\', one_of(|_| true)), none_of(['"', '\\']))),
                ),
                '"',
            )
                .map(|(s, _): (String, char)| s),
        )
        .context(StrContext::Label("quoted value"))
        .context(expected("closing `\"`")),
    )
    .parse_next(input)
}

fn value(input: &mut In<'_>) -> Result<(Value, std::ops::Range<usize>)> {
    alt((
        quoted.map(Value::Text),
        preceded('@', take_while(1.., is_name)).map(|s: &str| Value::Function(s.to_owned())),
        take_while(1.., is_bare).map(|s: &str| Value::Text(s.to_owned())),
    ))
    .context(expected("a value"))
    .with_span()
    .parse_next(input)
}

fn term(input: &mut In<'_>) -> Result<Term> {
    let negated = opt('-').parse_next(input)?.is_some();
    let (field, field_span) = take_while(1.., is_name)
        .context(expected("a field name"))
        .with_span()
        .parse_next(input)?;
    ':'.context(expected("`:` after the field name"))
        .parse_next(input)?;
    let values: Vec<_> = cut_err(separated(1.., value, ',')).parse_next(input)?;
    Ok(Term {
        negated,
        field: field.to_owned(),
        field_span,
        values,
    })
}

fn query(input: &mut In<'_>) -> Result<Vec<Term>> {
    let ws = take_while(0.., char::is_whitespace);
    delimited(
        ws,
        separated(0.., term, take_while(1.., char::is_whitespace)),
        take_while(0.., char::is_whitespace),
    )
    .parse_next(input)
}

fn parse(src: &str) -> std::result::Result<Query, ParseError> {
    let terms = query.parse(LocatingSlice::new(src)).map_err(|e| {
        let inner: &ContextError = e.inner();
        ParseError {
            code: "syntax",
            message: inner.to_string(),
            span: e.char_span(),
        }
    })?;
    finish(terms)
}

fn main() {
    report("winnow 1.0.4", parse);
}
