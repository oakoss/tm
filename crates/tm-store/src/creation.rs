//! A new Issue as a create writes it: the one place that decides what a
//! create sets beyond what was asked, such as the starting Status.

use serde_json::{Value, json};
use tm_core::scheme::Category;
use tm_core::{Actor, Change, Op, ProjectId, Scheme};
use uuid::Uuid;

use crate::family::{self, ParentStart};
use crate::{Draft, Store, StoreError, WriteOptions, Written, set};

/// The Issue a create made, and its parents the create started.
#[derive(Debug)]
pub struct Created {
    pub issue: Uuid,
    pub starts: Vec<ParentStart>,
}

impl Store {
    /// Writes `new` into `project` as one batch. A new Issue whose Status is
    /// active starts its parent and their ancestors, as
    /// [`crate::Lineage::start_parents`] does, read inside the same write.
    ///
    /// # Errors
    ///
    /// As [`Store::append_all`].
    pub fn create(
        &mut self,
        actor: &Actor,
        project: ProjectId,
        new: &NewIssue,
    ) -> Result<(Written<Vec<Op>>, Created), StoreError> {
        let issue = Uuid::now_v7();
        let changes = new.changes(&self.scheme);
        let active = new.issue_type.as_deref().is_some_and(|issue_type| {
            status(&changes).and_then(|status| self.scheme.category(issue_type, status))
                == Some(Category::Active)
        });
        let drafts = changes
            .into_iter()
            .map(|change| Draft {
                actor: actor.clone(),
                project,
                subject: issue,
                change,
            })
            .collect();
        let (written, starts) = match new.parent {
            Some(parent) if active => {
                let scheme = self.scheme.clone();
                let options = WriteOptions::default();
                self.append_with_lineage(drafts, parent, &options, |lineage| {
                    family::start(&scheme, lineage.issues(), lineage, actor)
                })?
            }
            _ => (self.append_all(drafts)?, Vec::new()),
        };
        Ok((written, Created { issue, starts }))
    }
}

/// The Status `changes` leave, the last one set.
fn status(changes: &[Change]) -> Option<&str> {
    changes.iter().rev().find_map(|change| match change {
        Change::SetField { field, value, .. } if field == "status" => value.as_str(),
        _ => None,
    })
}

/// What a create asks for. Each field's value is already typed.
#[derive(Debug, Clone, Default)]
pub struct NewIssue {
    pub issue_type: Option<String>,
    pub title: Option<String>,
    pub parent: Option<Uuid>,
    pub fields: Vec<(String, Value)>,
    pub labels: Vec<String>,
    /// Elements of several-valued Scheme fields, each with its field.
    pub elements: Vec<(String, String)>,
}

impl NewIssue {
    /// The changes that create it: the create, its type and that type's
    /// starting Status when the Scheme knows the type, then what was asked.
    #[must_use]
    pub fn changes(&self, scheme: &Scheme) -> Vec<Change> {
        let mut changes = vec![Change::Create];
        if let Some(issue_type) = &self.issue_type {
            changes.push(set("type", json!(issue_type)));
            if let Some(workflow) = scheme.workflow(issue_type) {
                changes.push(set("status", json!(workflow.start)));
            }
        }
        if let Some(title) = &self.title {
            changes.push(set("title", json!(title)));
        }
        if let Some(parent) = self.parent {
            changes.push(set("parent", json!(parent.to_string())));
        }
        for (field, value) in &self.fields {
            changes.push(set(field, value.clone()));
        }
        let labels = self.labels.iter().map(|label| ("labels", label));
        let elements = self
            .elements
            .iter()
            .map(|(field, elem)| (field.as_str(), elem));
        let mut seen = std::collections::BTreeSet::new();
        for (field, elem) in labels.chain(elements) {
            // Labels compare without case, as the store keys them.
            let key = if field == "labels" {
                elem.trim().to_lowercase()
            } else {
                elem.clone()
            };
            if !seen.insert((field, key)) {
                continue;
            }
            changes.push(Change::Add {
                field: field.to_owned(),
                label: String::new(),
                elem: elem.clone(),
                elem_label: None,
            });
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(changes: &[Change]) -> Vec<String> {
        changes
            .iter()
            .map(|change| match change {
                Change::Create => "create".to_owned(),
                Change::SetField { field, value, .. } => format!("{field}={value}"),
                Change::Add { field, elem, .. } => format!("+{field}:{elem}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_new_issue_starts_in_its_types_status_before_what_was_asked() {
        let scheme = Scheme::built_in();
        let parent = Uuid::nil();
        let new = NewIssue {
            issue_type: Some("bug".into()),
            title: Some("t".into()),
            parent: Some(parent),
            fields: vec![("priority".into(), json!("high"))],
            labels: vec!["ui".into(), "UI ".into()],
            elements: vec![
                ("paths".into(), "src/a.rs".into()),
                ("paths".into(), "src/a.rs".into()),
            ],
        };
        assert_eq!(
            written(&new.changes(&scheme)),
            [
                "create",
                r#"type="bug""#,
                r#"status="todo""#,
                r#"title="t""#,
                &format!(r#"parent="{parent}""#),
                r#"priority="high""#,
                "+labels:ui",
                "+paths:src/a.rs",
            ]
        );
        let unknown = NewIssue {
            issue_type: Some("gone".into()),
            ..NewIssue::default()
        };
        assert_eq!(
            written(&unknown.changes(&scheme)),
            ["create", r#"type="gone""#]
        );
    }
}
