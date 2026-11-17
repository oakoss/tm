//! Violations: the ways an Issue breaks its Scheme. A write may not add one,
//! but a merge or a Scheme edit can leave one, so they are judged from an
//! Issue's state rather than from the write that reached it.

use crate::scheme::{Field, Scheme};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// A field the Scheme requires holds no value.
    MissingField(String),
    /// The Issue holds a value for a field its type does not carry.
    UncarriedField(String),
    /// The Status is not one of its type's Workflow; an unknown type has none.
    StatusOutsideWorkflow,
}

/// The Violations of an Issue of `issue_type` in `status` under `scheme`:
/// missing fields, then uncarried ones, each in the Scheme's order; `holds`
/// says whether the Issue holds a value for a field. An unset type or Status
/// fits any Workflow.
#[must_use]
pub fn violations(
    scheme: &Scheme,
    (issue_type, status): (Option<&str>, Option<&str>),
    holds: impl Fn(&Field) -> bool,
) -> Vec<Violation> {
    let mut found: Vec<Violation> = scheme
        .fields
        .iter()
        .filter(|field| field.required && !holds(field))
        .map(|field| Violation::MissingField(field.name.clone()))
        .collect();
    if let Some(issue_type) = issue_type {
        found.extend(
            scheme
                .fields
                .iter()
                .filter(|field| !scheme.carries(issue_type, field) && holds(field))
                .map(|field| Violation::UncarriedField(field.name.clone())),
        );
    }
    if let (Some(issue_type), Some(status)) = (issue_type, status)
        && scheme.category(issue_type, status).is_none()
    {
        found.push(Violation::StatusOutsideWorkflow);
    }
    found
}

/// The Violations `after` has that `before` did not: what a write may not add.
#[must_use]
pub fn added(before: &[Violation], after: Vec<Violation>) -> Vec<Violation> {
    after
        .into_iter()
        .filter(|violation| !before.contains(violation))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheme::FieldType;

    fn missing(name: &str) -> Violation {
        Violation::MissingField(name.into())
    }

    #[test]
    fn an_issue_violates_its_required_fields_and_its_workflow() {
        let scheme = Scheme::built_in();
        let all = |_: &Field| true;
        assert_eq!(violations(&scheme, (Some("task"), Some("todo")), all), []);
        assert_eq!(violations(&scheme, (None, None), all), []);
        assert_eq!(
            violations(&scheme, (Some("task"), Some("gone")), all),
            [Violation::StatusOutsideWorkflow]
        );
        assert_eq!(
            violations(&scheme, (Some("gone"), Some("todo")), all),
            [Violation::StatusOutsideWorkflow]
        );
        let untitled = |field: &Field| field.name != "title" && field.name != "type";
        assert_eq!(
            violations(&scheme, (Some("task"), Some("gone")), untitled),
            [
                missing("type"),
                missing("title"),
                Violation::StatusOutsideWorkflow
            ]
        );
    }

    #[test]
    fn an_issue_violates_a_field_its_type_does_not_carry_only_while_holding_it() {
        let mut scheme = Scheme::built_in();
        for name in ["severity", "area"] {
            scheme
                .add_field(Field::custom(name, name, FieldType::Text, Vec::new()))
                .unwrap();
        }
        scheme.types[3].fields = Some(vec!["severity".into()]);
        let held = |field: &Field| field.name != "description";
        assert_eq!(
            violations(&scheme, (Some("bug"), Some("todo")), held),
            [Violation::UncarriedField("area".into())]
        );
        assert_eq!(violations(&scheme, (Some("task"), Some("todo")), held), []);
        let without_area = |field: &Field| field.name != "area";
        assert_eq!(
            violations(&scheme, (Some("bug"), Some("todo")), without_area),
            []
        );
        assert!(scheme.carries("bug", scheme.field("waiting_on").unwrap()));
        assert!(scheme.carries("bug", scheme.field("priority").unwrap()));
        assert!(scheme.carries("gone", scheme.field("area").unwrap()));
    }

    #[test]
    fn a_write_is_held_only_to_the_violations_it_adds() {
        let before = [missing("title"), Violation::StatusOutsideWorkflow];
        let after = vec![missing("title"), missing("status")];
        assert_eq!(added(&before, after), [missing("status")]);
    }
}
