//! Who writes an operation (ADR 0010).

/// The Actor writing, and the human an Agent writes for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    name: String,
    on_behalf_of: Option<String>,
}

impl Actor {
    /// A human writing for themselves.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            on_behalf_of: None,
        }
    }

    /// An Agent writing for `human`. An Agent named exactly as the human is the
    /// human writing, so nothing is recorded on their behalf.
    #[must_use]
    pub fn agent(name: impl Into<String>, human: impl Into<String>) -> Self {
        let name = name.into();
        let human = human.into();
        Self {
            on_behalf_of: (human != name).then_some(human),
            name,
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn on_behalf_of(&self) -> Option<&str> {
        self.on_behalf_of.as_deref()
    }
}

impl From<&str> for Actor {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for Actor {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_records_the_human_it_writes_for() {
        let agent = Actor::agent("claude", "jace");
        assert_eq!(
            (agent.name(), agent.on_behalf_of()),
            ("claude", Some("jace"))
        );
        let human = Actor::agent("jace", "jace");
        assert_eq!((human.name(), human.on_behalf_of()), ("jace", None));
        assert_eq!(Actor::from("jace"), human);
    }
}
