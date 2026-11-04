//! `tm comment`: a comment on an Issue, its body given or read from stdin, or
//! a correction of one.

use serde_json::json;
use tm_core::{Actor, Change};
use tm_store::{CommentView, Draft};
use uuid::Uuid;

use crate::error::CliError;
use crate::fields;
use crate::here::{self, Here};
use crate::identity;
use crate::output::{Output, id_by_part, id_has_part, printable, short};
use crate::show::imported_id;

pub(crate) fn run(reference: &str, body: &str, edit: Option<&str>) -> Result<Output, CliError> {
    here::write(|here| write(here, reference, body, edit))
}

fn write(
    here: &mut Here,
    reference: &str,
    body: &str,
    edit: Option<&str>,
) -> Result<(Output, Vec<String>), CliError> {
    // Comments are append-only, so a blank one from a failed `$(cmd)` would
    // stay for good.
    if body.trim().is_empty() {
        return Err(CliError::new("invalid_value", "a comment cannot be blank")
            .with("field", "body")
            .with("reason", "is blank"));
    }
    let id = here.issue(reference)?;
    let project = here
        .store
        .issue(id)?
        .map(|view| view.project)
        .ok_or_else(|| CliError::new("not_found", format!("no Issue is {reference}")))?;
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;
    // The comment, and the id `tm show` gives it.
    let edited = match edit {
        Some(which) => {
            let comments = here.store.comments(id)?;
            let comment = pick_comment(&comments, reference, which, &identity.actor)?;
            let named = comments
                .iter()
                .find(|held| held.id == comment)
                .and_then(imported_id)
                .map_or_else(|| short(comment), str::to_owned);
            Some((comment, named))
        }
        None => None,
    };
    // Read once all else is checked, so a mistake keeps the piped text.
    let body = if body == "-" {
        let read = fields::read_stdin_text("body")?;
        if read.trim().is_empty() {
            return Err(fields::refused("body", "is blank"));
        }
        read
    } else {
        body.to_owned()
    };
    let change = match edited {
        Some((comment, _)) => Change::EditComment {
            comment,
            body: body.clone(),
        },
        None => Change::Comment { body: body.clone() },
    };
    let op = here.store.append(Draft {
        actor: identity.actor.clone(),
        project,
        subject: id,
        change,
    })?;
    let key = here.store.issue(id)?.and_then(|view| view.key);
    let shown = key.clone().unwrap_or_else(|| id.to_string());
    let (text, comment) = match edited {
        Some((comment, named)) => (
            format!("edited comment {} on {shown}\n", printable(&named)),
            json!({ "id": comment, "body": body, "edited_utc_ms": op.utc_ms }),
        ),
        None => (
            format!("commented on {shown}\n"),
            json!({
                "id": op.op_id,
                "actor": op.actor,
                "on_behalf_of": op.on_behalf_of,
                "body": body,
                "utc_ms": op.utc_ms,
            }),
        ),
    };
    let data = json!({ "issue": { "id": id, "key": key }, "comment": comment });
    let output = Output::new(text, data);
    Ok((
        here.undoable(output, op.op_id, &identity.actor)?,
        identity.warnings,
    ))
}

