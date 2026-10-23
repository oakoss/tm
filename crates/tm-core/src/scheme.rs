//! The Scheme: Issue types, their fields and their Workflows.
//!
//! The projection never reads the Scheme; values are stored by id and the
//! Scheme is applied when reading, so editing it never needs a rebuild.

use serde::Serialize;

/// Every Status belongs to one category; cross-type features read categories,
/// never Status names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Triage,
    Todo,
    Active,
    Done,
    Cancelled,
}

impl Category {
    pub const ALL: [Self; 5] = [
        Self::Triage,
        Self::Todo,
        Self::Active,
        Self::Done,
        Self::Cancelled,
    ];

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Triage => "triage",
            Self::Todo => "todo",
            Self::Active => "active",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

/// The five priority levels, highest rank first. Levels are fixed in code so
/// the projection can store a sortable rank without reading the Scheme; only
/// their labels are configurable.
pub const PRIORITY_RANKS: [(&str, i64); 5] = [
    ("urgent", 4),
    ("high", 3),
    ("medium", 2),
    ("low", 1),
    ("none", 0),
];

/// The rank of a priority id, if it is one of the five levels.
#[must_use]
pub fn priority_rank(id: &str) -> Option<i64> {
    PRIORITY_RANKS
        .iter()
        .find(|(level, _)| *level == id)
        .map(|(_, rank)| *rank)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Key,
    IssueType,
    Text,
    RichText,
    Status,
    Priority,
    Person,
    IssueReference,
    Labels,
    Boolean,
    Datetime,
    Number,
    Date,
    SingleSelect,
    MultiSelect,
    /// The id of a Short-list in the Issue's Project (ADR 0016).
    Shortlist,
    /// An ordering key from [`crate::rank`].
    Rank,
}

impl FieldType {
    /// Holds a set of elements changed by `add` and `remove`, not a value.
    #[must_use]
    pub const fn is_multi(self) -> bool {
        matches!(self, Self::Labels | Self::MultiSelect)
    }
}

/// An enumerated value: a permanent id and a display label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    pub id: String,
    pub label: String,
}

