//! Who tm asks, and how: only a person at a terminal (ADR 0014), on stderr,
//! with what is shown on stdout.

use std::io::{self, BufRead, IsTerminal, Write as _};

use crate::identity;

/// Reads a line for a question; `None` at end of input.
pub(crate) trait Prompt: FnMut(&str) -> Result<Option<String>, String> {}
impl<F: FnMut(&str) -> Result<Option<String>, String>> Prompt for F {}

/// A person at a terminal, the only one tm asks: stdin and stdout are
/// terminals and no Agent is detected.
pub(crate) fn human() -> bool {
    is_human(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        identity::agent_detected(identity::env),
    )
}

fn is_human(terminal: bool, agent: bool) -> bool {
    terminal && !agent
}

/// Writes through a copy of the descriptor, as `output::finish` does: std's
/// stdout reports success for a write to one that takes none, and a caller
/// shows `text` to gate a write on it being seen.
pub(crate) fn print_now(text: &str) -> Result<(), String> {
    io::stdout()
        .flush()
        .and_then(|()| crate::output::stdout())
        .and_then(|mut out| out.write_all(text.as_bytes()))
        .map_err(|err| format!("cannot write output: {err}"))
}

/// Shows `shown` and asks `question` at the terminal, defaulting to no.
pub(crate) fn ask(shown: &str, question: &str) -> Result<bool, String> {
    print_now(shown)?;
    eprint!("{question} [y/N] ");
    answer(&mut io::stdin().lock())
}

/// A yes read from `input`; anything else, end of input included, is no.
fn answer(input: &mut impl BufRead) -> Result<bool, String> {
    let mut line = Vec::new();
    let read = input
        .read_until(b'\n', &mut line)
        .map_err(|err| format!("cannot read the answer: {err}"))?;
    if read == 0 {
        eprintln!();
    }
    Ok(matches!(line.trim_ascii(), b"y" | b"Y" | b"yes"))
}

/// Asks `question` at the terminal for a line of text.
pub(crate) fn prompt(question: &str) -> Result<Option<String>, String> {
    eprint!("{question}");
    line(&mut io::stdin().lock())
}

/// One line read from `input`, trimmed, or `None` at end of input.
fn line(input: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut line = String::new();
    let read = input
        .read_line(&mut line)
        .map_err(|err| format!("cannot read the answer: {err}"))?;
    if read == 0 {
        eprintln!();
        return Ok(None);
    }
    Ok(Some(line.trim().to_owned()))
}

pub(crate) fn stopped() -> String {
    "stopped at the prompt; nothing written".into()
}

#[cfg(test)]
mod tests {
    use super::{answer, is_human, line};

    #[test]
    fn only_a_person_at_a_terminal_is_asked() {
        assert!(is_human(true, false));
        assert!(!is_human(false, false), "a pipe is never asked");
        assert!(!is_human(true, true), "an Agent is never asked");
    }

    #[test]
    fn only_a_yes_is_yes() {
        for (typed, yes) in [
            ("y\n", true),
            ("yes\n", true),
            (" Y \n", true),
            ("\n", false),
            ("no\n", false),
            ("yess\n", false),
            ("", false),
        ] {
            assert_eq!(answer(&mut typed.as_bytes()), Ok(yes), "{typed:?}");
        }
    }

    #[test]
    fn a_line_is_trimmed_and_end_of_input_is_none() {
        assert_eq!(line(&mut " ops \n".as_bytes()), Ok(Some("ops".into())));
        assert_eq!(line(&mut "\n".as_bytes()), Ok(Some(String::new())));
        assert_eq!(line(&mut "".as_bytes()), Ok(None));
    }
}
