//! Stored text made safe to show on a terminal, for the CLI and the TUI.

/// Stored text made safe to print: control characters, which could move the
/// cursor or clear a terminal, and characters that disguise text appear
/// escaped, as `\u{1b}` or `\u{202e}`; a tab passes through.
#[must_use]
pub fn printable(text: &str) -> String {
    let escaped = |c: char| (c.is_control() && c != '\t') || disguises(c);
    // Almost all text needs nothing escaped, and lists print it by the
    // thousand.
    if !text.chars().any(escaped) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if escaped(c) {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// Bidirectional controls, which reorder what is shown (CVE-2021-42574); line
/// and paragraph separators, which `lines()` does not split on; and invisible
/// characters: zero-width space, word joiner and invisible operators, byte
/// order mark, and tags, which can hide text an agent reads but a person does
/// not see. Joiners stay, as emoji and scripts such as Persian need them.
const fn disguises(c: char) -> bool {
    matches!(
        c,
        '\u{061c}'
            | '\u{200b}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
            | '\u{e0000}'..='\u{e007f}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_escapes_what_disguises_text_and_keeps_joiners() {
        for (stored, shown) in [
            ("Fix login\u{202e}gnirts", r"Fix login\u{202e}gnirts"),
            ("a\u{2066}b\u{2069}", r"a\u{2066}b\u{2069}"),
            ("one\u{2028}two", r"one\u{2028}two"),
            ("TM\u{200b}-1", r"TM\u{200b}-1"),
            ("\u{feff}title", r"\u{feff}title"),
            ("\u{200f}\u{061c}", r"\u{200f}\u{61c}"),
            ("a\u{2060}b", r"a\u{2060}b"),
            ("a\u{e0041}b", r"a\u{e0041}b"),
            ("👩\u{200d}💻", "👩\u{200d}💻"),
            ("می\u{200c}خواهم", "می\u{200c}خواهم"),
            ("a\tb", "a\tb"),
        ] {
            assert_eq!(printable(stored), shown, "{stored:?}");
        }
    }
}