impl Choice {
    fn new(id: &str, label: &str) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Field {
    pub name: String,
    pub label: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    pub builtin: bool,
    /// Computed from operations or other records, never set directly.
    pub derived: bool,
    pub required: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<Choice>,
}

impl Field {
    fn builtin(name: &str, label: &str, field_type: FieldType) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            field_type,
            builtin: true,
            derived: false,
            required: false,
            options: Vec::new(),
        }
    }

    const fn derived(mut self) -> Self {
        self.derived = true;
        self
    }

    const fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// A Scheme field; its name must not be a built-in one.
    #[must_use]
    pub fn custom(name: &str, label: &str, field_type: FieldType, options: Vec<Choice>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            field_type,
            builtin: false,
            derived: false,
            required: false,
            options,
        }
    }

    /// The option with this id, for enumerated fields.
    #[must_use]
    pub fn option(&self, id: &str) -> Option<&Choice> {
        self.options.iter().find(|choice| choice.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    pub id: String,
    pub label: String,
    pub category: Category,
}

/// Which Status changes a Workflow allows. Only `Any` exists until Schemes
/// become configurable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transitions {
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Workflow {
    pub name: String,
    /// The Status a new Issue enters.
    pub start: String,
    pub statuses: Vec<Status>,
    pub transitions: Transitions,
}

impl Workflow {
    #[must_use]
    pub fn status(&self, id: &str) -> Option<&Status> {
        self.statuses.iter().find(|status| status.id == id)
    }

    /// Whether moving from `from` (none for a new Issue) to `to` is allowed.
    #[must_use]
    pub fn allows(&self, _from: Option<&str>, to: &str) -> bool {
        match self.transitions {
            Transitions::Any => self.status(to).is_some(),
        }
    }
}

/// Which types an Issue's parent may have. Only `Any` exists until Schemes
/// become configurable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Parents {
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssueType {
    pub id: String,
    pub label: String,
    pub workflow: String,
    pub parents: Parents,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SchemeError {
    #[error("{0:?} is reserved for a built-in field or filter qualifier")]
    ReservedField(String),
    #[error("field {0:?} already exists")]
    DuplicateField(String),
    #[error("{0:?} must be lowercase letters, digits and underscores, starting with a letter")]
    InvalidId(String),
    #[error("option {0:?} appears twice")]
    DuplicateOption(String),
    #[error("{0:?} is only for built-in fields")]
    BuiltInType(FieldType),
}

/// A real calendar date written `YYYY-MM-DD` with a four-digit year, the one
/// spelling date fields store, so comparing the text compares the dates.
#[must_use]
pub fn is_date(text: &str) -> bool {
    // Signed years are longer than ten characters, and sort out of order.
    text.len() == 10
        && text
            .parse::<jiff::civil::Date>()
            .is_ok_and(|date| date.to_string() == text)
}

/// The shape of every id: lowercase letters, digits and underscores, starting
/// with a letter, as operation field names already are.
#[must_use]
pub fn is_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Scheme {
    pub name: String,
    pub priorities: Vec<Choice>,
    pub fields: Vec<Field>,
    pub workflows: Vec<Workflow>,
    pub types: Vec<IssueType>,
}

impl Scheme {
    /// The Scheme every Project uses until Schemes become configurable.
    #[must_use]
    pub fn built_in() -> Self {
        use FieldType as T;
        let fields = vec![
            Field::builtin("key", "Key", T::Key).derived(),
            Field::builtin("type", "Type", T::IssueType).required(),
            Field::builtin("title", "Title", T::Text).required(),
            Field::builtin("description", "Description", T::RichText),
            Field::builtin("status", "Status", T::Status).required(),
            Field::builtin("priority", "Priority", T::Priority),
            Field::builtin("assignee", "Assignee", T::Person),
            Field::builtin("reporter", "Reporter", T::Person).derived(),
            Field::builtin("agent", "Agent", T::Person).derived(),
            Field::builtin("claim", "Claim", T::Text).derived(),
            Field::builtin("parent", "Parent", T::IssueReference),
            Field::builtin("labels", "Labels", T::Labels),
            Field::builtin("archived", "Archived", T::Boolean),
            Field::builtin("shortlist", "Short-list", T::Shortlist),
            Field::builtin("shortlist_rank", "Short-list rank", T::Rank),
            Field::builtin("created", "Created", T::Datetime).derived(),
            Field::builtin("updated", "Updated", T::Datetime).derived(),
        ];
        let status = |id: &str, label: &str, category| Status {
            id: id.into(),
            label: label.into(),
            category,
        };
        let workflow = Workflow {
            name: "default".into(),
            start: "todo".into(),
            statuses: vec![
                status("triage", "Triage", Category::Triage),
                status("todo", "Todo", Category::Todo),
                status("in_progress", "In Progress", Category::Active),
                status("in_review", "In Review", Category::Active),
                status("done", "Done", Category::Done),
                status("cancelled", "Cancelled", Category::Cancelled),
            ],
            transitions: Transitions::Any,
        };
        let types = [
            ("epic", "Epic"),
            ("story", "Story"),
            ("task", "Task"),
            ("bug", "Bug"),
            ("sub_task", "Sub-task"),
            ("spike", "Spike"),
            ("design", "Design"),
        ]
        .into_iter()
        .map(|(id, label)| IssueType {
            id: id.into(),
            label: label.into(),
            workflow: "default".into(),
            parents: Parents::Any,
        })
        .collect();
        let priorities = [
            ("urgent", "Urgent"),
            ("high", "High"),
            ("medium", "Medium"),
            ("low", "Low"),
            ("none", "None"),
        ]
        .into_iter()
        .map(|(id, label)| Choice::new(id, label))
        .collect();
        Self {
            name: "default".into(),
            priorities,
            fields,
            workflows: vec![workflow],
            types,
        }
    }

    /// Adds a Scheme field.
    ///
    /// # Errors
    ///
    /// [`SchemeError`] when the name is built-in or already taken.
    pub fn add_field(&mut self, field: Field) -> Result<(), SchemeError> {
        // Lowercase ids keep names and options unambiguous where filters
        // match them without case.
        for id in std::iter::once(&field.name).chain(field.options.iter().map(|o| &o.id)) {
            if !is_id(id) {
                return Err(SchemeError::InvalidId(id.clone()));
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        if let Some(duplicate) = field.options.iter().find(|o| !seen.insert(&o.id)) {
            return Err(SchemeError::DuplicateOption(duplicate.id.clone()));
        }
        if !matches!(
            field.field_type,
            FieldType::Text
                | FieldType::RichText
                | FieldType::Number
                | FieldType::Date
                | FieldType::Datetime
                | FieldType::Boolean
                | FieldType::SingleSelect
                | FieldType::MultiSelect
                | FieldType::Person
        ) {
            return Err(SchemeError::BuiltInType(field.field_type));
        }
        // A field named for a filter qualifier could never be filtered, and
        // `links` belongs to Link operations.
        if [
            "sort",
            "no",
            "has",
            "category",
            crate::link::FIELD,
            crate::op::COMMENTS_FIELD,
        ]
        .contains(&field.name.as_str())
        {
            return Err(SchemeError::ReservedField(field.name));
        }
        if let Some(existing) = self.field(&field.name) {
            return Err(if existing.builtin {
                SchemeError::ReservedField(field.name)
            } else {
                SchemeError::DuplicateField(field.name)
            });
        }
        self.fields.push(field);
        Ok(())
    }

    #[must_use]
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.name == name)
    }

    #[must_use]
    pub fn issue_type(&self, id: &str) -> Option<&IssueType> {
        self.types.iter().find(|issue_type| issue_type.id == id)
    }

    #[must_use]
    pub fn priority(&self, id: &str) -> Option<&Choice> {
        self.priorities.iter().find(|level| level.id == id)
    }

    /// The Workflow Issues of this type follow.
    #[must_use]
    pub fn workflow(&self, type_id: &str) -> Option<&Workflow> {
        let issue_type = self.issue_type(type_id)?;
        self.workflows
            .iter()
            .find(|workflow| workflow.name == issue_type.workflow)
    }

    /// The category of a type's Status; `None` when the Scheme no longer
    /// defines that type or Status.
    #[must_use]
    pub fn category(&self, type_id: &str, status: &str) -> Option<Category> {
        Some(self.workflow(type_id)?.status(status)?.category)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_type_has_a_workflow_with_its_start_status() {
        let scheme = Scheme::built_in();
        assert_eq!(scheme.types.len(), 7);
        for issue_type in &scheme.types {
            let workflow = scheme.workflow(&issue_type.id).unwrap();
            assert!(workflow.status(&workflow.start).is_some());
        }
    }

    #[test]
    fn every_category_has_a_default_status() {
        let scheme = Scheme::built_in();
        for category in Category::ALL {
            assert!(
                scheme.workflows[0]
                    .statuses
                    .iter()
                    .any(|status| status.category == category),
                "{category:?}"
            );
        }
    }

    #[test]
    fn categories_come_from_the_scheme() {
        let scheme = Scheme::built_in();
        assert_eq!(scheme.category("bug", "in_review"), Some(Category::Active));
        assert_eq!(
            scheme.category("bug", "cancelled"),
            Some(Category::Cancelled)
        );
        assert_eq!(scheme.category("bug", "gone"), None);
        assert_eq!(scheme.category("gone", "todo"), None);
    }

    #[test]
    fn priority_ranks_cover_the_scheme_levels_in_order() {
        let scheme = Scheme::built_in();
        let ranks: Vec<i64> = scheme
            .priorities
            .iter()
            .map(|level| priority_rank(&level.id).unwrap())
            .collect();
        assert_eq!(ranks, [4, 3, 2, 1, 0]);
        assert_eq!(priority_rank("highest"), None);
    }

    #[test]
    fn a_scheme_field_cannot_take_a_built_in_or_existing_name() {
        let mut scheme = Scheme::built_in();
        let area = || Field::custom("area", "Area", FieldType::Number, Vec::new());
        assert!(scheme.add_field(area()).is_ok());
        assert_eq!(
            scheme.add_field(area()),
            Err(SchemeError::DuplicateField("area".into()))
        );
        for reserved in ["status", "sort", "category", "links", "comments"] {
            assert_eq!(
                scheme.add_field(Field::custom(reserved, "S", FieldType::Text, Vec::new())),
                Err(SchemeError::ReservedField(reserved.into()))
            );
        }
    }

    #[test]
    fn scheme_field_ids_are_lowercase_unique_and_of_a_scheme_type() {
        let mut scheme = Scheme::built_in();
        let choice = |id: &str| Choice::new(id, id);
        for bad in ["Status", "SORT", "story-points", "1st", ""] {
            assert_eq!(
                scheme.add_field(Field::custom(bad, "X", FieldType::Text, Vec::new())),
                Err(SchemeError::InvalidId(bad.into())),
                "{bad}"
            );
        }
        let gold = vec![choice("gold"), choice("Gold")];
        assert_eq!(
            scheme.add_field(Field::custom("tier", "T", FieldType::SingleSelect, gold)),
            Err(SchemeError::InvalidId("Gold".into()))
        );
        let twice = vec![choice("gold"), choice("gold")];
        assert_eq!(
            scheme.add_field(Field::custom("tier", "T", FieldType::SingleSelect, twice)),
            Err(SchemeError::DuplicateOption("gold".into()))
        );
        assert_eq!(
            scheme.add_field(Field::custom("topics", "T", FieldType::Labels, Vec::new())),
            Err(SchemeError::BuiltInType(FieldType::Labels))
        );
        assert!(
            scheme
                .add_field(Field::custom(
                    "story_points",
                    "SP",
                    FieldType::Number,
                    Vec::new()
                ))
                .is_ok()
        );
    }

    #[test]
    fn transitions_allow_any_status_the_workflow_defines() {
        let scheme = Scheme::built_in();
        let workflow = scheme.workflow("task").unwrap();
        assert!(workflow.allows(None, "todo"));
        assert!(workflow.allows(Some("done"), "triage"));
        assert!(!workflow.allows(Some("todo"), "shipped"));
    }
}
