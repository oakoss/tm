//! The Agent sessions that worked an Issue, and the commands that resume them.

use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_store::{SessionPlace, SessionWork, short_session};

use crate::output::{printable, span};

/// The command that resumes `session` from where it last wrote here, for the
/// harnesses whose resume tm knows.
pub(crate) fn resume_command(session: &str, place: &SessionPlace) -> Option<String> {
    let dir = crate::fields::quoted(&place.dir);
    match place.harness.as_deref()? {
        "claude" => Some(format!("cd {dir} && claude -r {session}")),
        "codex" => Some(format!("cd {dir} && codex resume {session}")),
        _ => None,
    }
}

fn resume(work: &SessionWork) -> Option<String> {
    resume_command(&work.session, work.place.as_ref()?)
}

/// What the session did to the Issue, renewals aside.
fn did(work: &SessionWork) -> String {
    let count = |n: usize, one: &str, many: &str| match n {
        0 => None,
        1 => Some(one.to_owned()),
        n => Some(format!("{n} {many}")),
    };
    let mut parts: Vec<String> = Vec::new();
    if work.claimed {
        parts.push("claimed".into());
    }
    parts.extend(count(work.comments, "1 comment", "comments"));
    parts.extend(count(work.asked, "asked a question", "questions asked"));
    parts.extend(count(work.answered, "answered", "answers"));
    parts.extend(count(
        work.withdrew,
        "withdrew a question",
        "questions withdrawn",
    ));
    if !work.moved_to.is_empty() {
        let moved: Vec<String> = work.moved_to.iter().map(|to| printable(to)).collect();
        parts.push(format!("moved to {}", moved.join(", ")));
    }
    if work.handed {
        parts.push("handed it on".into());
    }
    parts.extend(count(work.other, "1 other change", "other changes"));
    if parts.is_empty() {
        return "kept its Claim".into();
    }
    parts.join(", ")
}

/// The Sessions section of `tm show`, most recently active first.
pub(crate) fn write_section(text: &mut String, sessions: &[SessionWork], now_ms: i64) {
    if sessions.is_empty() {
        return;
    }
    let _ = writeln!(text, "Sessions ({}):", sessions.len());
    for work in sessions {
        let mut line = format!(
            "  {} {}",
            printable(&work.agent),
            printable(&short_session(&work.session))
        );
        if let Some(branch) = &work.branch {
            let _ = write!(line, "  on {}", printable(branch));
        }
        let _ = write!(
            line,
            "  last wrote {} ago  {}",
            span(now_ms - work.last_ms),
            did(work)
        );
        if work.moved_last {
            line.push_str("  (moved it last)");
        }
        let _ = writeln!(text, "{line}");
        let how = match (resume(work), &work.place) {
            (Some(command), _) => command,
            (None, Some(_)) => format!("session {}", printable(&work.session)),
            (None, None) => format!("session {}, on another machine", printable(&work.session)),
        };
        let _ = writeln!(text, "    {how}");
    }
    let in_claude = |work: &SessionWork| {
        work.place
            .as_ref()
            .is_some_and(|place| place.harness.as_deref() == Some("claude"))
    };
    if sessions.iter().any(in_claude) {
        text.push_str(
            "  Claude deletes a session unused for 30 days by default, so an old one may not resume.\n",
        );
    }
}

pub(crate) fn json(work: &SessionWork) -> Value {
    let mut value = json!(work);
    if let Some(fields) = value.as_object_mut() {
        fields.remove("place");
    }
    value["dir"] = json!(work.place.as_ref().map(|place| &place.dir));
    value["harness"] = json!(work.place.as_ref().and_then(|place| place.harness.as_ref()));
    value["did"] = json!(did(work));
    value["resume"] = json!(resume(work));
    value
}
