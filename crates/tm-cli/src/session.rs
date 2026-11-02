//! The Agent sessions that worked an Issue, and the commands that resume them.

use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_store::{IssueView, SessionPlace, SessionWork, short_session};

use crate::error::CliError;
use crate::harness::Harness;
use crate::here;
use crate::list::Item;
use crate::output::{Output, printable, span};

/// The harness that named `place`'s session, when tm knows how to resume it.
pub(crate) fn harness(place: &SessionPlace) -> Option<Harness> {
    Harness::named(place.harness.as_deref()?)
}

/// The command that resumes `session` from where it last wrote here, for the
/// harnesses whose resume tm knows.
pub(crate) fn resume_command(session: &str, place: &SessionPlace) -> Option<String> {
    Some(harness(place)?.resume(&crate::fields::quoted(&place.dir), session))
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
        let _ = writeln!(text, "    {}", reach(&work.session, work.place.as_ref()));
    }
    expiry_notes(text, sessions.iter().map(|work| work.place.as_ref()));
}

/// How long each harness keeps a session, once per harness among `places`.
fn expiry_notes<'a>(text: &mut String, places: impl Iterator<Item = Option<&'a SessionPlace>>) {
    let named: Vec<Harness> = places.flatten().filter_map(harness).collect();
    for harness in Harness::ALL {
        if let Some(expiry) = harness.expiry().filter(|_| named.contains(&harness)) {
            let _ = writeln!(text, "  {expiry}");
        }
    }
}

/// How to reach `session` again: its resume command, or its id and where it ran.
fn reach(session: &str, place: Option<&SessionPlace>) -> String {
    match place {
        Some(place) => resume_command(session, place)
            .unwrap_or_else(|| format!("session {}", printable(session))),
        None => format!("session {}, on another machine", printable(session)),
    }
}

/// The Issues `tm sessions` prints under each session; the rest are counted.
const SHOWN_ISSUES: usize = 5;

/// `tm sessions`: the most recently active Agent sessions, each with the
/// Issues it wrote and how to resume it.
pub(crate) fn list(limit: Option<usize>, json: bool) -> Result<Output, CliError> {
    let here = here::read()?;
    let read = here.store.read(|store| {
        let (sessions, total) = store.recent_sessions(limit.unwrap_or(10))?;
        let mut views = Vec::with_capacity(sessions.len());
        for session in &sessions {
            views.push(store.issues(&session.issues)?);
        }
        let all: Vec<IssueView> = views.iter().flatten().cloned().collect();
        let projects = crate::list::project_keys(store, &all)?;
        let now_ms = store.utc_ms()?;
        Ok::<_, CliError>((sessions, total, views, projects, now_ms))
    });
    let (sessions, total, views, projects, now_ms) = match read {
        Ok(read) => read,
        Err(err) => return Err(err.warned(here.warnings)),
    };
    let mut output = if json {
        let listed: Vec<Value> = sessions
            .iter()
            .zip(&views)
            .map(|(session, views)| {
                json!({
                    "session": session.session,
                    "agent": session.agent,
                    "on_behalf_of": session.on_behalf_of,
                    "first_ms": session.first_ms,
                    "last_ms": session.last_ms,
                    "dir": session.place.as_ref().map(|place| &place.dir),
                    "harness": session.place.as_ref().and_then(|place| place.harness.as_ref()),
                    "resume": session.place.as_ref().and_then(|place| resume_command(&session.session, place)),
                    "issues": views.iter().map(|view| Item::of(view, &projects)).collect::<Vec<_>>(),
                })
            })
            .collect();
        Output::new(String::new(), json!({ "sessions": listed, "total": total }))
    } else {
        let mut text = String::new();
        for (session, views) in sessions.iter().zip(&views) {
            let _ = writeln!(
                text,
                "{} {}  last wrote {} ago",
                printable(&session.agent),
                printable(&short_session(&session.session)),
                span(now_ms - session.last_ms)
            );
            for view in views.iter().take(SHOWN_ISSUES) {
                let _ = writeln!(text, "  {}", crate::list::line(view));
            }
            if views.len() > SHOWN_ISSUES {
                let _ = writeln!(text, "  and {} more", views.len() - SHOWN_ISSUES);
            }
            let _ = writeln!(
                text,
                "  {}",
                reach(&session.session, session.place.as_ref())
            );
        }
        expiry_notes(
            &mut text,
            sessions.iter().map(|session| session.place.as_ref()),
        );
        if total == 0 {
            text.push_str("no Agent session has written here\n");
        } else if sessions.len() < total {
            let _ = writeln!(
                text,
                "({} of {total} sessions shown; --limit for more)",
                sessions.len()
            );
        }
        Output::new(text, Value::Null)
    };
    output.warnings = here.warnings;
    Ok(output)
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
