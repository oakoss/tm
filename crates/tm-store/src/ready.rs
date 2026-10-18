//! The ready queue: work an Agent can start now.

use std::collections::HashMap;

use tm_core::filter::{Context, parse, resolve};
use tm_core::link::BLOCKS;
use uuid::Uuid;

use crate::apply::uuid_from;
use crate::{Store, StoreError, query};

/// An Issue in the ready queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    pub issue: Uuid,
    /// Open Issues this one is the last open blocker of.
    pub unblocks: Vec<Uuid>,
}

impl Store {
    /// Unarchived, unclaimed Issues in the todo category that no open Issue
    /// blocks, most urgent first and then oldest. An Issue is open until it
    /// is done, cancelled or archived. Parents and children do not affect
    /// readiness.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn ready(&self) -> Result<Vec<Ready>, StoreError> {
        self.read(|store| {
            let category = |filter: &str| {
                resolve(&parse(filter)?, &store.scheme, Context { me: None })
                    .map_err(StoreError::from)
            };
            let todo = category("category:todo")?;
            let open = category("-category:done,cancelled")?;

            let (sql, params) = query::ready(&todo, &open, BLOCKS);
            let mut stmt = store.conn.prepare(&sql)?;
            let ready = stmt
                .query_map(rusqlite::params_from_iter(params), |row| {
                    row.get::<_, Vec<u8>>(0)
                })?
                .map(|id| uuid_from(&id?))
                .collect::<Result<Vec<_>, _>>()?;

            let (sql, params) = query::open_blocks(&open, BLOCKS);
            let mut stmt = store.conn.prepare(&sql)?;
            let mut blocks: Vec<(Uuid, Uuid)> = Vec::new();
            let mut rows = stmt.query(rusqlite::params_from_iter(params))?;
            while let Some(row) = rows.next()? {
                blocks.push((
                    uuid_from(&row.get::<_, Vec<u8>>(0)?)?,
                    uuid_from(&row.get::<_, Vec<u8>>(1)?)?,
                ));
            }
            let mut blockers: HashMap<Uuid, usize> = HashMap::new();
            for (_, dst) in &blocks {
                *blockers.entry(*dst).or_default() += 1;
            }
            let mut held: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
            for (src, dst) in blocks {
                if blockers[&dst] == 1 {
                    held.entry(src).or_default().push(dst);
                }
            }
            Ok(ready
                .into_iter()
                .map(|issue| {
                    let mut unblocks = held.remove(&issue).unwrap_or_default();
                    unblocks.sort();
                    Ready { issue, unblocks }
                })
                .collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Probing every pair of open Issues took 10.9 s on 10,000 Issues and
    /// 3,000 Links; scanning the `blocks` Links took 17 ms.
    #[test]
    fn open_blocks_scans_the_links_instead_of_probing_pairs() {
        let dir = std::env::temp_dir().join(format!("tm-store-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("store.db")).unwrap();
        let open = resolve(
            &parse("-category:done,cancelled").unwrap(),
            &store.scheme,
            Context { me: None },
        )
        .unwrap();
        let (sql, params) = query::open_blocks(&open, BLOCKS);
        let plan: Vec<String> = store
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map(rusqlite::params_from_iter(params), |row| row.get(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(
            plan.iter()
                .any(|step| step == "SEARCH links USING PRIMARY KEY (type=?)"),
            "{plan:?}"
        );
    }
}
