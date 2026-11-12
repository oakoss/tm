//! `@mentions` resolved when text is saved (ADR 0019).

use std::fmt::Write as _;

use tm_core::mention::{self, Found, Known, Mentioned};

use crate::error::CliError;
use crate::here::Here;
use crate::prompt;

/// `text` with each mention of a known Actor written as its token. One
/// candidate is taken; of several, a person at a terminal picks one and
/// anyone else is refused with them. Text naming no one stays as typed.
///
/// # Errors
///
/// `ambiguous_mention` naming the candidates, whose JSON carries the text as
/// given in `text`, since it may have been read from stdin; a stopped
/// prompt; or a store failure.
pub(crate) fn resolve(here: &Here, text: &str) -> Result<String, CliError> {
    if !text.contains('@') {
        return Ok(text.to_owned());
    }
    let known = here.store.mentionable()?;
    resolve_with(text, &known, |found| {
        if prompt::human() {
            pick(found)
        } else {
            Err(ambiguous(found).with("text", text))
        }
    })
}

/// [`resolve`] over `known`, with `choose` deciding a mention that names
/// several Actors: the one meant, or none for plain text.
pub(crate) fn resolve_with(
    text: &str,
    known: &[Known],
    mut choose: impl FnMut(&Found<'_>) -> Result<Option<Mentioned>, CliError>,
) -> Result<String, CliError> {
    let mut edits = Vec::new();
    for found in mention::find(text, known) {
        let chosen = match found.candidates.as_slice() {
            [one] => Some(one.actor.clone()),
            _ => choose(&found)?,
        };
        if let Some(actor) = chosen {
            edits.push((found.span, actor.token()));
        }
    }
    Ok(mention::replace(text, edits))
}

/// Asks which Actor `@typed` means, or none.
fn pick(found: &Found<'_>) -> Result<Option<Mentioned>, CliError> {
    let mut list = format!("@{} names more than one:\n", found.typed);
    for (n, known) in found.candidates.iter().enumerate() {
        let _ = writeln!(list, "  {}. {}", n + 1, described(known));
    }
    eprint!("{list}");
    let question = format!(
        "Which one? [1-{}, Enter leaves it as text, Ctrl-D writes nothing] ",
        found.candidates.len()
    );
    loop {
        let answer = prompt::prompt(&question)
            .map_err(|err| CliError::new("io", err))?
            .ok_or_else(|| CliError::new("stopped", prompt::stopped()))?;
        if answer.is_empty() {
            return Ok(None);
        }
        let chosen = answer
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| found.candidates.get(n));
        match chosen {
            Some(known) => return Ok(Some(known.actor.clone())),
            None => eprintln!("{answer:?} is not one of them"),
        }
    }
}

/// The refusal of a mention naming several Actors where no one can be asked,
/// with how to name each.
fn ambiguous(found: &Found<'_>) -> CliError {
    let named: Vec<String> = found
        .candidates
        .iter()
        .map(|known| format!("{} ({})", described(known), unique(known)))
        .collect();
    CliError::new(
        "ambiguous_mention",
        format!(
            "@{} names more than one: {}; write the one you mean as shown in brackets",
            found.typed,
            named.join(", ")
        ),
    )
}

/// An Actor as a list shows it: its name and email, or an Agent and whom it
/// writes for.
fn described(known: &Known) -> String {
    match (&known.actor, &known.name) {
        (Mentioned::Person(key), Some(name)) => format!("{name} <{key}>"),
        (Mentioned::Person(key), None) => key.clone(),
        (Mentioned::Agent { name, human }, _) => format!("{name}, an Agent for {human}"),
    }
}

/// What names only this Actor: a person's email, or an Agent's token.
fn unique(known: &Known) -> String {
    match &known.actor {
        Mentioned::Person(key) => format!("@{key}"),
        Mentioned::Agent { .. } => known.actor.token(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<Known> {
        vec![
            Known {
                actor: Mentioned::Person("kim@example.com".into()),
                name: Some("Kim Lee".into()),
            },
            Known {
                actor: Mentioned::Person("lee@example.com".into()),
                name: None,
            },
        ]
    }

    #[test]
    fn one_candidate_is_taken_several_are_chosen_and_none_stays_text() {
        let known = known();
        let refuse = |_: &Found<'_>| -> Result<Option<Mentioned>, CliError> {
            panic!("asked about a mention with one candidate")
        };
        assert_eq!(
            resolve_with("hi @kim, @nobody", &known, refuse).unwrap(),
            "hi <@kim@example.com>, @nobody"
        );
        let second = |found: &Found<'_>| Ok(Some(found.candidates[1].actor.clone()));
        assert_eq!(
            resolve_with("@lee", &known, second).unwrap(),
            "<@lee@example.com>"
        );
        assert_eq!(resolve_with("@lee", &known, |_| Ok(None)).unwrap(), "@lee");
    }

    #[test]
    fn a_refusal_names_each_candidate_as_it_can_be_written() {
        let known = known();
        let err = resolve_with("@lee", &known, |found| Err(ambiguous(found))).unwrap_err();
        assert_eq!(err.code, "ambiguous_mention");
        assert!(
            err.message
                .contains("Kim Lee <kim@example.com> (@kim@example.com)")
                && err.message.contains("lee@example.com (@lee@example.com)"),
            "{}",
            err.message
        );
    }
}
