//! Filter text to a [`Query`], with byte spans for every error. Knows nothing
//! about the Scheme; names and values are checked by the resolver.

use super::{Direction, FilterError, Query, SortKey, Span, Term, Value};

/// Parses a filter. Constructs reserved for later (bare words, `OR`, `AND`,
/// `NOT`, parentheses, comparisons, functions other than `@me`) are errors
/// now, so nothing accepted today changes meaning when they arrive.
///
/// # Errors
///
/// The first [`FilterError`] in the text.
pub fn parse(input: &str) -> Result<Query, FilterError> {
    let mut query = Query::default();
    let mut pos = 0;
    while let Some(start) = skip_space(input, pos) {
        let (term, end) = term(input, start)?;
        match term {
            Parsed::Term(term) => query.terms.push(term),
            Parsed::Sort(keys) => query.sorts.extend(keys),
        }
        pos = end;
    }
    Ok(query)
}

enum Parsed {
    Term(Term),
    Sort(Vec<SortKey>),
}

fn skip_space(input: &str, pos: usize) -> Option<usize> {
    input[pos..]
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(offset, _)| pos + offset)
}

/// The end of a run of characters that are not whitespace.
fn token_end(input: &str, start: usize) -> usize {
    input[start..]
        .char_indices()
        .find(|(_, c)| c.is_whitespace())
        .map_or(input.len(), |(offset, _)| start + offset)
}

fn term(input: &str, start: usize) -> Result<(Parsed, usize), FilterError> {
    let mut pos = start;
    let negated = input[pos..].starts_with('-');
    if negated {
        pos += 1;
    }
    let first = input[pos..].chars().next();
    if matches!(first, Some('(' | ')')) {
        return Err(unsupported("parentheses", pos, pos + 1));
    }
    let name_end = input[pos..]
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_'))
        .map_or(input.len(), |(offset, _)| pos + offset);
    let name = &input[pos..name_end];
    if name.is_empty() || !input[name_end..].starts_with(':') {
        let end = token_end(input, start);
        let word = &input[start..end];
        if ["OR", "AND", "NOT"].contains(&word) {
            return Err(unsupported("boolean operators", start, end));
        }
        return Err(FilterError::BareWord {
            word: word.to_owned(),
            span: Span(start, end),
        });
    }
    let name_span = Span(pos, name_end);
    let values_start = name_end + 1;
    let (values, end) = values(input, values_start)?;
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "sort" => {
            if negated {
                return Err(unsupported("negated sort", start, end));
            }
            Ok((Parsed::Sort(sort_keys(values)?), end))
        }
        "no" | "has" => {
            if negated {
                return Err(unsupported("negated no: or has:", start, end));
            }
            let [(Value::Text(field), span)] = values.as_slice() else {
                return Err(FilterError::Syntax {
                    message: format!("{lower}: takes one field name"),
                    span: Span(values_start, end),
                });
            };
            Ok((
                Parsed::Term(Term::Has {
                    field: (field.clone(), *span),
                    present: lower == "has",
                }),
                end,
            ))
        }
        _ => Ok((
            Parsed::Term(Term::Match {
                field: (name.to_owned(), name_span),
                negated,
                values,
            }),
            end,
        )),
    }
}

/// A comma-separated list of values starting at `start`.
fn values(input: &str, start: usize) -> Result<(Vec<(Value, Span)>, usize), FilterError> {
    let mut values = Vec::new();
    let mut pos = start;
    loop {
        let (value, span, end) = value(input, pos)?;
        values.push((value, span));
        pos = end;
        if input[pos..].starts_with(',') {
            pos += 1;
        } else {
            return Ok((values, pos));
        }
    }
}

