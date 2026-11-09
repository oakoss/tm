//! Markdown laid out as terminal lines: an Issue's description and its
//! comments, wrapped to a width with list items and quotes indented.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use tm_core::text::printable;
use unicode_width::UnicodeWidthStr;

/// `text` wrapped to `width` columns; a code line, or a word wider than
/// that, is left whole for the screen to wrap.
pub(crate) fn lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let mut out = Layout::new(usize::from(width.max(1)));
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_TABLES;
    for event in Parser::new_ext(text, options) {
        out.event(event);
    }
    out.flush();
    while out.lines.last().is_some_and(|line| line.width() == 0) {
        out.lines.pop();
    }
    out.lines
}

/// A list open around the text: its next number, or none for bullets.
struct List {
    next: Option<u64>,
}

struct Word {
    span: Span<'static>,
    /// Whether it joins the word before with no space, as `a**b**` does.
    glued: bool,
}

struct Layout {
    width: usize,
    lines: Vec<Line<'static>>,
    /// The words of the block being written.
    words: Vec<Word>,
    /// Whether the next text joins the word before it with no space.
    joined: bool,
    styles: Vec<Style>,
    lists: Vec<List>,
    /// The marker the next line of a list item starts with.
    marker: Option<String>,
    quotes: usize,
    code: bool,
    link: Vec<String>,
}

