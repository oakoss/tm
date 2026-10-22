//! Who writes an operation (ADR 0010).

/// The Actor writing: a human, or an Agent and the human it writes for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    name: String,
    agent: bool,
    on_behalf_of: Option<String>,
    session: Option<SessionId>,
}

/// A harness session's id, held to [`SessionId::RULE`] so no two sessions
/// can look alike in text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(String);

impl SessionId {
    /// The characters a session id may hold, as messages name them.
    pub const RULE: &str = "ASCII letters, digits, -, _ and .";

    /// `session` trimmed, if it is a session id.
    #[must_use]
    pub fn parse(session: &str) -> Option<Self> {
        let session = session.trim();
        is_session_id(session).then(|| Self(session.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Actor {
    /// A human writing for themselves.
    #[must_use]
    pub fn human(name: &str) -> Self {
        Self {
            name: name.trim().to_owned(),
            agent: false,
            on_behalf_of: None,
            session: None,
        }
    }

    /// An Agent writing for `human`. Names are compared trimmed and ignoring
    /// case; an Agent named as its human writes under the human's spelling
    /// and records nothing on their behalf.
    #[must_use]
    pub fn agent(name: &str, human: &str) -> Self {
        let (name, human) = (name.trim(), human.trim());
        let same = name.to_lowercase() == human.to_lowercase();
        Self {
            name: if same { human } else { name }.to_owned(),
            agent: true,
            on_behalf_of: (!same).then(|| human.to_owned()),
            session: None,
        }
    }

    /// This Agent in the harness session `session`, which tells apart
    /// sessions sharing its name; a human has none.
    #[must_use]
    pub fn in_session(mut self, session: SessionId) -> Self {
        self.session = self.agent.then_some(session);
        self
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn is_agent(&self) -> bool {
        self.agent
    }

    #[must_use]
    pub fn on_behalf_of(&self) -> Option<&str> {
        self.on_behalf_of.as_deref()
    }

    #[must_use]
    pub fn session(&self) -> Option<&str> {
        self.session.as_ref().map(SessionId::as_str)
    }
}

/// Whether `session` can name a session: ASCII letters, digits, `-`, `_` and
/// `.` only, so invisible or look-alike characters cannot make two sessions
/// look alike in text.
#[must_use]
pub fn is_session_id(session: &str) -> bool {
    !session.is_empty()
        && session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_records_the_human_it_writes_for() {
        let agent = Actor::agent("claude", "jace");
        assert_eq!(
            (agent.name(), agent.is_agent(), agent.on_behalf_of()),
            ("claude", true, Some("jace"))
        );
        let human = Actor::human("jace");
        assert_eq!(
            (human.name(), human.is_agent(), human.on_behalf_of()),
            ("jace", false, None)
        );
    }

    #[test]
    fn a_session_id_is_ascii_letters_digits_dash_underscore_and_dot() {
        for good in [
            "eeeceb72-32ea-496f-ab1d-60fb6417eead",
            "frontend-review",
            "codex.2_b",
        ] {
            assert!(is_session_id(good), "{good:?}");
        }
        for bad in [
            "",
            "a b",
            "s\u{200b}1",
            "x\u{202e}y",
            "…6417eead",
            "é",
            "s\u{7}1",
        ] {
            assert!(!is_session_id(bad), "{bad:?}");
            assert_eq!(SessionId::parse(bad), None, "{bad:?}");
        }
        assert_eq!(SessionId::parse(" s1 ").unwrap().as_str(), "s1");
    }

    #[test]
    fn only_an_agent_writes_in_a_session() {
        let s1 = || SessionId::parse("s1").unwrap();
        assert_eq!(
            Actor::agent("claude", "jace").in_session(s1()).session(),
            Some("s1")
        );
        assert_eq!(Actor::human("jace").in_session(s1()).session(), None);
    }

    #[test]
    fn names_are_trimmed_and_an_agent_named_as_its_human_writes_for_no_one() {
        let agent = Actor::agent(" claude ", " jace\n");
        assert_eq!(
            (agent.name(), agent.on_behalf_of()),
            ("claude", Some("jace"))
        );
        for (name, human) in [
            ("jace", "jace"),
            ("Jace ", "jace"),
            ("JACE@x.io", "jace@X.io"),
        ] {
            let same = Actor::agent(name, human);
            assert!(same.is_agent(), "{name}");
            assert_eq!(same.on_behalf_of(), None, "{name} writing for {human}");
            assert_eq!(same.name(), human, "the human's spelling is kept");
        }
        assert_eq!(Actor::human("  jace ").name(), "jace");
    }
}
