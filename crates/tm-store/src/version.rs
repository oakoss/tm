//! An Issue's version, and writes refused for a field changed since the
//! version they were based on. The version is this Replica's log position,
//! checked when a write is made and never recorded in it; sync merges by
//! ADR 0006 instead.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use tm_core::Change;
use uuid::Uuid;

use crate::{Draft, IssueView, Provenance, Store, StoreError, read};

/// The log position of the latest write that set one of the Issue's fields
/// or set elements; 0 for none. A new comment, a Claim or a renewal moves
/// nothing.
pub(crate) fn version(conn: &Connection, issue: Uuid) -> Result<i64, StoreError> {
    Ok(conn.query_row(
        "SELECT coalesce(max(o.seq), 0)
         FROM field_versions v JOIN ops o ON o.op_id = v.op_id
         WHERE v.issue_id = ?1",
        [issue.as_bytes()],
        |row| row.get(0),
    )?)
}

/// The log position of the write `field` currently holds on `issue`.
fn field_seq(conn: &Connection, issue: Uuid, field: &str) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT o.seq FROM field_versions v JOIN ops o ON o.op_id = v.op_id
             WHERE v.issue_id = ?1 AND v.field = ?2 AND v.elem = ''",
            rusqlite::params![issue.as_bytes(), field],
            |row| row.get(0),
        )
        .optional()?)
}

/// Refuses with [`StoreError::Stale`] when one of `fields` on `issue` was
/// written after `version`, naming each with its current value. A version
/// ahead of the Issue's was not read from it, and would check nothing.
pub(crate) fn check<'a>(
    conn: &Connection,
    scheme: &tm_core::Scheme,
    (issue, based_on): (Uuid, i64),
    fields: impl IntoIterator<Item = &'a str>,
) -> Result<(), StoreError> {
    let now = version(conn, issue)?;
    if based_on > now {
        return Err(StoreError::Invalid {
            field: "--if-version".into(),
            reason: format!(
                "{based_on} is ahead of the Issue's version {now}; read it with tm show"
            ),
            valid: Vec::new(),
        });
    }
    let mut changed = Vec::new();
    for field in fields {
        if !changed.iter().any(|seen| seen == field)
            && field_seq(conn, issue, field)?.is_some_and(|seq| seq > based_on)
        {
            changed.push(field.to_owned());
        }
    }
    if changed.is_empty() {
        return Ok(());
    }
    let view = read::issues(conn, scheme, &[issue])?.pop();
    let current: BTreeMap<String, Value> = changed
        .iter()
        .map(|field| {
            let value = view
                .as_ref()
                .and_then(|view| view.fields.get(field).cloned())
                .unwrap_or(Value::Null);
            (field.clone(), value)
        })
        .collect();
    Err(StoreError::Stale {
        issue: issue.into(),
        based_on,
        version: now,
        current,
    })
}

/// The single-value fields a local batch sets on `issue`. Set elements merge
/// one by one, so adding a label never conflicts with another label.
pub(crate) fn written(drafts: &[(Draft, Option<Provenance>)], issue: Uuid) -> Vec<&str> {
    drafts
        .iter()
        .filter(|(draft, provenance)| provenance.is_none() && draft.subject == issue)
        .filter_map(|(draft, _)| match &draft.change {
            Change::SetField { field, .. } => Some(field.as_str()),
            _ => None,
        })
        .collect()
}

impl Store {
    /// The Issue and its version, read together, so a write based on that
    /// version is based on what the view shows.
    ///
    /// # Errors
    ///
    /// As [`Store::issue`].
    pub fn issue_with_version(&self, issue: Uuid) -> Result<Option<(IssueView, i64)>, StoreError> {
        self.read(|store| {
            let Some(view) = store.issue(issue)? else {
                return Ok(None);
            };
            Ok(Some((view, version(&store.conn, issue)?)))
        })
    }

    /// `issue`'s version, as [`version`] defines it.
    ///
    /// # Errors
    ///
    /// Any SQLite failure.
    pub fn issue_version(&self, issue: Uuid) -> Result<i64, StoreError> {
        version(&self.conn, issue)
    }

    /// Later writes to `issue`'s fields are refused with
    /// [`StoreError::Stale`] when a field they set changed after `version`.
    pub fn base_writes_on(&mut self, issue: Uuid, version: i64) {
        self.based_on = Some((issue, version));
    }

    /// Refuses as a write based on `version` would when it sets `fields` on
    /// `issue`. The write checks again; this only fails early, such as
    /// before piped text is read.
    ///
    /// # Errors
    ///
    /// [`StoreError::Stale`], or any SQLite failure.
    pub fn check_version(
        &self,
        issue: Uuid,
        version: i64,
        fields: &[&str],
    ) -> Result<(), StoreError> {
        self.read(|store| {
            check(
                &store.conn,
                &store.scheme,
                (issue, version),
                fields.iter().copied(),
            )
        })
        .map_err(|err| err.named(&self.conn))
    }
}
