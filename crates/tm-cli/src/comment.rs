//! `tm comment`: a comment on an Issue, its body given or read from stdin.

use serde_json::json;
use tm_core::Change;
use tm_store::Draft;

use crate::error::CliError;
use crate::fields;
use crate::here::{self, Here};
use crate::identity;
use crate::output::Output;

pub(crate) fn run(reference: &str, body: &str) -> Result<Output, CliError> {
    here::write(|here| write(here, reference, body))
}

fn write(here: &mut Here, reference: &str, body: &str) -> Result<(Output, Vec<String>), CliError> {
    let body = if body == "-" {
        fields::read_stdin_text("body")?
    } else {
        body.to_owned()
    };
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
    let op = here.store.append(Draft {
        actor: identity.actor.clone(),
        project,
        subject: id,
        change: Change::Comment { body: body.clone() },
    })?;
    let key = here.store.issue(id)?.and_then(|view| view.key);
    let shown = key.clone().unwrap_or_else(|| id.to_string());
    let data = json!({
        "issue": { "id": id, "key": key },
        "comment": {
            "id": op.op_id,
            "actor": op.actor,
            "on_behalf_of": op.on_behalf_of,
            "body": body,
            "utc_ms": op.utc_ms,
        },
    });
    let output = Output::new(format!("commented on {shown}\n"), data);
    Ok((here.undoable(output, op.op_id)?, identity.warnings))
}
