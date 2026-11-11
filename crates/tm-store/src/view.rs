//! Views: saved, named filters and layouts, kept in the repo file so they
//! are shared with the repo (design.md, Views and UIs).

use serde::{Deserialize, Serialize};

/// A View as the repo file holds it. Its filter, columns and swimlanes are
/// checked when it is opened, so a bad View never stops another command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filter: String,
    #[serde(default, skip_serializing_if = "Layout::is_list")]
    pub layout: Layout,
    /// The Project it shows in; with none, the Project the directory maps
    /// to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// A Board's columns, in order; with none, the Scheme's Statuses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<Column>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swimlanes: Option<Swimlanes>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    #[default]
    List,
    Board,
}

impl Layout {
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn is_list(&self) -> bool {
        *self == Self::List
    }
}

/// A Board column: one Status, narrowed by its own filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filter: String,
}

/// A Board's horizontal bands, one per value of `field`, narrowed by their
/// own filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Swimlanes {
    pub field: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filter: String,
}

impl View {
    /// Whether it shows in the Project with `key`: one it names, or with
    /// none named, any. The repo file holds keys uppercased.
    #[must_use]
    pub fn shows_in(&self, key: &str) -> bool {
        self.project.as_deref().is_none_or(|project| project == key)
    }
}
