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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snippet {
    #[serde(default)]
    views: Vec<View>,
}

/// The `[[views]]` text a new View starts from in an editor.
pub const BLANK: &str = "[[views]]\nname = \"\"\nfilter = \"\"\n# layout = \"board\"\n";

impl View {
    /// Its `[[views]]` table, as a repo file writes it.
    ///
    /// # Panics
    ///
    /// Never: every field is a string or a table of strings.
    #[must_use]
    pub fn snippet(&self) -> String {
        let one = Snippet {
            views: vec![self.clone()],
        };
        toml::to_string(&one).expect("a View always serializes")
    }

    /// The one View `text` holds as a `[[views]]` table, checked as the
    /// repo file checks its own.
    ///
    /// # Errors
    ///
    /// Why `text` is not one named View.
    pub fn from_snippet(text: &str) -> Result<Self, String> {
        // On one line, as a status line shows only the first.
        let Snippet { views } = toml::from_str(text).map_err(|err: toml::de::Error| {
            let line = err
                .span()
                .map(|span| text[..span.start.min(text.len())].lines().count().max(1));
            match line {
                Some(line) => format!("{} (line {line})", err.message()),
                None => err.message().to_owned(),
            }
        })?;
        let [mut view] = <[Self; 1]>::try_from(views)
            .map_err(|views| format!("expected one [[views]] table, not {}", views.len()))?;
        if view.name.trim().is_empty() {
            return Err("a View needs a name".to_owned());
        }
        if let Some(key) = &mut view.project {
            *key = key.to_ascii_uppercase();
            if !tm_core::project::is_key(key) {
                return Err(format!(
                    "project {key:?} is not a Project key: a letter, then 1 to 9 letters or digits"
                ));
            }
        }
        Ok(view)
    }
    /// Whether it shows in the Project with `key`: one it names, or with
    /// none named, any. The repo file holds keys uppercased.
    #[must_use]
    pub fn shows_in(&self, key: &str) -> bool {
        self.project.as_deref().is_none_or(|project| project == key)
    }
}
