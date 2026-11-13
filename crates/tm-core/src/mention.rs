//! Mentions (ADR 0019): an `@word` in saved text, resolved to a token naming
//! an Actor and shown by that Actor's name.

use std::ops::Range;

/// The Actor a mention names: a person by the key tm records them under, or
/// an Agent by its name and the human it writes for.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mentioned {
    Person(String),
    Agent { name: String, human: String },
}

impl Mentioned {
    /// How the mention is stored in text.
    #[must_use]
    pub fn token(&self) -> String {
        match self {
            Self::Person(key) => format!("<@{key}>"),
            Self::Agent { name, human } => format!("<@{name} for {human}>"),
        }
    }

    /// Whether its token reads back as this Actor, so it may be offered.
    #[must_use]
    pub fn offerable(&self) -> bool {
        let fits = |part: &str| {
            !part.is_empty()
                && part.trim() == part
                && !part.contains(['<', '>', '\n', '\r'])
                && !part.contains(" for ")
        };
        match self {
            Self::Person(key) => fits(key),
            Self::Agent { name, human } => fits(name) && fits(human),
        }
    }
}

/// An Actor a mention may name, with the name it is shown by when tm holds
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    pub actor: Mentioned,
    pub name: Option<String>,
}

impl Known {
    /// How a mention of it reads: `@` and its name, else its email or Agent
    /// name.
    #[must_use]
    pub fn shown(&self) -> String {
        match (self.name(), &self.actor) {
            (Some(name), _) => format!("@{name}"),
            (None, Mentioned::Agent { name, .. } | Mentioned::Person(name)) => format!("@{name}"),
        }
    }

    /// Its recorded name, unless the name holds `<`, `>` or a line break,
    /// which ADR 0019 never offers.
    fn name(&self) -> Option<&str> {
        self.name
            .as_deref()
            .filter(|name| !name.contains(['<', '>', '\n', '\r']))
    }

    /// What may follow an `@` to name it: a person's email, name and each
    /// word of it, or an Agent's name.
    fn phrases(&self) -> Vec<&str> {
        match (&self.actor, self.name()) {
            (Mentioned::Agent { name, .. }, _) => vec![name.as_str()],
            (Mentioned::Person(key), name) => std::iter::once(key.as_str())
                .chain(name)
                .chain(name.into_iter().flat_map(str::split_whitespace))
                .collect(),
        }
    }

    /// Whether `word` is a person's email or its local part, ignoring case.
    fn addressed_by(&self, word: &str) -> bool {
        let Mentioned::Person(key) = &self.actor else {
            return false;
        };
        let (key, word) = (key.to_lowercase(), word.to_lowercase());
        key == word || key.split_once('@').is_some_and(|(local, _)| local == word)
    }
}

/// An `@` in text and what it names.
#[derive(Debug, PartialEq, Eq)]
pub struct Found<'a> {
    /// The `@` and the words taken, which a token replaces.
    pub span: Range<usize>,
    /// What was typed after the `@`.
    pub typed: &'a str,
    /// Every Actor it may name; one is resolved, several asks which.
    pub candidates: Vec<&'a Known>,
}

/// The tokens already in `text`, outside code, with where each is.
#[must_use]
pub fn tokens(text: &str) -> Vec<(Range<usize>, Mentioned)> {
    scan(text, &code(text))
}

/// `text` with each token `name` knows shown as it says, in code or not,
/// since tm writes tokens only where it resolved a mention; any other
/// stays as written.
#[must_use]
pub fn shown(text: &str, name: impl Fn(&Mentioned) -> Option<String>) -> String {
    let edits = every_token(text)
        .into_iter()
        .filter_map(|(span, actor)| name(&actor).map(|shown| (span, shown)))
        .collect();
    replace(text, edits)
}

/// Every token in `text`, in code or not, as [`shown`] reads them.
#[must_use]
pub fn every_token(text: &str) -> Vec<(Range<usize>, Mentioned)> {
    scan(text, &[])
}

