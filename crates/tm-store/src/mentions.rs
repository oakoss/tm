//! Who a mention may name (ADR 0019).

use std::collections::BTreeSet;

use tm_core::mention::{Known, Mentioned};
use tm_core::person;

use crate::{Store, StoreError};

impl Store {
    /// The Actors a mention may name: who has written to the Workspace and
    /// whom an Agent wrote for, Assignees and Short-list Actors, each person
    /// with the name tm holds for them. A Short-list Actor named as an Agent
    /// is that Agent, not a person.
    ///
    /// # Errors
    ///
    /// Any SQLite failure.
    pub fn mentionable(&self) -> Result<Vec<Known>, StoreError> {
        let mut agents = BTreeSet::new();
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT actor, on_behalf_of FROM ops WHERE on_behalf_of IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        for row in rows {
            let (name, human): (String, String) = row?;
            agents.insert((name, person::key(&human)));
        }
        let agent_names: BTreeSet<String> =
            agents.iter().map(|(name, _)| name.to_lowercase()).collect();
        let mut people = BTreeSet::new();
        let mut stmt = self.conn.prepare(
            "SELECT actor FROM ops WHERE on_behalf_of IS NULL
             UNION SELECT on_behalf_of FROM ops WHERE on_behalf_of IS NOT NULL
             UNION SELECT assignee FROM issues WHERE assignee IS NOT NULL
             UNION SELECT actor FROM shortlist_actors",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            let key = person::key(&row?);
            if !key.is_empty() && !agent_names.contains(&key) {
                people.insert(key);
            }
        }
        let mut known = Vec::with_capacity(people.len() + agents.len());
        for key in people {
            let name = self.name_of(&key)?;
            known.push(Known {
                actor: Mentioned::Person(key),
                name,
            });
        }
        known.extend(agents.into_iter().map(|(name, human)| Known {
            actor: Mentioned::Agent { name, human },
            name: None,
        }));
        Ok(known)
    }
}
