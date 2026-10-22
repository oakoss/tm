//! `tm claim`, `tm heartbeat`, `tm release` and `tm claims`: an Agent's hold
//! on an Issue, and its token.

use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::json;
use tm_core::Actor;
use tm_store::{Claim, IssueView, StoreError};
use uuid::Uuid;

use crate::error::CliError;
use crate::follow;
use crate::here::{self, Here};
use crate::identity;
use crate::list::line_within;
use crate::output::{Output, local_time, span};
use crate::prime::TITLE_BYTES;

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
        let lasts = match (claim.expires_ms, claim.ttl_ms) {
            (Some(at), Some(ttl)) => format!(
                " until {} ({}); your writes renew it, and tm heartbeat keeps it through quiet work",
                local_time(at),
                span(i64::try_from(ttl).unwrap_or(i64::MAX))
            ),
            _ => " with no expiry".to_owned(),
        };
        let text = format!(
            "{shown} claimed by {}{lasts}\nClaim {}; release with: tm release {shown}\n",
            holder(&claim),
            claim.id
        );
        let data = json!({
            "issue": { "id": issue, "key": key },
            "claim": claim_json(&claim, here.store.utc_ms()?),
        });
        let output = here.undoable(Output::new(text, data), claim.id, &identity.actor)?;
        Ok((output, identity.warnings))
    })
}

/// What a heartbeat did.
pub(crate) struct Beat {
    /// Each Issue whose Claim it renewed, with its Key.
    pub(crate) renewed: Vec<(Uuid, Option<String>)>,
    /// Each Claim that took an Issue over from this session, by id, with the
    /// warning that says so.
    pub(crate) taken_over: Vec<(Uuid, String)>,
    /// Why whether any was taken over is unknown, when it is.
    pub(crate) unchecked: Option<String>,
    /// The store's and the identity's warnings, then a failed publish's.
    pub(crate) warnings: (Vec<String>, Option<String>),
}

/// Renews this session's due Claims, or one Issue's at once; `tm heartbeat`.
pub(crate) fn heartbeat(reference: Option<&str>) -> Result<Output, CliError> {
    let beat = beat(reference)?;
    let shown: Vec<String> = beat
        .renewed
        .iter()
        .map(|(issue, key)| key.clone().unwrap_or_else(|| issue.to_string()))
        .collect();
    let text = if shown.is_empty() {
        "no Claim is due for renewal\n".to_owned()
    } else {
        format!("renewed {}\n", shown.join(", "))
    };
    let renewed: Vec<_> = beat
        .renewed
        .iter()
        .map(|(issue, key)| json!({ "id": issue, "key": key }))
        .collect();
    let mut output = Output::new(text, json!({ "renewed": renewed }));
    let (mut warnings, published) = beat.warnings;
    warnings.extend(beat.taken_over.into_iter().map(|(_, warning)| warning));
    warnings.extend(beat.unchecked);
    warnings.extend(published);
    output.warnings = warnings;
    Ok(output)
}

/// [`heartbeat`]'s work, for it and for a hook to report as each needs.
pub(crate) fn beat(reference: Option<&str>) -> Result<Beat, CliError> {
    let (beat, before, published) = here::write_with(|here| {
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
        for op in &ops {
            renewed.push((op.subject, key(here, op.subject)?));
        }
        let (taken_over, unchecked) = match here.takeovers(&identity.actor, None) {
            Ok(taken_over) => (taken_over, None),
            Err(err) => (
                Vec::new(),
                Some(format!(
                    "renewed, but whether another Agent took over your Claims is unknown: {}",
                    err.message
                )),
            ),
        };
        let beat = Beat {
            renewed,
            taken_over,
            unchecked,
            warnings: (Vec::new(), None),
        };
        Ok((beat, identity.warnings))
    })?;
    Ok(Beat {
        warnings: (before, published),
        ..beat
    })
}

