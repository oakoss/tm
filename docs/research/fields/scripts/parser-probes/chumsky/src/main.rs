use chumsky::prelude::*;
use filter_ast::{ParseError, Query, Term, Value, finish, report};

fn is_name(c: &char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}

fn is_bare(c: &char) -> bool {
    !c.is_whitespace() && !matches!(c, ',' | '(' | ')' | '"' | ':' | '<' | '>' | '=')
}

fn parser<'src>() -> impl Parser<'src, &'src str, Vec<Term>, extra::Err<Rich<'src, char>>> {
    let name = any().filter(is_name).repeated().at_least(1).to_slice();
    let quoted = none_of("\\\"")
        .or(just('\\').ignore_then(any()))
        .repeated()
        .collect::<String>()
        .delimited_by(just('"'), just('"'))
        .labelled("quoted value");
    let value = choice((
        quoted.map(Value::Text),
        just('@')
            .ignore_then(name)
            .map(|s: &str| Value::Function(s.to_owned())),
        any()
            .filter(is_bare)
            .repeated()
            .at_least(1)
            .to_slice()
            .map(|s: &str| Value::Text(s.to_owned())),
    ))
    .labelled("value")
    .map_with(|v, e| {
        let span: SimpleSpan = e.span();
        (v, span.into_range())
    });
    let term = just('-')
        .or_not()
        .then(name.map_with(|s: &str, e| {
            let span: SimpleSpan = e.span();
            (s.to_owned(), span.into_range())
        }))
        .then_ignore(just(':'))
        .then(
            value
                .separated_by(just(','))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .map(|((neg, (field, field_span)), values)| Term {
            negated: neg.is_some(),
            field,
            field_span,
            values,
        });
    term.separated_by(text::whitespace().at_least(1))
        .collect()
        .padded()
        .then_ignore(end())
}

fn parse(src: &str) -> Result<Query, ParseError> {
    let terms = parser().parse(src).into_result().map_err(|errs| {
        let e = &errs[0];
        ParseError {
            code: "syntax",
            message: e.to_string(),
            span: e.span().into_range(),
        }
    })?;
    finish(terms)
}

fn main() {
    report("chumsky 0.12.0", parse);
}
