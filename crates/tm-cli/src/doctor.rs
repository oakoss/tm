//! `tm doctor`: a Scheme file that cannot be read, and logged operations that
//! did not apply, lost Claims apart.

use std::fmt::Write as _;

use serde_json::{Value, json};
use tm_store::Unapplied;

use crate::error::CliError;
use crate::here;
use crate::output::{Output, printable};

pub(crate) fn run() -> Result<Output, CliError> {
    let (here, fault) = here::read_past_scheme()?;
    let (lost, problems): (Vec<Unapplied>, Vec<Unapplied>) = here
        .store
        .unapplied()?
        .into_iter()
        .partition(Unapplied::is_lost_claim);
    let entry = |row: &Unapplied| -> Value {
        json!({ "seq": row.seq, "op_id": row.op_id, "reason": row.reason })
    };
    let scheme = fault
        .as_ref()
        .map(|fault| json!({ "path": fault.path, "line": fault.line, "message": fault.message }));
    let data = json!({
        "scheme": scheme,
        "lost_claims": lost.iter().map(entry).collect::<Vec<_>>(),
        "problems": problems.iter().map(entry).collect::<Vec<_>>(),
    });

    let mut text = String::new();
    if let Some(fault) = &fault {
        let _ = writeln!(
            text,
            "the Scheme cannot be read, so every command that reads it stops:\n  {fault}"
        );
    }
    if lost.is_empty() && problems.is_empty() {
        text.push_str("every logged operation applied\n");
    }
    for (title, rows) in [
        ("operations that did not apply", &problems),
        ("Claims another take, renewal or release won", &lost),
    ] {
        if rows.is_empty() {
            continue;
        }
        let _ = writeln!(text, "{title}:");
        for row in rows {
            let _ = writeln!(text, "  {} {}", row.op_id, printable(&row.reason));
        }
    }
    // The unapplied warning every other command gives is this report's own
    // subject; that it read the Snapshot is not.
    let mut output = Output::new(text, data);
    output.warnings.extend(here.snapshot);
    Ok(output)
}
