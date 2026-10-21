//! `tm guide`: how to work with tm, built into the binary so it matches the
//! installed tm wherever an instruction file points at it.

use serde_json::json;

use crate::output::Output;

const GUIDE: &str = include_str!("guide.txt");

/// The habits `tm prime` ends with, short forms of the guide's.
pub(crate) const HABITS: [&str; 3] = [
    "tm move KEY in_progress when work starts",
    "the description holds the current plan; tm edit changes it",
    "tm comment KEY for progress, what was learned and changes of plan",
];

/// Reads no Workspace, so it works in any directory.
pub(crate) fn run() -> Output {
    Output::new(GUIDE.to_owned(), json!({ "guide": GUIDE }))
}
