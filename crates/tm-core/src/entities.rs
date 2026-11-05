//! HTML entities an Agent writes when it escapes plain text by mistake, as
//! in `Build &amp; Run`.

use std::borrow::Cow;

/// The entities that escaping `&`, `<`, `>`, `"` and `'` produces, with the
/// characters they stand for.
const ENTITIES: [(&str, char); 6] = [
    ("&amp;", '&'),
    ("&lt;", '<'),
    ("&gt;", '>'),
    ("&quot;", '"'),
    ("&#39;", '\''),
    ("&#x27;", '\''),
];

/// The first of those entities in `text`, if it holds one.
#[must_use]
pub fn found(text: &str) -> Option<&'static str> {
    text.match_indices('&').find_map(|(at, _)| {
        ENTITIES
            .iter()
            .find(|(entity, _)| text[at..].starts_with(entity))
            .map(|(entity, _)| *entity)
    })
}

/// `text` with each of those entities decoded once, left to right, so
/// `&amp;lt;` becomes `&lt;`; any other `&` stays as it is.
#[must_use]
pub fn decoded(text: &str) -> Cow<'_, str> {
    if found(text).is_none() {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        if let Some((entity, plain)) = ENTITIES.iter().find(|(entity, _)| rest.starts_with(entity))
        {
            out.push(*plain);
            rest = &rest[entity.len()..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_escaped_characters_once_and_leaves_other_ampersands() {
        let cases = [
            ("Add a Build &amp; Run section", "Add a Build & Run section"),
            ("Mutex&lt;PaneManager&gt;", "Mutex<PaneManager>"),
            (
                "&quot;a&quot; and it&#39;s, it&#x27;s",
                "\"a\" and it's, it's",
            ),
            ("&amp;lt; stays one level", "&lt; stays one level"),
            (
                "takes &mut self && &str & &nbsp;",
                "takes &mut self && &str & &nbsp;",
            ),
            ("a &amp; b&nbsp;c", "a & b&nbsp;c"),
            ("trailing &", "trailing &"),
            ("", ""),
        ];
        for (text, want) in cases {
            assert_eq!(decoded(text), want, "{text}");
        }
        assert!(matches!(decoded("no entity & here"), Cow::Borrowed(_)));
    }

    #[test]
    fn finds_the_first_entity() {
        assert_eq!(found("a &mut b &lt; c &amp;"), Some("&lt;"));
        assert_eq!(found("a &mut b && c"), None);
    }
}
