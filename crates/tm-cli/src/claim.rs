//! `tm claim`, `tm heartbeat` and `tm release`: an Agent's hold on an Issue,
//! and its token.

use serde_json::json;
use tm_core::Actor;
use tm_store::{Claim, IssueView};
use uuid::Uuid;

use crate::error::CliError;
use crate::follow;
use crate::here::{self, Here};
use crate::identity;
use crate::output::Output;

/// A duration such as `90s`, `30m`, `2h` or `1d`, in milliseconds.
pub(crate) fn duration(text: &str) -> Result<u64, String> {
    match tm_core::duration::parse_ms(text) {
        Some(0) => Err("a Claim cannot expire as it is taken".into()),
        Some(ms) => Ok(ms),
        None => Err(format!(
            "expected a duration such as 30m or 2h, up to 36500d, not {text:?}"
        )),
    }
}

pub(crate) fn take(
    reference: &str,
    ttl_ms: Option<u64>,
    no_expiry: bool,
) -> Result<Output, CliError> {
    here::write(|here| {
        let issue = here.issue(reference)?;
        let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
            .map_err(|message| CliError::new("no_identity", message))?;
        let ttl_ms = if no_expiry {
            None
        } else {
            ttl_ms.or_else(|| here.found.file.claim_ttl_ms())
        };
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
            "{shown} claimed by {}; release with: tm release {shown} {}\n",
            holder(&claim),
            claim.id
        );
        let data = json!({
            "issue": { "id": issue, "key": key },
            "claim": claim_json(&claim),
        });
        let output = here.undoable(Output::new(text, data), claim.id, &identity.actor)?;
        Ok((output, identity.warnings))
    })
}

/// Renews this session's due Claims, or one Issue's at once; `tm heartbeat`.
pub(crate) fn heartbeat(reference: Option<&str>) -> Result<Output, CliError> {
    here::write(|here| {
        let issue = reference
            .map(|reference| here.issue(reference))
            .transpose()?;
        let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
            .map_err(|message| CliError::new("no_identity", message))?;
        if !identity.actor.is_agent() {
            return Err(CliError::new(
                "no_agent",
                format!(
                    "only an Agent holds Claims, and {} writes as a human",
                    identity.actor.name()
                ),
            )
            .fix("TM_AGENT=<name> tm heartbeat"));
        }
        let ops = here.store.heartbeat(&identity.actor, issue)?;
        let mut renewed = Vec::with_capacity(ops.len());
        let mut shown = Vec::with_capacity(ops.len());
        for op in &ops {
            let key = key(here, op.subject)?;
            shown.push(key.clone().unwrap_or_else(|| op.subject.to_string()));
            renewed.push(json!({ "id": op.subject, "key": key }));
        }
        let text = if shown.is_empty() {
            "no Claim is due for renewal\n".to_owned()
        } else {
            format!("renewed {}\n", shown.join(", "))
        };
        let mut output = Output::new(text, json!({ "renewed": renewed }));
        output.warnings = here.takeovers(&identity.actor, None)?;
        Ok((output, identity.warnings))
    })
}

pub(crate) fn release(reference: &str, claim: Option<Uuid>) -> Result<Output, CliError> {
    here::write(|here| {
        let issue = here.issue(reference)?;
        let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
            .map_err(|message| CliError::new("no_identity", message))?;
        let claim = match claim {
            Some(claim) => claim,
            None => own_claim(here, issue, &identity.actor, reference)?,
        };
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

/// The token of `issue`'s Claim, lapsed or not, when `writer` is the session
/// holding it or the human it is held for (ADR 0009).
fn own_claim(here: &Here, issue: Uuid, writer: &Actor, reference: &str) -> Result<Uuid, CliError> {
    let held = here
        .store
        .claim_state(issue)?
        .map(|state| state.claim().clone());
    match held {
        Some(held)
            if held.taken_by(writer)
                || (!writer.is_agent() && held.on_behalf_of == writer.name()) =>
        {
            Ok(held.id)
        }
        Some(held) => Err(CliError::new(
            "not_claim_holder",
            format!(
                "{reference} is held by {}, so name its Claim to release it",
                holder(&held)
            ),
        )
        .with("issue", issue.to_string())
        .fix(format!("tm release {reference} <claim>"))),
        None => Err(
            CliError::new("not_claim_holder", format!("{reference} is unclaimed"))
                .with("issue", issue.to_string()),
        ),
    }
}

fn key(here: &Here, issue: Uuid) -> Result<Option<String>, CliError> {
    Ok(here.store.issue(issue)?.and_then(|view| view.key))
}

/// A Claim as every command's `--json` gives it.
pub(crate) fn claim_json(claim: &Claim) -> serde_json::Value {
    json!({
        "id": claim.id,
        "agent": claim.agent,
        "on_behalf_of": claim.on_behalf_of,
        "expires_ms": claim.expires_ms,
        "session": claim.session,
    })
}

/// Who holds `claim`, as text names it: the Agent, its session when the
/// harness named one, and its human.
pub(crate) fn holder(claim: &Claim) -> String {
    format!("{} for {}", claim.taker(), claim.on_behalf_of)
}

/// Whether an Agent may take a lapsed Claim on `issue`: a take is refused on
/// an archived Issue.
pub(crate) fn takeable(issue: &IssueView) -> bool {
    !follow::archived(issue)
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
