//! `tm claim` and `tm release`: an Agent's hold on an Issue, and its token.

use serde_json::json;
use tm_store::Claim;
use uuid::Uuid;

use crate::error::CliError;
use crate::here::{self, Here};
use crate::identity;
use crate::output::Output;

/// A duration such as `90s`, `30m`, `2h` or `1d`, in milliseconds.
pub(crate) fn duration(text: &str) -> Result<u64, String> {
    let unit_ms = match text.bytes().last() {
        Some(b's') => Some(1_000),
        Some(b'm') => Some(60_000),
        Some(b'h') => Some(3_600_000),
        Some(b'd') => Some(86_400_000),
        _ => None,
    };
    // The unit is one ASCII byte, so cutting it off stays on a char boundary.
    let ms = unit_ms.and_then(|unit_ms| {
        let count = &text[..text.len() - 1];
        if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        count.parse::<u64>().ok()?.checked_mul(unit_ms)
    });
    match ms {
        Some(0) => Err("a Claim cannot expire as it is taken".into()),
        Some(ms) => Ok(ms),
        None => Err(format!(
            "expected a duration such as 30m or 2h, not {text:?}"
        )),
    }
}

pub(crate) fn take(reference: &str, ttl_ms: Option<u64>) -> Result<Output, CliError> {
    with_here(|here| {
        let issue = here.issue(reference)?;
        let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
            .map_err(|message| CliError::new("no_identity", message))?;
        let claim = here
            .store
            .take(&identity.actor, issue, ttl_ms)
            .map_err(|err| {
                let err = CliError::from(err);
                if err.code == "no_agent" {
                    err.fix(format!("TM_AGENT=<name> tm claim {reference}"))
                } else {
                    err
                }
            })?;
        let key = key(here, issue)?;
        let shown = key.clone().unwrap_or_else(|| issue.to_string());
        let text = format!(
            "{shown} claimed by {} for {}; release with: tm release {shown} {}\n",
            claim.agent, claim.on_behalf_of, claim.id
        );
        let data = json!({
            "issue": { "id": issue, "key": key },
            "claim": view(&claim),
        });
        let output = here.undoable(Output::new(text, data), claim.id)?;
        Ok((output, identity.warnings))
    })
}

pub(crate) fn release(reference: &str, claim: Uuid) -> Result<Output, CliError> {
    with_here(|here| {
        let issue = here.issue(reference)?;
        let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
            .map_err(|message| CliError::new("no_identity", message))?;
        let op = here.store.release(&identity.actor, issue, claim)?;
        let key = key(here, issue)?;
        let shown = key.clone().unwrap_or_else(|| issue.to_string());
        let data = json!({
            "issue": { "id": issue, "key": key },
            "released": claim,
            "batch": op.op_id,
        });
        Ok((
            Output::new(format!("released {shown}\n"), data),
            identity.warnings,
        ))
    })
}

fn with_here(
    write: impl FnOnce(&mut Here) -> Result<(Output, Vec<String>), CliError>,
) -> Result<Output, CliError> {
    let mut here = here::open()?;
    let warnings = std::mem::take(&mut here.warnings);
    match write(&mut here) {
        Ok((mut output, identity_warnings)) => {
            output.warnings = warnings;
            output.warnings.extend(identity_warnings);
            Ok(output)
        }
        Err(err) => Err(err.warned(warnings)),
    }
}

fn key(here: &Here, issue: Uuid) -> Result<Option<String>, CliError> {
    Ok(here.store.issue(issue)?.and_then(|view| view.key))
}

fn view(claim: &Claim) -> serde_json::Value {
    json!({
        "id": claim.id,
        "agent": claim.agent,
        "on_behalf_of": claim.on_behalf_of,
        "expires_ms": claim.expires_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_is_a_count_and_one_unit() {
        assert_eq!(duration("90s"), Ok(90_000));
        assert_eq!(duration("30m"), Ok(1_800_000));
        assert_eq!(duration("2h"), Ok(7_200_000));
        assert_eq!(duration("1d"), Ok(86_400_000));
        for bad in ["", "h", "2", "2 h", "-2h", "+2h", "2w", "1.5h", "2é", "é"] {
            assert!(duration(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            duration("0m"),
            Err("a Claim cannot expire as it is taken".into())
        );
        assert!(duration(&format!("{}d", u64::MAX)).is_err(), "overflow");
    }
}
