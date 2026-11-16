//! A new Issue as a create writes it: the one place that decides what a
//! create sets beyond what was asked, such as the starting Status.

use serde_json::{Value, json};
use tm_core::{Change, Scheme};
use uuid::Uuid;

use crate::set;

/// What a create asks for. Each field's value is already typed.
#[derive(Debug, Clone, Default)]
pub struct NewIssue {
    pub issue_type: Option<String>,
    pub title: Option<String>,
    pub parent: Option<Uuid>,
    pub fields: Vec<(String, Value)>,
    pub labels: Vec<String>,
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
        for label in &self.labels {
            changes.push(Change::Add {
                field: "labels".to_owned(),
                label: String::new(),
                elem: label.clone(),
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
                Change::Add { elem, .. } => format!("+{elem}"),
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
            labels: vec!["ui".into()],
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
                "+ui",
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
