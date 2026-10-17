use filter_ast::{ParseError, Query, Span, Term, Value, finish, report};
use logos::Logos;

#[derive(Logos, Debug, Clone, PartialEq)]
enum Tok<'s> {
    #[regex(r"\s+")]
    Ws,
    #[token(":")]
    Colon,
    #[token(",")]
    Comma,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[regex(r#""([^"\\]|\\.)*""#, |lex| lex.slice())]
    Quoted(&'s str),
    #[regex(r#"[^\s,():"]+"#, |lex| lex.slice())]
    Word(&'s str),
}

fn err(code: &'static str, message: impl Into<String>, span: Span) -> ParseError {
    ParseError {
        code,
        message: message.into(),
        span,
    }
}

fn parse(src: &str) -> Result<Query, ParseError> {
    let mut toks = Vec::new();
    for (tok, span) in Tok::lexer(src).spanned() {
        match tok {
            Ok(Tok::Ws) => {}
            Ok(t) => toks.push((t, span)),
            Err(()) => return Err(err("lex", "unrecognised input", span)),
        }
    }
    let mut i = 0;
    let mut terms = Vec::new();
    while i < toks.len() {
        let (tok, span) = &toks[i];
        let Tok::Word(word) = tok else {
            return Err(err(
                "unexpected",
                format!("unexpected {tok:?}"),
                span.clone(),
            ));
        };
        let (negated, field, field_span) = match word.strip_prefix('-') {
            Some(rest) => (true, rest, span.start + 1..span.end),
            None => (false, *word, span.clone()),
        };
        match toks.get(i + 1) {
            Some((Tok::Colon, c)) if c.start == span.end => {}
            _ if matches!(*word, "OR" | "AND" | "NOT") => {
                return Err(err(
                    "unsupported",
                    format!("`{word}` is not supported yet"),
                    span.clone(),
                ));
            }
            _ => {
                return Err(err(
                    "bare_word",
                    "full-text search is not supported yet",
                    span.clone(),
                ));
            }
        }
        i += 2;
        let mut values = Vec::new();
        loop {
            match toks.get(i) {
                Some((Tok::Quoted(q), s)) => values.push((
                    Value::Text(q[1..q.len() - 1].replace("\\\"", "\"")),
                    s.clone(),
                )),
                Some((Tok::Word(w), s)) if w.starts_with('@') => {
                    values.push((Value::Function(w[1..].to_owned()), s.clone()))
                }
                Some((Tok::Word(w), s)) => values.push((Value::Text((*w).to_owned()), s.clone())),
                Some((t, s)) => {
                    return Err(err(
                        "expected_value",
                        format!("expected a value, found {t:?}"),
                        s.clone(),
                    ));
                }
                None => {
                    return Err(err(
                        "expected_value",
                        "expected a value",
                        src.len()..src.len(),
                    ));
                }
            }
            i += 1;
            if matches!(toks.get(i), Some((Tok::Comma, _))) {
                i += 1;
            } else {
                break;
            }
        }
        terms.push(Term {
            negated,
            field: field.to_owned(),
            field_span,
            values,
        });
    }
    finish(terms)
}

fn main() {
    report("logos 0.16.1 + hand parser", parse);
}
