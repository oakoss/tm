//! The Agent harnesses tm knows by their session: what names a session, and
//! how a person resumes one.

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Harness {
    /// Claude Code.
    Claude,
    /// Codex.
    Codex,
}

impl Harness {
    /// Every harness, in the order their session variables are read: Codex
    /// first, as it is the one launched from Claude Code.
    pub(crate) const ALL: [Self; 2] = [Self::Codex, Self::Claude];

    /// The name the commands take, and `session_dirs` stores.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub(crate) fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|harness| harness.name() == name)
    }

    /// The variables holding its session id, in the order they are read.
    pub(crate) fn session_vars(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["CLAUDE_CODE_SESSION_ID"],
            Self::Codex => &["CODEX_SESSION_ID", "CODEX_THREAD_ID"],
        }
    }

    /// The command that resumes `session` from `dir`, already quoted.
    pub(crate) fn resume(self, dir: &str, session: &str) -> String {
        match self {
            Self::Claude => format!("cd {dir} && claude -r {session}"),
            Self::Codex => format!("cd {dir} && codex resume {session}"),
        }
    }

    /// Why a resume command it printed may no longer work.
    pub(crate) fn expiry(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some(
                "Claude deletes a session unused for 30 days by default, so an old one may not resume.",
            ),
            Self::Codex => None,
        }
    }
}
