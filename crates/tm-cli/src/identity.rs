//! Who is writing: the human, and the Agent writing for them (ADR 0010).

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use tm_core::Actor;
use tm_core::actor::SessionId;

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

/// The variables holding each harness's session id, in the order they are
/// read; the other harnesses set only flags, so name none.
const SESSIONS: [(&str, &[&str]); 2] = [
    ("codex", &["CODEX_SESSION_ID", "CODEX_THREAD_ID"]),
    ("claude", &["CLAUDE_CODE_SESSION_ID"]),
];

/// The writer, and what a write should warn about.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) actor: Actor,
    pub(crate) warnings: Vec<String>,
}

/// Resolves the writer from the environment `var` reads: the human is
/// `TM_ACTOR`, else git's `user.email` from `email`; an Agent is `TM_AGENT`,
/// else the first harness whose variables are set. An Agent's session is
/// [`agent_session`]'s.
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
    let detected = harnesses(&var);
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
        Some(agent) => {
            let named = var("TM_AGENT").is_some();
            let session = agent_session(&var, named, &detected, &mut warnings)?;
            let actor = Actor::agent(&agent, &human);
            match session {
                Some(session) => actor.in_session(session),
                None => actor,
            }
        }
        None => Actor::human(&human),
    };
    Ok(Identity { actor, warnings })
}

/// The session an Agent writes in, as [`resolve`] finds it, or `None` for a
/// human or a variable [`resolve`] would refuse.
pub(crate) fn session(var: impl Fn(&str) -> Option<Var>) -> Option<SessionId> {
    let detected = harnesses(&var);
    let named = set(&var, "TM_AGENT").ok()?.is_some();
    if !named && detected.is_empty() {
        return None;
    }
    agent_session(&var, named, &detected, &mut Vec::new())
        .ok()
        .flatten()
}

/// Codex's own id when detection named the Agent `codex`, since its hooks and
/// its shell may see different `TM_SESSION`s; else `TM_SESSION`, else the
/// first detected harness's id. A `TM_AGENT` names an Agent that may only have
/// inherited Codex's variables, so its `TM_SESSION` stands.
fn agent_session(
    var: &impl Fn(&str) -> Option<Var>,
    named: bool,
    detected: &[&str],
    warnings: &mut Vec<String>,
) -> Result<Option<SessionId>, String> {
    let exported = match set(var, "TM_SESSION")? {
        Some(session) => Some(SessionId::parse(&session).ok_or_else(|| {
            format!(
                "TM_SESSION may hold only {}; set it to the session's id, or unset it",
                SessionId::RULE
            )
        })?),
        None => None,
    };
    let codex = detected
        .first()
        .filter(|harness| !named && **harness == "codex")
        .and_then(|harness| session_id(var, harness, &mut Vec::new()));
    Ok(codex.or(exported).or_else(|| {
        detected
            .first()
            .and_then(|harness| session_id(var, harness, warnings))
    }))
}

/// The first of `harness`'s session id variables that holds a usable value.
/// When those set are all unusable, a warning names them, as this session's
/// Claims would then read as every session's own.
fn session_id(
    var: &impl Fn(&str) -> Option<Var>,
    harness: &str,
    warnings: &mut Vec<String>,
) -> Option<SessionId> {
    let (_, names) = SESSIONS.iter().find(|(name, _)| *name == harness)?;
    let mut unusable = Vec::new();
    for name in *names {
        let Some(value) = var(name) else {
            continue;
        };
        match value.ok().as_deref().and_then(SessionId::parse) {
            Some(session) => return Some(session),
            None => unusable.push(*name),
        }
    }
    if !unusable.is_empty() {
        let (verb, ids) = if unusable.len() == 1 {
            ("is", "a usable session id")
        } else {
            ("are", "usable session ids")
        };
        warnings.push(format!(
            "{} {verb} not {ids}, so this session's Claims record none; set TM_SESSION to name it",
            unusable.join(" and ")
        ));
    }
    None
}

