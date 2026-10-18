//! Who is writing: the human, and the Agent writing for them (ADR 0010).

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use tm_core::Actor;

/// Each harness's variables, in the order they are checked, and the name its
/// Agent writes under. Codex comes first: it is the one launched from Claude
/// Code, and only the inner Agent sees both sets.
const HARNESSES: [(&str, &[&str]); 5] = [
    ("codex", &["CODEX_SESSION_ID", "CODEX_THREAD_ID"]),
    (
        "claude",
        &["CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_SESSION_ID"],
    ),
    ("gemini", &["GEMINI_CLI"]),
    ("opencode", &["OPENCODE"]),
    ("cursor", &["CURSOR_AGENT"]),
];

/// The writer, and what a write should warn about.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) actor: Actor,
    pub(crate) warnings: Vec<String>,
}

/// Resolves the writer from the environment `var` reads: the human is
/// `TM_ACTOR`, else git's `user.email` from `email`; an Agent is `TM_AGENT`,
/// else the first harness whose variables are set.
///
/// # Errors
///
/// A message for a `TM_ACTOR` or `TM_AGENT` that is set but blank, or when
/// no human can be found.
pub(crate) fn resolve(
    var: impl Fn(&str) -> Option<Var>,
    email: impl FnOnce() -> Result<String, String>,
) -> Result<Identity, String> {
    let human = match set(&var, "TM_ACTOR")? {
        Some(human) => human,
        None => email()?,
    };
    let detected: Vec<&str> = HARNESSES
        .iter()
        .filter(|(_, vars)| {
            vars.iter()
                .any(|name| var(name).is_some_and(|v| v.map_or(true, |v| !v.is_empty())))
        })
        .map(|(agent, _)| *agent)
        .collect();
    let mut warnings = Vec::new();
    let agent = match (set(&var, "TM_AGENT")?, detected.as_slice()) {
        (Some(named), _) => Some(named),
        (None, []) => None,
        (None, [only]) => Some((*only).to_owned()),
        (None, [first, ..]) => {
            warnings.push(format!(
                "found the variables of {}, so writing as {first}; set TM_AGENT to name the Agent",
                detected.join(" and ")
            ));
            Some((*first).to_owned())
        }
    };
    let actor = match agent {
        Some(agent) => Actor::agent(&agent, &human),
        None => Actor::human(&human),
    };
    Ok(Identity { actor, warnings })
}

/// A variable's value, or the bytes of one that is not valid UTF-8.
pub(crate) type Var = Result<String, OsString>;

/// A variable that is unset gives `None`; one set but blank or not valid
/// UTF-8 is an error, never a reason to write as someone else.
fn set(var: &impl Fn(&str) -> Option<Var>, name: &str) -> Result<Option<String>, String> {
    match var(name) {
        None => Ok(None),
        Some(Err(_)) => Err(format!("{name} is not valid UTF-8")),
        Some(Ok(value)) if value.trim().is_empty() => Err(format!(
            "{name} is set but blank; set it to {}, or unset it",
            if name == "TM_AGENT" {
                "the Agent's name"
            } else {
                "who you are"
            }
        )),
        Some(Ok(value)) => Ok(Some(value.trim().to_owned())),
    }
}

pub(crate) fn env(name: &str) -> Option<Var> {
    std::env::var_os(name).map(OsString::into_string)
}