/// The comment `which` names among an Issue's `comments`: `last`, a person's
/// own latest plain comment, the id an import brought it with, such as
/// `CMT-243`, or a unique start or end of its id. An Agent must name its
/// comment, since other sessions may write under its name.
fn pick_comment(
    comments: &[CommentView],
    reference: &str,
    which: &str,
    actor: &Actor,
) -> Result<Uuid, CliError> {
    if which == "last" {
        if actor.is_agent() {
            return Err(CliError::new(
                "comment_required",
                format!(
                    "{} is an Agent, and other sessions may write as it; name the comment by its id",
                    actor.name()
                ),
            )
            .fix(format!("tm comment {reference} --edit '<comment>' '<text>'")));
        }
        return comments
            .iter()
            .rev()
            .find(|comment| {
                comment.actor.to_lowercase() == actor.name().to_lowercase()
                    && comment.on_behalf_of.is_none()
                    && comment.role.is_none()
                    && !comment.retracted
            })
            .map(|comment| comment.id)
            .ok_or_else(|| {
                CliError::new(
                    "not_found",
                    format!("{} has no comment on {reference} to edit", actor.name()),
                )
                .with("kind", "comment")
            });
    }
    let imported: Vec<Uuid> = comments
        .iter()
        .filter(|comment| imported_id(comment).is_some_and(|id| id.eq_ignore_ascii_case(which)))
        .map(|comment| comment.id)
        .collect();
    let hex = which.chars().all(|c| c == '-' || c.is_ascii_hexdigit());
    let id = match imported.as_slice() {
        [only] => *only,
        [] if !hex => {
            return Err(
                CliError::new("not_found", format!("{reference} has no comment {which}"))
                    .with("kind", "comment")
                    .with("reference", which),
            );
        }
        [] => {
            let within = format!("on {reference}");
            id_by_part(which, ("comment", &within), |digits| {
                let ids = comments.iter().map(|comment| comment.id);
                Ok(ids.filter(|id| id_has_part(*id, digits)).collect())
            })?
        }
        several => {
            return Err(CliError::new(
                "invalid_value",
                format!("{which} names more than one comment on {reference}; give its id"),
            )
            .with("field", "comment")
            .with(
                "matches",
                several.iter().map(Uuid::to_string).collect::<Vec<_>>(),
            ));
        }
    };
    // The store lets undo rewrite a hidden comment; a person edits only what
    // they can see.
    if comments
        .iter()
        .any(|comment| comment.id == id && comment.retracted)
    {
        return Err(CliError::new(
            "invalid_value",
            format!(
                "comment {} is retracted; undoing its retraction shows it again",
                short(id)
            ),
        )
        .with("field", "comment"));
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use tm_store::CommentRole;

    use super::*;

    fn said(actor: &str, on_behalf_of: Option<&str>) -> CommentView {
        CommentView {
            id: Uuid::now_v7(),
            actor: actor.into(),
            on_behalf_of: on_behalf_of.map(Into::into),
            body: "x".into(),
            utc_ms: 0,
            retracted: false,
            role: None,
            options: Vec::new(),
            edited_ms: None,
            source: None,
            import_id: None,
        }
    }

    fn imported(id: &str) -> CommentView {
        CommentView {
            import_id: Some(id.into()),
            ..said("jace", None)
        }
    }

    #[test]
    fn an_imported_comment_is_named_by_its_source_id_in_any_case_and_once() {
        let named = imported("CMT-243");
        let comments = [
            said("jace", None),
            named.clone(),
            imported("cmt-7"),
            imported("CMT-7"),
        ];
        let jace = Actor::human("jace");
        let pick = |which: &str| pick_comment(&comments, "TM-1", which, &jace);
        assert_eq!(pick("CMT-243").unwrap(), named.id);
        assert_eq!(pick("cmt-243").unwrap(), named.id);
        assert_eq!(pick("CMT-24").unwrap_err().code, "not_found");
        assert_eq!(
            pick("CMT-7").unwrap_err().code,
            "invalid_value",
            "two read the same"
        );
    }

    #[test]
    fn last_is_the_persons_own_latest_shown_plain_comment() {
        let own = said("Éva", None);
        let retracted = CommentView {
            retracted: true,
            ..said("éva", None)
        };
        let asked = CommentView {
            role: Some(CommentRole::Question),
            ..said("éva", None)
        };
        let comments = [
            own.clone(),
            retracted,
            asked,
            said("claude", Some("éva")),
            said("kim", None),
        ];
        let eva = Actor::human("éva");
        assert_eq!(
            pick_comment(&comments, "TM-1", "last", &eva).unwrap(),
            own.id
        );

        let err = pick_comment(&comments, "TM-1", "last", &Actor::human("lee")).unwrap_err();
        assert_eq!(err.code, "not_found");
        let agent = Actor::agent("claude", "éva");
        let err = pick_comment(&comments, "TM-1", "last", &agent).unwrap_err();
        assert_eq!(err.code, "comment_required");
    }

    #[test]
    fn an_id_names_one_shown_comment_by_its_start_or_end() {
        let shown = said("jace", None);
        let hidden = CommentView {
            retracted: true,
            ..said("jace", None)
        };
        let comments = [shown.clone(), hidden.clone()];
        let jace = Actor::human("jace");
        let pick = |which: &str| pick_comment(&comments, "TM-1", which, &jace);

        assert_eq!(pick(&short(shown.id)).unwrap(), shown.id);
        assert_eq!(pick(&shown.id.to_string()).unwrap(), shown.id);
        let start = &shown.id.simple().to_string()[..8];
        assert_eq!(
            pick(start).unwrap_err().code,
            "invalid_value",
            "both share it"
        );
        assert_eq!(pick(&short(hidden.id)).unwrap_err().code, "invalid_value");
        assert_eq!(pick("ffffffffffff").unwrap_err().code, "not_found");
        assert_eq!(pick("abc").unwrap_err().code, "invalid_value");
    }
}
