//! Questions that park an Issue on a person's decision, and their answers.

use rusqlite::{Connection, OptionalExtension, params};
use tm_core::op::ClaimAction;
use tm_core::scheme::WAITING_ON;
use tm_core::{Actor, Change, Op};
use uuid::Uuid;

use crate::apply::issue_project;
use crate::claim::{Claim, current};
use crate::read::{self, CommentView};
use crate::{Draft, Store, StoreError};

impl Store {
    /// Asks `question` of `of` on `issue` in one batch: the question joins
    /// its comments and `waiting_on` names `of`. A Claim `held` by the asker's
    /// session is handed to its own Agent with no expiry, so no other Agent
    /// takes the work while it waits and any of the asker's sessions resumes
    /// it once answered. Checked inside the write: the Issue is open, waits
    /// on no one, and holds `held` or no live Claim.
    ///
    /// # Errors
    ///
    /// [`StoreError::Invalid`] for a finished or already waiting Issue,
    /// [`StoreError::ClaimChanged`] when the Claim is not `held`, or as
    /// [`Store::append_all`].
    pub fn ask(
        &mut self,
        actor: &Actor,
        issue: Uuid,
        question: &str,
        of: &str,
        held: Option<&Claim>,
    ) -> Result<Vec<Op>, StoreError> {
        let project = issue_project(&self.conn, issue)?.ok_or(StoreError::UnknownIssue(issue))?;
        let draft = |change| Draft {
            actor: actor.clone(),
            project,
            subject: issue,
            change,
        };
        let mut drafts = vec![
            draft(Change::Ask {
                body: question.to_owned(),
            }),
            draft(self.waiting_on(Some(of))),
        ];
        if let Some(held) = held {
            drafts.push(draft(Change::Claim {
                action: ClaimAction::Hand {
                    claim: held.id,
                    agent: held.agent.clone(),
                    ttl_ms: None,
                    note: question.to_owned(),
                },
            }));
        }
        let scheme = self.scheme.clone();
        let drafts = drafts.into_iter().map(|draft| (draft, None)).collect();
        self.append_with(
            drafts,
            |tx, now_ms| {
                if crate::shortlist::finished(tx, &scheme, issue)? {
                    return Err(invalid_wait(
                        "cannot wait on anyone once the Issue is finished",
                    ));
                }
                if waiting_on_now(tx, issue)?.is_some() {
                    return Err(invalid_wait("already names who the Issue waits on"));
                }
                let current = current(tx, issue)?;
                let live = current.as_ref().filter(|claim| !claim.lapsed(now_ms));
                match (live, held) {
                    (None, None) => Ok(Vec::new()),
                    (_, Some(held)) if current.as_ref() == Some(held) => Ok(Vec::new()),
                    _ => Err(StoreError::ClaimChanged(issue.into())),
                }
            },
            true,
            None,
        )
    }

    /// Answers `issue`'s open question `question`, the id of the operation
    /// that asked it, with `reply` in one batch: the reply joins its comments
    /// and `waiting_on` is cleared. A Claim the question handed to the asking
    /// Agent stays, for that Agent to resume.
    ///
    /// # Errors
    ///
    /// [`StoreError::Invalid`] when the Issue no longer waits on `question`,
    /// or as [`Store::append_all`].
    pub fn answer(
        &mut self,
        actor: &Actor,
        issue: Uuid,
        question: Uuid,
        reply: &str,
    ) -> Result<Vec<Op>, StoreError> {
        let project = issue_project(&self.conn, issue)?.ok_or(StoreError::UnknownIssue(issue))?;
        let draft = |change| Draft {
            actor: actor.clone(),
            project,
            subject: issue,
            change,
        };
        let drafts = vec![
            draft(Change::Answer {
                body: reply.to_owned(),
            }),
            draft(self.waiting_on(None)),
        ];
        self.append_guarded(drafts, |tx| {
            let open = read::open_question(tx, issue)?.map(|asked| asked.id);
            if waiting_on_now(tx, issue)?.is_none() || open != Some(question) {
                return Err(invalid_wait(
                    "changed since the question was read; read it again",
                ));
            }
            Ok(())
        })
    }

    /// `issue`'s open question: its latest question not retracted.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn open_question(&self, issue: Uuid) -> Result<Option<CommentView>, StoreError> {
        read::open_question(&self.conn, issue)
    }

    /// The answer given to the question that handed `claim` to its Agent, as
    /// (who answered, the reply), when the Claim came from `tm ask` and the
    /// question has been answered since.
    ///
    /// # Errors
    ///
    /// Any SQLite failure, or [`StoreError::Corrupt`] for a malformed row.
    pub fn answer_for(&self, claim: Uuid) -> Result<Option<(String, String)>, StoreError> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT c.actor, c.body FROM op_batches h
                 JOIN op_batches q ON q.batch = h.batch
                 JOIN comments asked ON asked.op_id = q.op_id AND asked.role = 'question'
                 JOIN comments c ON c.issue_id = asked.issue_id AND c.role = 'answer'
                   AND c.retracted = 0 AND c.hlc > asked.hlc
                 WHERE h.op_id = ?1
                 ORDER BY c.hlc DESC LIMIT 1",
                [claim.as_bytes()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(row)
    }

    fn waiting_on(&self, who: Option<&str>) -> Change {
        let label = self
            .scheme
            .field(WAITING_ON)
            .map_or_else(|| WAITING_ON.to_owned(), |field| field.label.clone());
        Change::SetField {
            field: WAITING_ON.to_owned(),
            label,
            value: who.map_or(serde_json::Value::Null, serde_json::Value::from),
            value_label: None,
        }
    }
}

fn invalid_wait(reason: &str) -> StoreError {
    StoreError::Invalid {
        field: WAITING_ON.to_owned(),
        reason: reason.to_owned(),
        valid: Vec::new(),
    }
}

fn waiting_on_now(conn: &Connection, issue: Uuid) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT txt FROM field_values WHERE issue_id = ?1 AND field = ?2",
            params![issue.as_bytes(), WAITING_ON],
            |row| row.get(0),
        )
        .optional()?)
}