/// Whether an Agent is writing, by `TM_AGENT` or a harness's variables. A
/// `TM_AGENT` set at all counts, even blank, so doubt never prompts an Agent.
pub(crate) fn agent_detected(var: impl Fn(&str) -> Option<Var>) -> bool {
    var("TM_AGENT").is_some() || !harnesses(&var).is_empty()
}

/// The Agents whose harness variables are set, in [`HARNESSES`] order.
fn harnesses(var: &impl Fn(&str) -> Option<Var>) -> Vec<&'static str> {
    HARNESSES
        .iter()
        .filter(|(_, vars)| {
            vars.iter()
                .any(|name| var(name).is_some_and(|v| v.map_or(true, |v| !v.is_empty())))
        })
        .map(|(agent, _)| *agent)
        .collect()
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
            match name {
                "TM_AGENT" => "the Agent's name",
                "TM_SESSION" => "the session's id",
                _ => "who you are",
            }
        )),
        Some(Ok(value)) => Ok(Some(value.trim().to_owned())),
    }
}

static HOOK_VAR: OnceLock<(&str, String)> = OnceLock::new();

/// Reads `value` as `name` for the rest of the process, as the Agent's own
/// commands see it; a harness gives its hooks that value only on stdin. Call
/// it once, before anything reads the environment.
pub(crate) fn hook_var(name: &'static str, value: String) {
    let set = HOOK_VAR.set((name, value));
    assert!(set.is_ok(), "hook_var runs once a process");
}

