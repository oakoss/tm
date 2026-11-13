//! People's names, one record per email across a Workspace (ADR 0019).

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use tm_core::{Actor, Change, Hlc, Op, ProjectId, person};

use crate::apply::{Version, rust_int, sql_int, uuid_from};
use crate::{Draft, Store, StoreError};

/// Applies a name: the latest by clock wins, since the writes of one person
/// span Projects whose server positions do not compare.
pub(crate) fn apply(
    tx: &Transaction<'_>,
    op: &Op,
    change: &Change,
    version: &Version,
) -> Result<(), StoreError> {
    let Change::SetName { name } = change else {
        unreachable!("only a name reaches here")
    };
    if !crate::project::exists(tx, op.project)? {
        return Err(StoreError::UnknownProject(op.project));
    }
    let rank = version.rank();
    let current = tx
        .query_row(
            "SELECT hlc, replica_id, op_id FROM people WHERE id = ?1",
            [op.subject.as_bytes()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .optional()?;
    if let Some((hlc, replica, op_id)) = current {
        let current = (
            Hlc::from_raw(rust_int(hlc)?),
            uuid_from(&replica)?,
            uuid_from(&op_id)?,
        );
        if current >= rank {
            return Ok(());
        }
    }
    let (hlc, replica, op_id) = rank;
    tx.execute(
        "INSERT INTO people (id, email, name, hlc, replica_id, op_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (id) DO UPDATE SET email = excluded.email, name = excluded.name,
           hlc = excluded.hlc, replica_id = excluded.replica_id, op_id = excluded.op_id",
        params![
            op.subject.as_bytes(),
            person::key(&op.actor),
            name,
            sql_int(hlc.raw())?,
            replica.as_bytes(),
            op_id.as_bytes()
        ],
    )?;
    Ok(())
}

/// Where a name is written, only the human it names writes it.
pub(crate) fn check(actor: &Actor, subject: uuid::Uuid) -> Result<(), StoreError> {
    if actor.is_agent() || subject != person::id(actor.name()) {
        return Err(StoreError::Invalid {
            field: "name".into(),
            reason: "is written by the person it names".into(),
            valid: Vec::new(),
        });
    }
    Ok(())
}

/// The name recorded for `email`, if any.
pub(crate) fn name_of(conn: &Connection, email: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT name FROM people WHERE id = ?1",
            [person::id(email).as_bytes()],
            |row| row.get(0),
        )
        .optional()?)
}

impl Store {
    /// The operations this connection has committed since it opened, which
    /// another process's writes do not count toward.
    #[must_use]
    pub const fn appended(&self) -> usize {
        self.appended
    }

    /// The name the person writing as `email` last recorded.
    ///
    /// # Errors
    ///
    /// Any SQLite failure.
    pub fn name_of(&self, email: &str) -> Result<Option<String>, StoreError> {
        name_of(&self.conn, email)
    }

    /// The name this replica last recorded for the person writing as
    /// `email`, whatever another replica recorded since.
    fn recorded_here(&self, email: &str) -> Result<Option<String>, StoreError> {
        let payload: Option<String> = self
            .conn
            .query_row(
                // Without planner stats SQLite walks this replica's ops by
                // `ops_by_replica` instead, tens of ms on each write.
                "SELECT payload FROM ops INDEXED BY ops_target
                 WHERE issue_id = ?1 AND kind = 'set_name'
                   AND replica_id = (SELECT v FROM meta WHERE k = 'replica_id')
                 ORDER BY hlc DESC LIMIT 1",
                [person::id(email).as_bytes()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(
            payload.and_then(|payload| match serde_json::from_str(&payload) {
                Ok(Change::SetName { name }) => Some(name),
                _ => None,
            }),
        )
    }

    /// Records `name` as the human `actor`'s own, in `project`, unless it is
    /// the name this replica last recorded, so replicas whose git names
    /// differ do not each rewrite it; the operation written, if any.
    ///
    /// # Errors
    ///
    /// [`StoreError::Op`] for a name that is blank, overlong or holds a line
    /// break, [`StoreError::Invalid`] for an Agent, or as
    /// [`Store::append_all`].
    pub fn record_name(
        &mut self,
        actor: &Actor,
        project: ProjectId,
        name: &str,
    ) -> Result<Option<Op>, StoreError> {
        let name = name.trim();
        if self.recorded_here(actor.name())?.as_deref() == Some(name) {
            return Ok(None);
        }
        self.append(Draft {
            actor: actor.clone(),
            project,
            subject: person::id(actor.name()),
            change: Change::SetName {
                name: name.to_owned(),
            },
        })
        .map(Some)
    }
}
