//! `@mentions` resolved when text is saved, and shown by name (ADR 0019).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_core::mention::{self, Found, Known, Mentioned};
use tm_store::{Store, StoreError};

use crate::error::CliError;
use crate::here::Here;
use crate::output::{Output, printable};
use crate::prompt;

/// How each Actor a token may name is shown, read once per command.
pub(crate) struct Names(BTreeMap<Mentioned, String>);

impl Names {
    /// The names `store` holds; none are read when `texts` hold no token.
    ///
    /// # Errors
    ///
    /// A store failure.
    pub(crate) fn of<'a>(
        store: &Store,
        texts: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, StoreError> {
        if !texts.into_iter().any(|text| text.contains("<@")) {
            return Ok(Self(BTreeMap::new()));
        }
        let known = store.mentionable()?;
        Ok(Self(
            known
                .iter()
                .map(|known| (known.actor.clone(), known.shown()))
                .collect(),
        ))
    }

    /// `text` with each token of a known Actor shown as `@` and its name,
    /// made printable; any other stays as written.
    pub(crate) fn shown(&self, text: &str) -> String {
        mention::shown(text, |actor| self.0.get(actor).map(|name| printable(name)))
    }

    /// The mentions in `texts`, each token once with how it is shown, as a
    /// command's JSON lists them beside the stored text.
    pub(crate) fn json<'a>(&self, texts: impl IntoIterator<Item = &'a str>) -> Value {
        let mut listed = BTreeMap::new();
        for text in texts {
            for (_, actor) in mention::tokens(text) {
                if let Some(shown) = self.0.get(&actor) {
                    listed
                        .entry(actor.token())
                        .or_insert_with(|| mention_json(&actor, shown));
                }
            }
        }
        Value::Array(listed.into_values().collect())
    }
}

fn mention_json(actor: &Mentioned, shown: &str) -> Value {
    match actor {
        Mentioned::Person(key) => json!({ "token": actor.token(), "person": key, "shown": shown }),
        Mentioned::Agent { name, human } => {
            json!({ "token": actor.token(), "agent": name, "on_behalf_of": human, "shown": shown })
        }
    }
}

/// `output` with each token in its text shown by name.
///
/// # Errors
///
/// A store failure.
pub(crate) fn named(store: &Store, mut output: Output) -> Result<Output, CliError> {
    let names = Names::of(store, [output.text.as_str()])?;
    output.text = names.shown(&output.text);
    Ok(output)
}

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