impl Layout {
    fn new(width: usize) -> Self {
        Self {
            width,
            lines: Vec::new(),
            words: Vec::new(),
            joined: false,
            styles: vec![Style::new()],
            lists: Vec::new(),
            marker: None,
            quotes: 0,
            code: false,
            link: Vec::new(),
        }
    }

    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, add: Style) {
        let style = self.style().patch(add);
        self.styles.push(style);
    }

    /// The indent a line inside the open lists and quotes starts with.
    fn indent(&self) -> String {
        let mut indent = "│ ".repeat(self.quotes);
        indent.push_str(&"  ".repeat(self.lists.len()));
        indent
    }

    fn blank(&mut self) {
        if self.lines.last().is_some_and(|line| line.width() > 0) {
            let indent = "│ ".repeat(self.quotes);
            self.lines.push(Line::from(indent.trim_end().to_owned()));
        }
    }

    fn text(&mut self, text: &str) {
        if self.code {
            let indent = format!("{}    ", self.indent());
            for line in text.lines() {
                let safe = printable(line).replace('\t', "    ");
                self.lines.push(Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::styled(safe, Style::new().add_modifier(Modifier::DIM)),
                ]));
            }
            return;
        }
        let safe = printable(text).replace('\t', "    ");
        let style = self.style();
        let starts_spaced = safe.starts_with(char::is_whitespace);
        for (at, word) in safe.split_whitespace().enumerate() {
            let glued = at == 0 && self.joined && !starts_spaced;
            match self.words.last_mut() {
                Some(last) if glued && last.span.style == style => {
                    last.span.content.to_mut().push_str(word);
                }
                _ => self.words.push(Word {
                    span: Span::styled(word.to_owned(), style),
                    glued,
                }),
            }
        }
        self.joined = !safe.ends_with(char::is_whitespace) && !safe.is_empty();
    }

    /// Writes the words gathered as wrapped lines.
    fn flush(&mut self) {
        if self.words.is_empty() {
            self.marker = None;
            return;
        }
        let indent = self.indent();
        let (first, rest) = match self.marker.take() {
            // The marker takes its list level's two columns and more; the
            // item's later lines start under its text.
            Some(marker) => {
                let outer = &indent[..indent.len().saturating_sub(2)];
                let under = " ".repeat(UnicodeWidthStr::width(marker.as_str()));
                (format!("{outer}{marker}"), format!("{outer}{under}"))
            }
            None => (indent.clone(), indent),
        };
        let words = std::mem::take(&mut self.words);
        self.joined = false;
        let mut used = UnicodeWidthStr::width(first.as_str());
        let mut line: Vec<Span<'static>> = vec![Span::raw(first)];
        let mut fresh = true;
        // A word's width with the glued pieces after it, as `ab**cd**` is
        // one word: it wraps whole.
        let mut whole = vec![0; words.len()];
        for at in (0..words.len()).rev() {
            let after = words.get(at + 1).filter(|next| next.glued);
            whole[at] = words[at].span.width() + after.map_or(0, |_| whole[at + 1]);
        }
        for (Word { span, glued }, whole) in words.into_iter().zip(whole) {
            let wide = span.width();
            let gap = usize::from(!fresh && !glued);
            if !fresh && !glued && used + gap + whole > self.width {
                self.lines.push(Line::from(std::mem::take(&mut line)));
                used = UnicodeWidthStr::width(rest.as_str());
                line.push(Span::raw(rest.clone()));
            } else if gap == 1 {
                line.push(Span::raw(" "));
                used += 1;
            }
            used += wide;
            line.push(span);
            fresh = false;
        }
        self.lines.push(Line::from(line));
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                let style = self.style().add_modifier(Modifier::REVERSED);
                self.styles.push(style);
                self.text(&code);
                self.styles.pop();
            }
            Event::SoftBreak => self.joined = false,
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.flush();
                self.blank();
                self.lines.push(Line::from("─".repeat(self.width.min(40))));
            }
            Event::TaskListMarker(done) => self.text(if done { "[x] " } else { "[ ] " }),
            Event::Html(html) | Event::InlineHtml(html) => self.text(&html),
            Event::FootnoteReference(name) => self.text(&format!("[{name}]")),
            Event::InlineMath(math) | Event::DisplayMath(math) => self.text(&math),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                if self.marker.is_none() {
                    self.flush();
                    if self.lists.is_empty() {
                        self.blank();
                    }
                }
            }
            Tag::Heading { level, .. } => {
                self.flush();
                self.blank();
                let mut style = Style::new().add_modifier(Modifier::BOLD);
                if level == HeadingLevel::H1 {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                self.push_style(style);
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.blank();
                self.quotes += 1;
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                self.blank();
                if let CodeBlockKind::Fenced(language) = kind
                    && !language.is_empty()
                {
                    let indent = format!("{}    ", self.indent());
                    self.lines.push(Line::from(Span::styled(
                        format!("{indent}{}", printable(&language)),
                        Style::new().add_modifier(Modifier::ITALIC | Modifier::DIM),
                    )));
                }
                self.code = true;
            }
            Tag::List(first) => {
                self.flush();
                if self.lists.is_empty() {
                    self.blank();
                }
                self.lists.push(List { next: first });
            }
            Tag::Item => {
                self.flush();
                let marker = match self.lists.last_mut() {
                    Some(List { next: Some(n) }) => {
                        let marker = format!("{n}. ");
                        *n += 1;
                        marker
                    }
                    _ => "• ".to_owned(),
                };
                self.marker = Some(marker);
            }
            Tag::Emphasis => self.push_style(Style::new().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(Style::new().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(Style::new().add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.link.push(dest_url.into_string());
                self.push_style(Style::new().add_modifier(Modifier::UNDERLINED));
            }
            Tag::Table(_) | Tag::TableHead | Tag::TableRow => self.flush(),
            Tag::TableCell if !self.words.is_empty() => self.text(" │ "),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::TableHead | TagEnd::TableRow | TagEnd::Item => {
                self.flush();
            }
            TagEnd::Heading(_) => {
                self.flush();
                self.styles.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quotes = self.quotes.saturating_sub(1);
            }
            TagEnd::CodeBlock => self.code = false,
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link | TagEnd::Image => {
                self.styles.pop();
                if let Some(url) = self.link.pop()
                    && !url.is_empty()
                {
                    let style = self.style().add_modifier(Modifier::DIM);
                    self.styles.push(style);
                    self.text(&format!(" ({url})"));
                    self.styles.pop();
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str, width: u16) -> Vec<String> {
        lines(text, width)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn paragraphs_wrap_at_the_width_with_a_blank_line_between() {
        assert_eq!(
            plain("one two three four\n\nfive", 9),
            ["one two", "three", "four", "", "five"]
        );
    }

    #[test]
    fn list_items_wrap_under_their_text() {
        assert_eq!(
            plain("- alpha beta gamma\n- delta\n\n1. one two three", 12),
            [
                "• alpha beta",
                "  gamma",
                "• delta",
                "",
                "1. one two",
                "   three"
            ]
        );
    }

    #[test]
    fn a_nested_list_indents_under_its_item() {
        assert_eq!(
            plain("- outer\n  - inner words here", 14),
            ["• outer", "  • inner", "    words here"]
        );
    }

    #[test]
    fn styles_follow_emphasis_and_code_keeps_its_lines() {
        let laid = lines("plain **bold** `code`\n\n```rust\nlet a = 1;\n```", 40);
        let styled = |text: &str| {
            let span = laid[0].spans.iter().find(|span| span.content == text);
            span.unwrap().style.add_modifier
        };
        assert!(styled("bold").contains(Modifier::BOLD));
        assert!(styled("code").contains(Modifier::REVERSED));
        assert!(!styled("plain").contains(Modifier::BOLD));
        let text: Vec<String> = laid.iter().map(ToString::to_string).collect();
        assert_eq!(text[2..], ["    rust", "    let a = 1;"]);
    }

    #[test]
    fn words_joined_by_markup_stay_joined() {
        assert_eq!(plain("a**b**c, `x`.", 20), ["abc, x."]);
    }

    #[test]
    fn links_show_where_they_go_and_quotes_are_marked() {
        assert_eq!(
            plain("[docs](https://x.dev)\n\n> quoted text", 40),
            ["docs (https://x.dev)", "", "│ quoted text"]
        );
    }

    #[test]
    fn text_the_terminal_would_act_on_is_shown_escaped() {
        let laid = plain("red \u{1b}[31m and\ttab", 40);
        assert_eq!(laid, ["red \\u{1b}[31m and tab"]);
        let code = plain("```\u{1b}x\n\u{1b}[2J clear\n```", 40);
        assert_eq!(code, ["    \\u{1b}x", "    \\u{1b}[2J clear"]);
    }

    #[test]
    fn an_ordered_list_counts_from_its_first_number() {
        assert_eq!(plain("3. a\n4. b\n5. c", 20), ["3. a", "4. b", "5. c"]);
    }

    #[test]
    fn a_quote_keeps_its_bar_on_every_line() {
        assert_eq!(
            plain("> one two three four\n>\n> five", 10),
            ["│ one two", "│ three", "│ four", "│", "│ five"]
        );
    }

    #[test]
    fn a_word_glued_from_pieces_wraps_whole() {
        assert_eq!(plain("123456 ab**cd**", 10), ["123456", "abcd"]);
    }

    #[test]
    fn no_line_is_wider_than_asked_unless_one_word_is() {
        let text = "- a list item long enough to wrap twice over here\n\n> a quote long enough to wrap as well\n\nplain words to wrap";
        for width in 8..40 {
            for line in lines(text, width) {
                let wide = line.width();
                let one_word = line
                    .spans
                    .iter()
                    .filter(|span| !span.content.trim().is_empty())
                    .count()
                    == 1;
                assert!(wide <= usize::from(width) || one_word, "{width}: {line}");
            }
        }
    }
}