/// The tokens in `text` that do not start inside `skip`.
fn scan(text: &str, skip: &[Range<usize>]) -> Vec<(Range<usize>, Mentioned)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("<@").map(|at| at + from) {
        let inner_start = at + 2;
        let end = text[inner_start..]
            .find(['>', '<', '\n'])
            .map(|end| end + inner_start);
        match end {
            Some(end) if text[end..].starts_with('>') && !inside(skip, at) => {
                let inner = &text[inner_start..end];
                let actor = match inner.split_once(" for ") {
                    Some((name, human)) => Mentioned::Agent {
                        name: name.to_owned(),
                        human: human.to_owned(),
                    },
                    None => Mentioned::Person(inner.to_owned()),
                };
                if actor.offerable() {
                    found.push((at..end + 1, actor));
                }
                from = end + 1;
            }
            _ => from = inner_start,
        }
    }
    found
}

/// Every `@` in `text` that may start a mention and names a known Actor:
/// at the start of a word, outside code and tokens. The longest email, local
/// part, name, word of a name or Agent name that follows is taken whole, so
/// what `shown` prints saves back the same.
#[must_use]
pub fn find<'a>(text: &'a str, known: &'a [Known]) -> Vec<Found<'a>> {
    let code = code(text);
    let taken: Vec<Range<usize>> = tokens(text).into_iter().map(|(span, _)| span).collect();
    let offered: Vec<&Known> = known
        .iter()
        .filter(|known| known.actor.offerable())
        .collect();
    let mut found: Vec<Found<'a>> = Vec::new();
    for (at, _) in text.match_indices('@') {
        let within_last = found.last().is_some_and(|last| at < last.span.end);
        if within_last || inside(&code, at) || inside(&taken, at) || !starts_word(text, at) {
            continue;
        }
        let after = &text[at + 1..];
        let word = word(after);
        let addressed: Vec<&Known> = offered
            .iter()
            .copied()
            .filter(|known| !word.is_empty() && known.addressed_by(word))
            .collect();
        // The longer reading wins, so `@kim@example.com` is the email though
        // `kim` names Kim, and a word longer than any name, as `@jace.babin`
        // or an email no one holds, names no one; readings of one length are
        // all candidates.
        let (len, candidates) = match named(&offered, after) {
            Some((len, named)) if len > word.len() => (len, named),
            Some((len, mut named)) if len == word.len() => {
                for known in addressed {
                    if !named.contains(&known) {
                        named.push(known);
                    }
                }
                (len, named)
            }
            _ => (word.len(), addressed),
        };
        if !candidates.is_empty() {
            found.push(Found {
                span: at..at + 1 + len,
                typed: &after[..len],
                candidates,
            });
        }
    }
    found
}

/// `text` with each span replaced, skipping one that overlaps another.
#[must_use]
pub fn replace(text: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|(span, _)| span.start);
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    for (span, with) in edits {
        if span.start < from {
            continue;
        }
        out.push_str(&text[from..span.start]);
        out.push_str(&with);
        from = span.end;
    }
    out.push_str(&text[from..]);
    out
}