/// git's `user.email` for the repo at `root`.
///
/// # Errors
///
/// A message when git cannot run, or when no email is set.
pub(crate) fn git_email(root: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .args(["config", "--get", "user.email"])
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|err| format!("cannot run git to read user.email: {err}"))?;
    let email = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    // `git config --get` exits 1 when the key is unset.
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let said = if stderr.is_empty() {
        String::new()
    } else {
        format!(" (git said: {stderr})")
    };
    match output.status.code() {
        Some(0) if !email.is_empty() => Ok(email),
        Some(0 | 1) => Err(format!(
            "cannot tell who you are; set TM_ACTOR or git's user.email{said}"
        )),
        _ => Err(format!(
            "git config --get user.email failed ({}){said}",
            output.status
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn resolved(vars: &[(&str, &str)]) -> Result<Identity, String> {
        let vars: BTreeMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        resolve(
            |name| vars.get(name).cloned().map(Ok),
            || Ok("jace@x.io".into()),
        )
    }

    fn writer(vars: &[(&str, &str)]) -> (String, bool, Option<String>) {
        let actor = resolved(vars).unwrap().actor;
        (
            actor.name().to_owned(),
            actor.is_agent(),
            actor.on_behalf_of().map(str::to_owned),
        )
    }

    #[test]
    fn with_no_agent_the_human_writes() {
        assert_eq!(writer(&[]), ("jace@x.io".into(), false, None));
        assert_eq!(
            writer(&[("TM_ACTOR", " kim ")]),
            ("kim".into(), false, None)
        );
    }

    #[test]
    fn each_harness_is_detected_by_its_own_variables() {
        for (vars, agent) in [
            (&[("CLAUDE_CODE_SESSION_ID", "s")][..], "claude"),
            (&[("CLAUDE_CODE_CHILD_SESSION", "1")][..], "claude"),
            (&[("CODEX_THREAD_ID", "t")][..], "codex"),
            (&[("CODEX_SESSION_ID", "s")][..], "codex"),
            (&[("GEMINI_CLI", "1")][..], "gemini"),
            (&[("OPENCODE", "1")][..], "opencode"),
            (&[("CURSOR_AGENT", "1")][..], "cursor"),
        ] {
            assert_eq!(
                writer(vars),
                (agent.into(), true, Some("jace@x.io".into())),
                "{vars:?}"
            );
        }
    }

    #[test]
    fn variables_that_mark_a_terminal_not_an_agent_are_ignored() {
        assert_eq!(
            writer(&[("CLAUDECODE", "1"), ("AI_AGENT", "claude-code_2_agent")]),
            ("jace@x.io".into(), false, None)
        );
        assert!(!writer(&[("CLAUDE_CODE_SESSION_ID", "")]).1);
    }

    #[test]
    fn two_harnesses_write_as_the_first_and_warn() {
        let both = [("CLAUDE_CODE_SESSION_ID", "s"), ("CODEX_THREAD_ID", "t")];
        let identity = resolved(&both).unwrap();
        assert_eq!(identity.actor.name(), "codex");
        assert_eq!(identity.warnings.len(), 1);
        assert!(
            identity.warnings[0].contains("TM_AGENT"),
            "{:?}",
            identity.warnings
        );
        assert_eq!(
            resolved(&[("CODEX_THREAD_ID", "t")]).unwrap().warnings,
            Vec::<String>::new()
        );
    }

    #[test]
    fn tm_agent_names_an_agent_and_overrides_detection() {
        assert_eq!(
            writer(&[("TM_AGENT", "reviewer-2")]),
            ("reviewer-2".into(), true, Some("jace@x.io".into()))
        );
        let named = resolved(&[
            ("TM_AGENT", "reviewer-2"),
            ("CLAUDE_CODE_SESSION_ID", "s"),
            ("CODEX_THREAD_ID", "t"),
        ])
        .unwrap();
        assert_eq!(named.actor.name(), "reviewer-2");
        assert_eq!(named.warnings, Vec::<String>::new());
    }

    #[test]
    fn a_name_that_is_not_utf8_is_refused_but_a_replacement_character_is_a_name() {
        use std::os::unix::ffi::OsStringExt as _;
        let bytes = OsString::from_vec(vec![b'j', 0xff]);
        let refused = resolve(
            |name| (name == "TM_ACTOR").then(|| Err(bytes.clone())),
            || Ok("jace@x.io".into()),
        )
        .unwrap_err();
        assert!(refused.contains("not valid UTF-8"), "{refused}");
        assert_eq!(writer(&[("TM_ACTOR", "ja\u{fffd}ce")]).0, "ja\u{fffd}ce");
    }

    #[test]
    fn a_blank_name_is_refused_not_ignored() {
        for name in ["TM_ACTOR", "TM_AGENT"] {
            let err = resolved(&[(name, "  ")]).unwrap_err();
            assert!(err.starts_with(name), "{err}");
        }
    }
}