/// One value: the value, the span of its text (inside any quotes), and where
/// the next token starts.
fn value(input: &str, start: usize) -> Result<(Value, Span, usize), FilterError> {
    let rest = &input[start..];
    if let Some(quoted) = rest.strip_prefix('"') {
        let close = quoted.find('"').ok_or(FilterError::Syntax {
            message: "unterminated quote".into(),
            span: Span(start, input.len()),
        })?;
        let end = start + 1 + close + 1;
        // Text glued to the closing quote would otherwise start a new term
        // or extend this one, either of which an escape syntax could change.
        if input[end..]
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace() && c != ',')
        {
            return Err(FilterError::Syntax {
                message: "a quoted value must be followed by a space, a comma or the end".into(),
                span: Span(start, token_end(input, end)),
            });
        }
        let span = Span(start + 1, start + 1 + close);
        return Ok((Value::Text(quoted[..close].to_owned()), span, end));
    }
    let end = rest
        .char_indices()
        .find(|(_, c)| c.is_whitespace() || *c == ',')
        .map_or(input.len(), |(offset, _)| start + offset);
    let text = &input[start..end];
    let span = Span(start, end);
    if text.is_empty() {
        return Err(FilterError::Syntax {
            message: "expected a value".into(),
            span,
        });
    }
    if text.contains('"') {
        return Err(FilterError::Syntax {
            message: "quotes must surround the whole value".into(),
            span,
        });
    }
    if text.starts_with(['>', '<', '=']) || text.contains("..") {
        return Err(unsupported("comparisons", start, end));
    }
    if text.contains(['(', ')']) {
        return Err(unsupported("parentheses", start, end));
    }
    if text.contains('*') {
        return Err(unsupported("wildcards", start, end));
    }
    if let Some(function) = text.strip_prefix('@') {
        return if function.eq_ignore_ascii_case("me") {
            Ok((Value::Me, span, end))
        } else {
            Err(unsupported("functions other than @me", start, end))
        };
    }
    Ok((Value::Text(text.to_owned()), span, end))
}

fn sort_keys(values: Vec<(Value, Span)>) -> Result<Vec<SortKey>, FilterError> {
    values
        .into_iter()
        .map(|(value, span)| {
            let Value::Text(text) = value else {
                return Err(FilterError::Syntax {
                    message: "sort: takes field names".into(),
                    span,
                });
            };
            let suffix = text
                .rsplit_once('-')
                .map(|(field, suffix)| (field, suffix.to_ascii_lowercase()));
            let (field, direction) = match suffix.as_ref().map(|(f, s)| (*f, s.as_str())) {
                Some((field, "asc")) => (field, Some(Direction::Ascending)),
                Some((field, "desc")) => (field, Some(Direction::Descending)),
                _ => (text.as_str(), None),
            };
            Ok(SortKey {
                field: (field.to_owned(), Span(span.0, span.0 + field.len())),
                direction,
            })
        })
        .collect()
}

