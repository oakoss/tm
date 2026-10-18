//! `tm ready`: the work an Agent can start now, and what each would unblock.

use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::json;
use uuid::Uuid;

use crate::error::CliError;
use crate::here;
use crate::list::{item, line, project_keys};
use crate::output::Output;

pub(crate) fn run(limit: Option<usize>) -> Result<Output, CliError> {
    let here = here::open()?;
    let read = here.store.read(|store| {
        let mut ready = store.ready()?;
        let total = ready.len();
        ready.truncate(limit.unwrap_or(total));
        let ids: Vec<Uuid> = ready.iter().map(|ready| ready.issue).collect();
        let views = store.issues(&ids)?;
        let unblocked: Vec<Uuid> = ready.iter().flat_map(|r| r.unblocks.clone()).collect();
        let keys: HashMap<Uuid, Option<String>> = store
            .issues(&unblocked)?
            .into_iter()
            .map(|view| (view.id, view.key))
            .collect();
        let projects = project_keys(store, &views)?;
        Ok::<_, CliError>((total, ready, views, keys, projects))
    });
    let (total, ready, views, keys, projects) = match read {
        Ok(read) => read,
        Err(err) => return Err(err.warned(here.warnings)),
    };

    let mut items = Vec::with_capacity(views.len());
    let mut text = String::new();
    for (entry, view) in ready.iter().zip(&views) {
        let unblocks: Vec<_> = entry
            .unblocks
            .iter()
            .map(|id| json!({ "id": id, "key": keys.get(id).cloned().flatten() }))
            .collect();
        let mut item = item(view, &projects);
        item["unblocks"] = json!(unblocks);
        items.push(item);

        let _ = write!(text, "{}", line(view));
        if !entry.unblocks.is_empty() {
            let named: Vec<String> = entry
                .unblocks
                .iter()
                .map(|id| {
                    keys.get(id)
                        .cloned()
                        .flatten()
                        .unwrap_or_else(|| id.to_string())
                })
                .collect();
            let _ = write!(text, "  (unblocks {})", named.join(", "));
        }
        text.push('\n');
    }
    if total == 0 {
        text.push_str("nothing is ready\n");
    } else if views.len() < total {
        let _ = writeln!(text, "({} of {total} shown)", views.len());
    }
    let mut output = Output::new(text, json!({ "issues": items, "total": total }));
    output.warnings = here.warnings;
    Ok(output)
}
