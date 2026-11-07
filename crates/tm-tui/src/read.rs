//! Issues as the screens show them: their text safe for the terminal, and
//! the version a write from the screen is checked against.

use tm_core::filter::Context;
use tm_core::text::printable;
use tm_store::{IssueView, Store, StoreError};
use uuid::Uuid;

/// One Issue a filter matched, as read.
pub(crate) struct Read {
    pub(crate) id: Uuid,
    pub(crate) key: String,
    pub(crate) version: i64,
    view: IssueView,
}

impl Read {
    pub(crate) fn field(&self, name: &str) -> String {
        field(&self.view, name)
    }
}

/// `view`'s `name` safe for a screen cell, `-` when unset.
pub(crate) fn field(view: &IssueView, name: &str) -> String {
    let text = view.fields.get(name).and_then(|value| value.as_str());
    // A tab passes `printable` but would move a screen cell's cursor.
    printable(text.unwrap_or("-")).replace('\t', " ")
}

/// `view`'s Key, or its id until it has one.
pub(crate) fn key(view: &IssueView) -> String {
    view.key.clone().unwrap_or_else(|| view.id.to_string())
}

/// The Issues `filter` matches, in its order, each with its version.
pub(crate) fn issues(
    store: &Store,
    filter: &str,
    context: Context,
) -> Result<Vec<Read>, StoreError> {
    store.read(|store| {
        let ids = store.find(filter, context)?;
        let mut read = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some((view, version)) = store.issue_with_version(id)? {
                read.push(Read {
                    id,
                    key: key(&view),
                    version,
                    view,
                });
            }
        }
        Ok(read)
    })
}