pub(crate) fn env(name: &str) -> Option<Var> {
    match HOOK_VAR.get() {
        Some((var, value)) if *var == name => Some(Ok(value.clone())),
        _ => std::env::var_os(name).map(OsString::into_string),
    }
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
    fn an_agent_writes_in_its_harness_session_or_tm_session() {
        let session =
            |vars: &[(&str, &str)]| resolved(vars).unwrap().actor.session().map(str::to_owned);
        assert_eq!(
            session(&[("CLAUDE_CODE_SESSION_ID", " s1 ")]),
            Some("s1".into())
        );
        assert_eq!(session(&[("CODEX_THREAD_ID", "t1")]), Some("t1".into()));
        assert_eq!(
            session(&[("CODEX_SESSION_ID", "s1"), ("CODEX_THREAD_ID", "t1")]),
            Some("s1".into())
        );
        assert_eq!(session(&[("CLAUDE_CODE_CHILD_SESSION", "1")]), None);
        assert_eq!(session(&[("GEMINI_CLI", "1")]), None);
        assert_eq!(
            session(&[("GEMINI_CLI", "1"), ("TM_SESSION", "g1")]),
            Some("g1".into())
        );
        assert_eq!(
            session(&[("TM_SESSION", "mine"), ("CLAUDE_CODE_SESSION_ID", "s1")]),
            Some("mine".into())
        );
        assert_eq!(
            session(&[("TM_AGENT", "reviewer"), ("CLAUDE_CODE_SESSION_ID", "s1")]),
            Some("s1".into())
        );
        assert_eq!(
            session(&[("TM_SESSION", "mine"), ("CODEX_SESSION_ID", "s1")]),
            Some("s1".into())
        );
        assert_eq!(
            session(&[("TM_SESSION", "mine"), ("CODEX_SESSION_ID", " ")]),
            Some("mine".into())
        );
        assert_eq!(
            session(&[("TM_SESSION", "mine"), ("CODEX_THREAD_ID", "t1")]),
            Some("t1".into())
        );
        assert_eq!(
            session(&[
                ("TM_AGENT", "rev"),
                ("TM_SESSION", "mine"),
                ("CODEX_SESSION_ID", "s1")
            ]),
            Some("mine".into()),
            "an Agent named by TM_AGENT may only have inherited Codex's id"
        );
        let bad = resolved(&[("TM_SESSION", "a b"), ("CODEX_SESSION_ID", "s1")]).unwrap_err();
        assert!(bad.contains("the session's id"), "{bad}");
        assert_eq!(session(&[("TM_SESSION", "mine")]), None, "a human has none");
        for bad in [" ", "a\u{1}b", "s\u{200b}1", "…6417eead"] {
            let err = resolved(&[("TM_AGENT", "a"), ("TM_SESSION", bad)]).unwrap_err();
            assert!(err.contains("the session's id"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn session_finds_the_session_resolve_writes_in() {
        for vars in [
            &[("CLAUDE_CODE_SESSION_ID", "c1"), ("TM_SESSION", "t1")][..],
            &[("CODEX_THREAD_ID", "x1"), ("TM_SESSION", "t1")],
            &[("CODEX_SESSION_ID", " "), ("CODEX_THREAD_ID", "x2")],
            &[
                ("CODEX_SESSION_ID", "x1"),
                ("TM_AGENT", "rev"),
                ("TM_SESSION", "t1"),
            ],
            &[("CODEX_SESSION_ID", "x1"), ("CLAUDE_CODE_SESSION_ID", "c1")],
            &[("GEMINI_CLI", "1"), ("TM_SESSION", "g1")],
            &[("TM_SESSION", "t1")],
        ] {
            let map: BTreeMap<&str, &str> = vars.iter().copied().collect();
            let found = session(|name| map.get(name).map(|v| Ok((*v).to_owned())));
            let written = resolved(vars).unwrap().actor.session().map(str::to_owned);
            assert_eq!(
                found.as_ref().map(SessionId::as_str),
                written.as_deref(),
                "{vars:?}"
            );
        }
    }

    #[test]
    fn an_unusable_harness_session_is_skipped_with_a_warning() {
        for bad in [" ", "a\u{1}b", "s\u{202e}1"] {
            let identity = resolved(&[("TM_AGENT", "a"), ("CLAUDE_CODE_SESSION_ID", bad)]).unwrap();
            assert_eq!(identity.actor.session(), None, "{bad:?}");
            assert!(
                identity.warnings.iter().any(|warning| {
                    warning.starts_with("CLAUDE_CODE_SESSION_ID is not a usable session id")
                }),
                "{bad:?}: {:?}",
                identity.warnings
            );
        }
        let not_utf8 = resolve(
            |name| (name == "CODEX_THREAD_ID").then(|| Err(OsString::from("x"))),
            || Ok("jace@x.io".into()),
        )
        .unwrap();
        assert_eq!(not_utf8.actor.session(), None);
        assert_eq!(not_utf8.warnings.len(), 1, "{:?}", not_utf8.warnings);
        assert_eq!(
            resolved(&[("CODEX_THREAD_ID", "t1")]).unwrap().warnings,
            Vec::<String>::new()
        );
        let fallback = resolved(&[("CODEX_SESSION_ID", " "), ("CODEX_THREAD_ID", "t1")]).unwrap();
        assert_eq!(fallback.actor.session(), Some("t1"));
        assert_eq!(fallback.warnings, Vec::<String>::new());
        let neither = resolved(&[("CODEX_SESSION_ID", " "), ("CODEX_THREAD_ID", " ")]).unwrap();
        assert_eq!(
            neither.warnings,
            [
                "CODEX_SESSION_ID and CODEX_THREAD_ID are not usable session ids, so this session's \
              Claims record none; set TM_SESSION to name it"
            ]
        );
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
    fn an_agent_is_detected_whenever_resolve_would_find_one_or_tm_agent_is_set() {
        let detected = |vars: &[(&str, &str)]| {
            agent_detected(|name| {
                vars.iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| Ok((*v).to_owned()))
            })
        };
        assert!(!detected(&[]));
        assert!(!detected(&[("TM_ACTOR", "kim"), ("CLAUDECODE", "1")]));
        assert!(!detected(&[("CLAUDE_CODE_SESSION_ID", "")]));
        assert!(detected(&[("CODEX_THREAD_ID", "t")]));
        assert!(detected(&[("TM_AGENT", "reviewer")]));
        assert!(
            detected(&[("TM_AGENT", " ")]),
            "a blank TM_AGENT still counts"
        );
    }

    #[test]
    fn a_blank_name_is_refused_not_ignored() {
        for name in ["TM_ACTOR", "TM_AGENT"] {
            let err = resolved(&[(name, "  ")]).unwrap_err();
            assert!(err.starts_with(name), "{err}");
        }
    }
}