/// Every held Claim and every lapsed one on an Issue an Agent may take;
/// `tm claims`.
pub(crate) fn list() -> Result<Output, CliError> {
    let here = here::read()?;
    let read = here.store.read(|store| {
        let claims = store.claims()?;
        let ids: Vec<Uuid> = claims
            .held
            .iter()
            .chain(&claims.lapsed)
            .map(|claim| claim.issue)
            .collect();
        let views: HashMap<Uuid, IssueView> = store
            .issues(&ids)?
            .into_iter()
            .map(|view| (view.id, view))
            .collect();
        Ok::<_, CliError>((claims, views))
    });
    let (claims, views) = read.map_err(|err| err.warned(here.warnings.clone()))?;
    let now_ms = claims.at_ms;
    let mut text = String::new();
    let mut section = |heading: &str, claims: &[Claim]| {
        let shown: Vec<(&Claim, &IssueView)> = claims
            .iter()
            .filter_map(|claim| Some((claim, views.get(&claim.issue)?)))
            .collect();
        let _ = writeln!(text, "{heading} ({}):", shown.len());
        if shown.is_empty() {
            let _ = writeln!(text, "  none");
        }
        let mut items = Vec::with_capacity(shown.len());
        for (claim, view) in shown {
            let _ = writeln!(
                text,
                "  {}  ({}, {})",
                line_within(view, TITLE_BYTES),
                holder(claim),
                expiry(claim, now_ms)
            );
            items.push(json!({
                "issue": { "id": view.id, "key": view.key },
                "claim": claim_json(claim, now_ms),
            }));
        }
        items
    };
    let held = section("Held", &claims.held);
    let lapsed: Vec<Claim> = claims
        .lapsed
        .into_iter()
        .filter(|claim| views.get(&claim.issue).is_some_and(takeable))
        .collect();
    let lapsed = section("Lapsed, any Agent may take", &lapsed);
    let mut output = Output::new(text, json!({ "held": held, "lapsed": lapsed }));
    output.warnings = here.warnings;
    Ok(output)
}

/// Releases every Claim this session took, lapsed ones included;
/// `tm release --mine`.
pub(crate) fn release_mine() -> Result<Output, CliError> {
    here::write(|here| {
        let identity = identity::resolve(identity::env, || identity::git_email(&here.found.dir))
            .map_err(|message| CliError::new("no_identity", message))?;
        let writer = &identity.actor;
        if !writer.is_agent() {
            return Err(CliError::new(
                "no_agent",
                format!(
                    "only an Agent holds Claims, and {} writes as a human",
                    writer.name()
                ),
            )
            .fix("TM_AGENT=<name> tm release --mine"));
        }
        let claims = here.store.claims_by(
            writer.name(),
            writer.on_behalf_of().unwrap_or(writer.name()),
        )?;
        let (mut released, mut shown, mut failed) = (Vec::new(), Vec::new(), Vec::new());
        for claim in claims.held.iter().chain(&claims.lapsed) {
            if !claim.taken_by(writer) {
                continue;
            }
            // A failure must not keep the rest held, so an unread Key falls
            // back to the id; a Claim another Agent took over is not this
            // session's to release.
            let key = key(here, claim.issue).ok().flatten();
            let name = key.clone().unwrap_or_else(|| claim.issue.to_string());
            match here.store.release(writer, claim.issue, claim.id) {
                Ok(_) => {
                    shown.push(name);
                    released.push(json!({ "id": claim.issue, "key": key, "claim": claim.id }));
                }
                Err(StoreError::NotClaimHolder { .. }) => {}
                Err(err) => failed.push(format!("{name}: {}", CliError::from(err).message)),
            }
        }
        if !failed.is_empty() {
            let done = if shown.is_empty() {
                String::new()
            } else {
                format!("released {}; ", shown.join(", "))
            };
            return Err(CliError::new(
                "release_failed",
                format!("{done}could not release {}", failed.join("; ")),
            )
            .with("released", json!(released)));
        }
        let text = if shown.is_empty() {
            "this session holds no Claims\n".to_owned()
        } else {
            format!("released {}\n", shown.join(", "))
        };
        Ok((
            Output::new(text, json!({ "released": released })),
            identity.warnings,
        ))
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

/// A Claim as every command's `--json` gives it at `now_ms`, with the time
/// left so an Agent need not read a clock; 0 once lapsed.
pub(crate) fn claim_json(claim: &Claim, now_ms: i64) -> serde_json::Value {
    json!({
        "id": claim.id,
        "agent": claim.agent,
        "on_behalf_of": claim.on_behalf_of,
        "expires_ms": claim.expires_ms,
        "expires_in_ms": claim.expires_ms.map(|at| (at - now_ms).max(0)),
        "session": claim.session,
    })
}

/// A Claim's expiry at `now_ms`: `expires in 1h 12m`, `lapsed` and when in
/// local time, or `no expiry`.
pub(crate) fn expiry(claim: &Claim, now_ms: i64) -> String {
    match claim.expires_ms {
        None => "no expiry".to_owned(),
        Some(at) if claim.lapsed(now_ms) => format!("lapsed {}", local_time(at)),
        Some(at) => format!("expires in {}", span(at - now_ms)),
    }
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
