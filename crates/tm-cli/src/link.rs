//! `tm link` and `tm unlink`: a Link written or removed from either end.

use serde_json::json;
use tm_core::link::{self, LinkType};
use tm_store::StoreError;
use uuid::Uuid;

use crate::error::CliError;
use crate::here::{self, Here};
use crate::identity;
use crate::output::Output;

pub(crate) fn run(words: &[String], present: bool) -> Result<Output, CliError> {
    let mut here = here::open()?;
    let warnings = std::mem::take(&mut here.warnings);
    match write(&mut here, words, present) {
        Ok((mut output, identity_warnings)) => {
            output.warnings = warnings;
            output.warnings.extend(identity_warnings);
            Ok(output)
        }
        Err(err) => Err(err.warned(warnings)),
    }
}

fn write(
    here: &mut Here,
    words: &[String],
    present: bool,
) -> Result<(Output, Vec<String>), CliError> {
    let (from, name, to) = match words {
        [from, name @ .., to] if !name.is_empty() => (from, name.join(" "), to),
        _ => {
            return Err(CliError::new(
                "usage",
                "expected an Issue, a Link name and an Issue, such as TM-2 is blocked by TM-1",
            ));
        }
    };
    let (link_type, reversed) = link::by_name(&name).ok_or_else(|| {
        CliError::new("invalid_value", format!("{name:?} is not a Link name"))
            .with("field", "link")
            .with("allowed", link::names())
    })?;
    let name = if reversed {
        link_type.inward
    } else {
        link_type.outward
    };
    let (from, to) = (here.issue(from)?, here.issue(to)?);
    let (source, target) = if reversed { (to, from) } else { (from, to) };
    let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
        .map_err(|message| CliError::new("no_identity", message))?;

    let op = if present {
        here.store
            .link(&identity.actor, link_type.id, source, target)?
    } else {
        match here
            .store
            .unlink_held(&identity.actor, link_type.id, source, target)
        {
            Err(StoreError::NotLinked { .. }) => {
                return Err(not_linked(here, link_type, name, from, to)?);
            }
            other => other?,
        }
    };

    let keys = here.store.issues(&[from, to])?;
    let key = |id: Uuid| {
        keys.iter()
            .find(|view| view.id == id)
            .and_then(|view| view.key.clone())
    };
    let shown = |id: Uuid| key(id).unwrap_or_else(|| id.to_string());
    let text = format!(
        "{} {}{name} {}\n",
        shown(from),
        if present { "" } else { "no longer " },
        shown(to)
    );
    let data = json!({
        "link": {
            "type": link_type.id,
            "name": name,
            "from": { "id": from, "key": key(from) },
            "to": { "id": to, "key": key(to) },
            "present": present,
        },
    });
    let output = here.undoable(Output::new(text, data), op.op_id)?;
    Ok((output, identity.warnings))
}

/// `not_linked`, naming the Links the pair does hold.
fn not_linked(
    here: &Here,
    link_type: &LinkType,
    name: &str,
    from: Uuid,
    to: Uuid,
) -> Result<CliError, CliError> {
    let held: Vec<String> = here
        .store
        .links(from)?
        .into_iter()
        .filter(|held| held.other == to)
        .map(|held| held.name)
        .collect();
    Ok(CliError::new(
        "not_linked",
        if held.is_empty() {
            format!("the two Issues hold no {name} Link, and no other Link")
        } else {
            format!(
                "the two Issues hold no {name} Link; from the first they read: {}",
                held.join(", ")
            )
        },
    )
    .with("type", link_type.id)
    .with("held", held))
}
