//! An Issue's text as lines: its title, its fields beside their labels, its
//! description and comments as Markdown, and its latest writes.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use tm_core::text::printable;

use super::data::{Issue, LATEST_WRITES, Shown};
use crate::markdown;

const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const DIM: Style = Style::new().add_modifier(Modifier::DIM);

/// `issue` laid out to `width` columns, its ages as of `now_ms`.
pub(crate) fn body(issue: &Issue, width: u16, now_ms: i64) -> Vec<Line<'static>> {
    let shown = &issue.shown;
    let title = field(shown, "title").unwrap_or_default();
    let mut lines = vec![Line::styled(title, BOLD), Line::default()];
    lines.extend(facts(shown, now_ms));
    lines.push(Line::default());
    lines.push(Line::styled("Description", BOLD));
    match shown.fields.get("description").and_then(Value::as_str) {
        Some(text) if !text.trim().is_empty() => lines.extend(markdown::lines(text, width)),
        _ => lines.push(Line::styled("none", DIM)),
    }
    lines.push(Line::default());
    lines.extend(comments(shown, width, now_ms));
    lines.push(Line::default());
    lines.extend(history(issue, now_ms));
    lines
}

/// `shown`'s text field `name`, safe for a screen cell.
fn field(shown: &Shown, name: &str) -> Option<String> {
    let text = match shown.fields.get(name)? {
        Value::Null => return None,
        Value::String(text) => text.clone(),
        // Shown as the CLI sent it rather than dropped.
        other => other.to_string(),
    };
    Some(printable(&text).replace('\t', " "))
}

/// Who acted, and for whom.
fn by(actor: &str, on_behalf_of: Option<&str>) -> String {
    match on_behalf_of {
        Some(person) => format!("{} for {}", printable(actor), printable(person)),
        None => printable(actor),
    }
}

/// The fields beside their labels: Status, the people, the Claim, the dates.
fn facts(shown: &Shown, now_ms: i64) -> Vec<Line<'static>> {
    let mut facts: Vec<(&str, String)> = Vec::new();
    for (label, name) in [
        ("Status", "status"),
        ("Type", "type"),
        ("Priority", "priority"),
        ("Assignee", "assignee"),
        ("Reporter", "reporter"),
    ] {
        if let Some(value) = field(shown, name) {
            facts.push((label, value));
        }
    }
    if let Some(claim) = &shown.claim {
        let mut holder = by(&claim.agent, claim.on_behalf_of.as_deref());
        if let Some(branch) = &claim.branch {
            holder = format!("{holder} on {}", printable(branch));
        }
        if let Some(left) = claim.expires_in_ms {
            holder = format!("{holder}, {} left", span(left));
        }
        facts.push(("Claim", holder));
    }
    for (label, name) in [("Created", "created"), ("Updated", "updated")] {
        if let Some(at) = shown.fields.get(name).and_then(Value::as_i64) {
            facts.push((label, ago(now_ms, at)));
        }
    }
    let wide = facts
        .iter()
        .map(|(label, _)| label.len())
        .max()
        .unwrap_or(0);
    facts
        .into_iter()
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(format!("{label:<wide$}  "), DIM),
                Span::raw(value),
            ])
        })
        .collect()
}

/// Each comment under who wrote it and when, its body as Markdown.
fn comments(shown: &Shown, width: u16, now_ms: i64) -> Vec<Line<'static>> {
    let heading = format!("Comments ({})", shown.comments.len());
    let mut lines = vec![Line::styled(heading, BOLD)];
    for comment in &shown.comments {
        let mut who = by(&comment.actor, comment.on_behalf_of.as_deref());
        // As `tm show` names a comment's part in a `tm ask`.
        match comment.role.as_deref() {
            Some("question") => who.push_str(" asked"),
            Some("answer") => who.push_str(" answered"),
            Some("withdrawn") => who.push_str(" withdrew the question"),
            _ => {}
        }
        let edited = if comment.edited_utc_ms.is_some() {
            " (edited)"
        } else {
            ""
        };
        let when = ago(now_ms, comment.utc_ms);
        lines.push(Line::styled(format!("{who} · {when}{edited}"), DIM));
        let body = comment.body.as_deref().filter(|_| !comment.retracted);
        let Some(body) = body else {
            lines.push(Line::styled("  retracted", DIM));
            continue;
        };
        for line in markdown::lines(body, width.saturating_sub(2)) {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
        if let Some(options) = comment
            .options
            .as_ref()
            .filter(|options| !options.is_empty())
        {
            let numbered: Vec<String> = options
                .iter()
                .enumerate()
                .map(|(at, option)| format!("{}. {}", at + 1, printable(option)))
                .collect();
            lines.push(Line::from(format!("  options: {}", numbered.join("  "))));
        }
    }
    lines
}

/// The latest writes, newest first, and how many more `tm history` has.
fn history(issue: &Issue, now_ms: i64) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled("History", BOLD)];
    let history = &issue.history;
    let shown = history.writes.len().min(LATEST_WRITES);
    for logged in history.writes.iter().rev().take(shown) {
        let who = by(&logged.actor, logged.on_behalf_of.as_deref());
        lines.push(Line::styled(
            format!("{} · {who}", ago(now_ms, logged.utc_ms)),
            DIM,
        ));
        for change in &logged.changes {
            lines.push(Line::from(format!("  {}", printable(&change.line))));
        }
    }
    let earlier = history.total.saturating_sub(shown);
    if earlier > 0 {
        let key = printable(issue.shown.key.as_deref().unwrap_or_default());
        lines.push(Line::styled(
            format!("{earlier} earlier: tm history {key}"),
            DIM,
        ));
    }
    lines
}

/// How long ago `at` was, as of `now`, both in Unix milliseconds.
fn ago(now: i64, at: i64) -> String {
    format!("{} ago", span(now.saturating_sub(at)))
}

/// `ms` as its largest whole unit.
fn span(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_age_takes_its_largest_whole_unit() {
        assert_eq!(ago(10_000, 10_000), "0s ago");
        assert_eq!(ago(3_600_000, 0), "1h ago");
        assert_eq!(ago(90_000_000, 0), "1d ago");
        assert_eq!(span(-5), "0s");
    }
}