fn unsupported(construct: &str, start: usize, end: usize) -> FilterError {
    FilterError::Unsupported {
        construct: construct.to_owned(),
        span: Span(start, end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(query: &Query) -> Vec<(String, bool, Vec<Value>)> {
        query
            .terms
            .iter()
            .map(|term| match term {
                Term::Match {
                    field,
                    negated,
                    values,
                } => (
                    field.0.clone(),
                    *negated,
                    values.iter().map(|(value, _)| value.clone()).collect(),
                ),
                Term::Has { .. } => panic!("not a match term"),
            })
            .collect()
    }

    fn text(value: &str) -> Value {
        Value::Text(value.into())
    }

    #[test]
    fn a_qualifier_names_a_field_and_a_value() {
        let query = parse("status:todo").unwrap();
        assert_eq!(
            matches(&query),
            [("status".into(), false, vec![text("todo")])]
        );
    }

    #[test]
    fn a_comma_list_holds_several_values() {
        let query = parse("status:todo,in_review").unwrap();
        assert_eq!(matches(&query)[0].2, [text("todo"), text("in_review")]);
    }

    #[test]
    fn a_leading_dash_negates() {
        let query = parse("-type:spike").unwrap();
        assert!(matches(&query)[0].1);
    }

    #[test]
    fn terms_are_separated_by_whitespace() {
        let query = parse("  type:bug \t labels:ui  ").unwrap();
        assert_eq!(query.terms.len(), 2);
    }

    #[test]
    fn quotes_hold_spaces_commas_and_colons() {
        let query = parse(r#"area:"a b,c:d""#).unwrap();
        assert_eq!(matches(&query)[0].2, [text("a b,c:d")]);
    }

    #[test]
    fn at_me_is_a_value() {
        let query = parse("assignee:@me").unwrap();
        assert_eq!(matches(&query)[0].2, [Value::Me]);
    }

    #[test]
    fn no_and_has_name_a_field() {
        let query = parse("no:assignee has:labels").unwrap();
        let present: Vec<(String, bool)> = query
            .terms
            .iter()
            .map(|term| match term {
                Term::Has { field, present } => (field.0.clone(), *present),
                Term::Match { .. } => panic!("not a has term"),
            })
            .collect();
        assert_eq!(
            present,
            [("assignee".into(), false), ("labels".into(), true)]
        );
    }

    #[test]
    fn sort_takes_fields_with_an_optional_direction() {
        let query = parse("sort:priority,created-asc,updated-desc").unwrap();
        let keys: Vec<(String, Option<Direction>)> = query
            .sorts
            .iter()
            .map(|key| (key.field.0.clone(), key.direction))
            .collect();
        assert_eq!(
            keys,
            [
                ("priority".into(), None),
                ("created".into(), Some(Direction::Ascending)),
                ("updated".into(), Some(Direction::Descending)),
            ]
        );
    }

    #[test]
    fn an_empty_filter_matches_everything() {
        assert_eq!(parse("   ").unwrap(), Query::default());
    }

    #[test]
    fn reserved_constructs_are_refused_with_their_name_and_span() {
        let cases = [
            ("type:bug OR type:task", "boolean operators", "OR"),
            ("type:bug AND type:task", "boolean operators", "AND"),
            ("NOT type:bug", "boolean operators", "NOT"),
            ("(type:bug)", "parentheses", "("),
            ("type:a)", "parentheses", "a)"),
            ("priority:>=high", "comparisons", ">=high"),
            ("priority:<high", "comparisons", "<high"),
            ("priority:<=high", "comparisons", "<=high"),
            ("labels:=x", "comparisons", "=x"),
            (
                "created:2026-01-01..2026-02-01",
                "comparisons",
                "2026-01-01..2026-02-01",
            ),
            ("assignee:@agent", "functions other than @me", "@agent"),
            ("labels:ui*", "wildcards", "ui*"),
            ("-sort:priority", "negated sort", "-sort:priority"),
            ("-no:assignee", "negated no: or has:", "-no:assignee"),
            ("-has:labels", "negated no: or has:", "-has:labels"),
        ];
        for (input, expected, flagged) in cases {
            let err = parse(input).unwrap_err();
            let FilterError::Unsupported { construct, span } = &err else {
                panic!("{input}: {err:?}");
            };
            assert_eq!(construct, expected, "{input}");
            assert_eq!(&input[span.0..span.1], flagged, "{input}");
        }
    }

    #[test]
    fn a_word_without_a_colon_is_a_bare_word() {
        let err = parse("type:bug crash").unwrap_err();
        assert_eq!(
            err,
            FilterError::BareWord {
                word: "crash".into(),
                span: Span(9, 14),
            }
        );
    }

    #[test]
    fn malformed_values_are_syntax_errors() {
        for input in [
            "status:",
            "status:a,",
            r#"area:"open"#,
            "no:a,b",
            r#"area:"a"status:todo"#,
            r#"status:to"do"#,
        ] {
            let err = parse(input).unwrap_err();
            assert!(
                matches!(err, FilterError::Syntax { .. }),
                "{input}: {err:?}"
            );
        }
    }

    #[test]
    fn spans_count_bytes_and_cover_the_text_inside_quotes() {
        let err = parse("type:é crash").unwrap_err();
        let FilterError::BareWord { span, .. } = err else {
            panic!("{err:?}");
        };
        assert_eq!(span, Span(8, 13));
        let query = parse(r#"labels:é,"ü x" sort:"é""#).unwrap();
        let Term::Match { values, .. } = &query.terms[0] else {
            panic!("{query:?}");
        };
        let spans: Vec<Span> = values.iter().map(|(_, span)| *span).collect();
        assert_eq!(spans, [Span(7, 9), Span(11, 15)]);
        assert_eq!(query.sorts[0].field.1, Span(23, 25));
    }

    #[test]
    fn sort_suffixes_and_at_me_ignore_case() {
        let query = parse("sort:priority-ASC assignee:@ME").unwrap();
        assert_eq!(query.sorts[0].direction, Some(Direction::Ascending));
        assert_eq!(matches(&query)[0].2, [Value::Me]);
    }

    #[test]
    fn field_names_may_hold_underscores_and_digits() {
        let query = parse("story_points2:3").unwrap();
        assert_eq!(matches(&query)[0].0, "story_points2");
    }
}