/// The longest phrase naming an Actor that `after` starts with, ignoring
/// case and ending a word, as the bytes of `after` it takes, with every
/// Actor it names.
fn named<'a>(offered: &[&'a Known], after: &str) -> Option<(usize, Vec<&'a Known>)> {
    let mut best: Option<(usize, Vec<&'a Known>)> = None;
    for known in offered {
        let len = known
            .phrases()
            .into_iter()
            .filter_map(|phrase| prefix(after, phrase))
            .max();
        let Some(len) = len else { continue };
        match &mut best {
            Some((best_len, all)) if *best_len == len => all.push(known),
            Some((best_len, _)) if *best_len > len => {}
            _ => best = Some((len, vec![known])),
        }
    }
    best
}

/// The bytes of `text` that `phrase` matches at its start, ignoring case,
/// when the match ends a word.
fn prefix(text: &str, phrase: &str) -> Option<usize> {
    let mut rest = text.char_indices();
    let mut end = 0;
    for want in phrase.chars() {
        let (at, got) = rest.next()?;
        if !got.to_lowercase().eq(want.to_lowercase()) {
            return None;
        }
        end = at + got.len_utf8();
    }
    let ends = text[end..]
        .chars()
        .next()
        .is_none_or(|next| !next.is_alphanumeric());
    (end > 0 && ends).then_some(end)
}

/// The word after an `@`: letters, digits and `._+-@`, without the
/// punctuation that ends a sentence.
fn word(after: &str) -> &str {
    let end = after
        .find(|c: char| !(c.is_alphanumeric() || "._+-@".contains(c)))
        .unwrap_or(after.len());
    after[..end].trim_end_matches(|c: char| !c.is_alphanumeric())
}

/// Whether the `@` at `at` starts a word, as `assignee:@me` and an email's
/// `@` do not.
fn starts_word(text: &str, at: usize) -> bool {
    text[..at]
        .chars()
        .next_back()
        .is_none_or(|before| before.is_whitespace() || "([{\"'*_~>".contains(before))
}

fn inside(spans: &[Range<usize>], at: usize) -> bool {
    spans.iter().any(|span| span.contains(&at))
}

/// Where `text` holds code: fenced blocks, and spans between equal runs of
/// backticks.
fn code(text: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut fence: Option<(usize, &str)> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = ["```", "~~~"]
            .into_iter()
            .find(|marker| trimmed.starts_with(marker));
        match (fence, marker) {
            (None, Some(marker)) => fence = Some((offset, marker)),
            (Some((start, open)), Some(marker)) if marker == open => {
                spans.push(start..offset + line.len());
                fence = None;
            }
            _ => {}
        }
        offset += line.len();
    }
    if let Some((start, _)) = fence {
        spans.push(start..text.len());
    }
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'`' || inside(&spans, at) {
            at += 1;
            continue;
        }
        let run = bytes[at..].iter().take_while(|&&b| b == b'`').count();
        let closing = "`".repeat(run);
        let close = text[at + run..]
            .match_indices(closing.as_str())
            .map(|(found, _)| found + at + run)
            .find(|&found| bytes.get(found + run) != Some(&b'`') && bytes[found - 1] != b'`');
        match close {
            Some(close) => {
                spans.push(at..close + run);
                at = close + run;
            }
            None => at += run,
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(key: &str, name: Option<&str>) -> Known {
        Known {
            actor: Mentioned::Person(key.into()),
            name: name.map(Into::into),
        }
    }

    fn known() -> Vec<Known> {
        vec![
            person("jace@example.com", Some("Jace Babin")),
            person("kim@example.com", Some("Kim Lee")),
            person("lee@example.com", None),
            Known {
                actor: Mentioned::Agent {
                    name: "claude".into(),
                    human: "jace@example.com".into(),
                },
                name: None,
            },
        ]
    }

    /// Each mention found in `text`, as typed, with its candidates' tokens.
    fn named(text: &str) -> Vec<(&str, Vec<String>)> {
        let known = known();
        find(text, &known)
            .into_iter()
            .map(|found| {
                let tokens = found.candidates.iter().map(|k| k.actor.token()).collect();
                (&text[found.span], tokens)
            })
            .collect()
    }

    #[test]
    fn a_word_names_an_email_its_local_part_a_word_of_a_name_or_an_agent() {
        assert_eq!(
            named("hi @JACE"),
            [("@JACE", vec!["<@jace@example.com>".to_owned()])]
        );
        assert_eq!(
            named("@babin"),
            [("@babin", vec!["<@jace@example.com>".to_owned()])]
        );
        assert_eq!(
            named("@kim@example.com."),
            [("@kim@example.com", vec!["<@kim@example.com>".to_owned()])]
        );
        assert_eq!(
            named("ask @claude"),
            [("@claude", vec!["<@claude for jace@example.com>".to_owned()])]
        );
        assert_eq!(
            named("@lee"),
            [(
                "@lee",
                vec![
                    "<@kim@example.com>".to_owned(),
                    "<@lee@example.com>".to_owned()
                ]
            )],
            "Kim Lee's name and lee@'s local part both match"
        );
        assert_eq!(named("@ja and @nobody"), Vec::new());
    }

    #[test]
    fn a_full_name_is_taken_whole_so_a_shown_mention_saves_back_the_same() {
        assert_eq!(
            named("thanks @Jace Babin!"),
            [("@Jace Babin", vec!["<@jace@example.com>".to_owned()])]
        );
        let known = known();
        let text = "thanks <@jace@example.com>, ask <@claude for jace@example.com>";
        let shown_text = shown(text, |actor| {
            known.iter().find(|k| &k.actor == actor).map(Known::shown)
        });
        assert_eq!(shown_text, "thanks @Jace Babin, ask @claude");
        let edits = find(&shown_text, &known)
            .into_iter()
            .map(|found| (found.span, found.candidates[0].actor.token()))
            .collect();
        assert_eq!(replace(&shown_text, edits), text);
    }

    #[test]
    fn code_tokens_and_an_at_inside_a_word_are_never_mentions() {
        assert_eq!(named("mail jace@example.com"), Vec::new());
        assert_eq!(named("assignee:@me and @@kim"), Vec::new());
        assert_eq!(named("run `tm list @kim` now"), Vec::new());
        assert_eq!(named("```\n@kim\n```\n"), Vec::new());
        assert!(
            named("~~~\n@kim").is_empty(),
            "an unclosed fence runs to the end"
        );
        assert_eq!(named("<@kim@example.com>"), Vec::new(), "already a token");
        assert_eq!(named("`a` @kim").len(), 1);
    }

    #[test]
    fn a_token_reads_back_only_when_its_actor_could_be_offered() {
        let text = "<@kim@example.com> <@claude for jace@example.com> <@a<b> <@x for y for z>";
        let read: Vec<Mentioned> = tokens(text).into_iter().map(|(_, actor)| actor).collect();
        assert_eq!(
            read,
            [
                Mentioned::Person("kim@example.com".into()),
                Mentioned::Agent {
                    name: "claude".into(),
                    human: "jace@example.com".into()
                },
            ]
        );
        assert!(!Mentioned::Person("a>b".into()).offerable());
        assert!(!Mentioned::Person("kim for jace".into()).offerable());
    }

    #[test]
    fn any_name_or_agent_name_saves_back_as_shown_and_ends_at_a_word() {
        let known = vec![
            person("ann@example.com", Some("Ann (@jbabin)")),
            person("jbabin@example.com", None),
            person("ob@example.com", Some("O'Brien")),
            person("ops@example.com", Some("Kim <Ops>")),
            person("jace@example.com", Some("Jace Babin")),
            person("o'neil@example.com", None),
            Known {
                actor: Mentioned::Agent {
                    name: "claude code".into(),
                    human: "jace@example.com".into(),
                },
                name: None,
            },
        ];
        for one in &known {
            let text = format!("hi {} there", one.actor.token());
            let shown_text = shown(&text, |actor| {
                known.iter().find(|k| &k.actor == actor).map(Known::shown)
            });
            let edits = find(&shown_text, &known)
                .into_iter()
                .map(|found| (found.span, found.candidates[0].actor.token()))
                .collect();
            assert_eq!(replace(&shown_text, edits), text, "{shown_text}");
        }
        let typed: Vec<&str> = find("@Jace Babinski", &known)
            .into_iter()
            .map(|found| found.typed)
            .collect();
        assert_eq!(
            typed,
            ["Jace"],
            "the name ends a word, so only `Jace` is taken"
        );
        assert_eq!(
            find("@Kim <Ops>", &known).len(),
            0,
            "a name with <> is never offered"
        );
        assert_eq!(
            find("@Ann (@jbabin) hi", &known).len(),
            1,
            "an @ inside a name taken is not another mention"
        );
        assert_eq!(
            find("@jace@elsewhere.com", &known).len(),
            0,
            "an email no one holds names no one"
        );
        for longer in ["@jace.babin", "@jace-babin", "@claude-code-2"] {
            assert_eq!(find(longer, &known).len(), 0, "{longer} names no one");
        }
        for punctuated in ["@jace.", "@Jace's", "(@jace)", "@O'Brien,"] {
            assert_eq!(find(punctuated, &known).len(), 1, "{punctuated}");
        }
    }

    proptest::proptest! {
        #[test]
        fn any_text_is_read_without_panicking_and_resolving_is_stable(
            text in "(@kim|@lee|@Jace Babin|@claude|<@kim@example.com>|`|```\\n|[ a-z@<>~\\né日.-]){0,20}"
        ) {
            let known = known();
            let edits = find(&text, &known)
                .into_iter()
                .map(|found| (found.span, found.candidates[0].actor.token()))
                .collect();
            let saved = replace(&text, edits);
            let held = tokens(&saved);
            let again = find(&saved, &known)
                .iter()
                .any(|found| held.iter().any(|(span, _)| span.contains(&found.span.start)));
            proptest::prop_assert!(!again, "a token was found again in {saved:?}");
            let _ = shown(&saved, |actor| Some(actor.token()));
        }
    }
}
