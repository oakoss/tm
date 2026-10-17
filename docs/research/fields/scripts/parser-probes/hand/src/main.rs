use filter_ast::{ParseError, Query, Span, Term, Value, finish, report};

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

fn err(code: &'static str, message: impl Into<String>, span: Span) -> ParseError {
    ParseError {
        code,
        message: message.into(),
        span,
    }
}

fn is_name(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}

fn is_bare(c: char) -> bool {
    !c.is_whitespace() && !matches!(c, ',' | '(' | ')' | '"' | ':')
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn take_while(&mut self, f: impl Fn(char) -> bool) -> &str {
        let start = self.pos;
        while let Some(c) = self.peek().filter(|&c| f(c)) {
            self.pos += c.len_utf8();
        }
        &self.src[start..self.pos]
    }

    fn query(&mut self) -> Result<Query, ParseError> {
        let mut terms = Vec::new();
        loop {
            self.take_while(char::is_whitespace);
            if self.peek().is_none() {
                return finish(terms);
            }
            terms.push(self.term()?);
        }
    }

    fn term(&mut self) -> Result<Term, ParseError> {
        let start = self.pos;
        let negated = self.eat('-');
        if matches!(self.peek(), Some('(' | ')')) {
            return Err(err(
                "unsupported",
                "parentheses are not supported yet",
                self.pos..self.pos + 1,
            ));
        }
        let name_start = self.pos;
        let field = self.take_while(is_name).to_owned();
        let field_span = name_start..self.pos;
        if !self.eat(':') {
            let end = self.pos
                + self.src[self.pos..]
                    .find(char::is_whitespace)
                    .unwrap_or(self.src.len() - self.pos);
            let word = &self.src[start..end];
            if matches!(word, "OR" | "AND" | "NOT") {
                return Err(err(
                    "unsupported",
                    format!("`{word}` is not supported yet"),
                    start..end,
                ));
            }
            if field.is_empty() {
                return Err(err(
                    "expected_field",
                    "expected a field name before `:`",
                    start..end.max(start + 1),
                ));
            }
            return Err(err(
                "bare_word",
                "full-text search is not supported yet; use field:value",
                start..end,
            ));
        }
        if field.is_empty() {
            return Err(err(
                "expected_field",
                "expected a field name before `:`",
                start..self.pos,
            ));
        }
        let mut values = vec![self.value()?];
        while self.eat(',') {
            values.push(self.value()?);
        }
        match self.peek() {
            None => {}
            Some(c) if c.is_whitespace() => {}
            Some(c) => {
                return Err(err(
                    "unexpected",
                    format!("unexpected `{c}`"),
                    self.pos..self.pos + c.len_utf8(),
                ));
            }
        }
        Ok(Term {
            negated,
            field,
            field_span,
            values,
        })
    }

    fn value(&mut self) -> Result<(Value, Span), ParseError> {
        let start = self.pos;
        match self.peek() {
            Some('"') => {
                self.pos += 1;
                let mut text = String::new();
                loop {
                    match self.peek() {
                        None => {
                            return Err(err(
                                "unterminated_quote",
                                "missing closing `\"`",
                                start..self.pos,
                            ));
                        }
                        Some('"') => {
                            self.pos += 1;
                            return Ok((Value::Text(text), start..self.pos));
                        }
                        Some('\\') => {
                            self.pos += 1;
                            if let Some(c) = self.peek() {
                                text.push(c);
                                self.pos += c.len_utf8();
                            }
                        }
                        Some(c) => {
                            text.push(c);
                            self.pos += c.len_utf8();
                        }
                    }
                }
            }
            Some('@') => {
                self.pos += 1;
                let name = self.take_while(is_name).to_owned();
                if self.peek() == Some('(') {
                    return Err(err(
                        "unsupported",
                        "function arguments are not supported yet",
                        start..self.pos + 1,
                    ));
                }
                Ok((Value::Function(name), start..self.pos))
            }
            Some('<' | '>' | '=') => Err(err(
                "unsupported",
                "comparisons are not supported yet",
                start..start + 1,
            )),
            _ => {
                let text = self.take_while(is_bare).to_owned();
                if text.is_empty() {
                    return Err(err("expected_value", "expected a value", start..start + 1));
                }
                Ok((Value::Text(text), start..self.pos))
            }
        }
    }
}

fn parse(src: &str) -> Result<Query, ParseError> {
    Parser { src, pos: 0 }.query()
}

fn main() {
    report("hand-written", parse);
}
