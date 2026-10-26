//! `tm guide`: how to work with tm, built into the binary so it matches the
//! installed tm wherever an instruction file points at it.

use serde_json::json;

use crate::output::Output;

const GUIDE: &str = include_str!("guide.txt");

/// The habits `tm prime` ends with, short forms of the guide's.
pub(crate) const HABITS: [&str; 5] = [
    "tm start KEY when work starts",
    "after a merge: git fetch, then tm git scan moves what it closed to done",
    "your writes renew your Claims; a person's tm setup claude adds hooks that heartbeat between them",
    "the description holds the current plan; tm edit changes it",
    "tm comment KEY for progress, what was learned and changes of plan",
];

/// Reads no Workspace, so it works in any directory.
pub(crate) fn run() -> Output {
    Output::new(GUIDE.to_owned(), json!({ "guide": GUIDE }))
}
