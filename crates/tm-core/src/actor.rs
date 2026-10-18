//! Who writes an operation (ADR 0010).

/// The Actor writing: a human, or an Agent and the human it writes for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    name: String,
    agent: bool,
    on_behalf_of: Option<String>,
}

impl Actor {
    /// A human writing for themselves.
    #[must_use]
    pub fn human(name: &str) -> Self {
        Self {
            name: name.trim().to_owned(),
            agent: false,
            on_behalf_of: None,
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
        }
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
