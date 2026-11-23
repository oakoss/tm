//! The Scheme: Issue types, their fields and their Workflows.
//!
//! The projection never reads the Scheme; values are stored by id and the
//! Scheme is applied when reading, so editing it never needs a rebuild.

use std::collections::BTreeMap;

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

    /// Done or cancelled: no work is left on an Issue in it.
    #[must_use]
    pub const fn is_closed(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}

/// The person an Issue waits on to answer its question; `tm ask` sets it and
/// `tm answer` clears it.
pub const WAITING_ON: &str = "waiting_on";

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
    Version,
    Url,
    FilePaths,
    /// The id of a Short-list in the Issue's Project (ADR 0016).
    Shortlist,
    /// An ordering key from [`crate::rank`].
    Rank,
}

impl FieldType {
    /// Holds a set of elements changed by `add` and `remove`, not a value.
    #[must_use]
    pub const fn is_multi(self) -> bool {
        matches!(self, Self::Labels | Self::MultiSelect | Self::FilePaths)
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

    /// Built in, or the question `tm ask` sets: every type carries it, and
    /// no Scheme file may remove or replace it.
    #[must_use]
    pub fn is_fixed(&self) -> bool {
        self.builtin || self.name == WAITING_ON
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
    /// The fields an Issue must hold to enter it and while it stays, binding
    /// only types that carry them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
}

/// Which Status changes a Workflow allows. JSON writes `"any"`, or the list
/// of `{ "from", "to" }` a Scheme file gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transitions {
    Any,
    /// The Statuses each listed Status may move to; one not listed moves to
    /// any.
    Listed(BTreeMap<String, Vec<String>>),
}

impl Serialize for Transitions {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Move<'a> {
            from: &'a str,
            to: &'a [String],
        }
        match self {
            Self::Any => serializer.serialize_str("any"),
            Self::Listed(moves) => {
                serializer.collect_seq(moves.iter().map(|(from, to)| Move { from, to }))
            }
        }
    }
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

    /// The Status `name` means: its id, else its label, without case. An id
    /// wins, since a configured label may spell another Status's id.
    #[must_use]
    pub fn named(&self, name: &str) -> Option<&Status> {
        let by = |key: fn(&Status) -> &str| {
            self.statuses
                .iter()
                .find(|status| key(status).eq_ignore_ascii_case(name))
        };
        by(|status| &status.id).or_else(|| by(|status| &status.label))
    }

    /// The Statuses of `category`, in order.
    pub fn of_category(&self, category: Category) -> impl Iterator<Item = &Status> {
        self.statuses
            .iter()
            .filter(move |status| status.category == category)
    }

    /// Whether moving from `from` (none for a new Issue) to `to` is allowed.
    /// An Issue in a Status this Workflow does not define may move to any, so
    /// it can be repaired.
    #[must_use]
    pub fn allows(&self, from: Option<&str>, to: &str) -> bool {
        if self.status(to).is_none() {
            return false;
        }
        let Some(from) = from.filter(|from| self.status(from).is_some()) else {
            return true;
        };
        match &self.transitions {
            Transitions::Any => true,
            Transitions::Listed(moves) => moves
                .get(from)
                .is_none_or(|allowed| allowed.iter().any(|id| id == to)),
        }
    }

    /// The Statuses an Issue in `from` may move to, its own left out.
    #[must_use]
    pub fn moves_from(&self, from: Option<&str>) -> Vec<&Status> {
        self.statuses
            .iter()
            .filter(|status| Some(status.id.as_str()) != from && self.allows(from, &status.id))
            .collect()
    }

    /// The first Status of `category` an Issue in `from` may move to.
    ///
    /// # Errors
    ///
    /// [`MoveGap`] when there is none.
    pub fn first_move(
        &self,
        from: Option<&str>,
        category: Category,
    ) -> Result<&Status, MoveGap<'_>> {
        let mut statuses = self.of_category(category).peekable();
        if statuses.peek().is_none() {
            return Err(MoveGap::NoStatus);
        }
        statuses
            .find(|status| self.allows(from, &status.id))
            .ok_or_else(|| MoveGap::Unreachable(self.moves_from(from)))
    }
}

/// Why an Issue has no Status of a category to move to.
#[derive(Debug, PartialEq, Eq)]
pub enum MoveGap<'a> {
    NoStatus,
    /// None is allowed from where it is; holds the moves that are.
    Unreachable(Vec<&'a Status>),
}

/// Which types an Issue's parent may have; an empty list takes no parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parents {
    Any,
    Listed(Vec<String>),
}

/// What a type's listed parent types allow.
#[must_use]
pub fn parent_rule(types: &[String]) -> String {
    if types.is_empty() {
        "takes no parent".to_owned()
    } else {
        format!("takes a parent of type {}", types.join(" or "))
    }
}

/// `"any"`, or the list of type ids.
impl Serialize for Parents {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Any => serializer.serialize_str("any"),
            Self::Listed(types) => types.serialize(serializer),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssueType {
    pub id: String,
    pub label: String,
    pub workflow: String,
    pub parents: Parents,
    /// The Scheme fields it carries besides the fixed ones; `None` carries
    /// every field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
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

/// A Semantic Versioning 2.0.0 version: `MAJOR.MINOR.PATCH`, then an
/// optional `-` prerelease and `+` build, each dot-separated identifiers of
/// ASCII letters, digits and hyphens. Numbers outside the build carry no leading zero.
#[must_use]
pub fn is_version(text: &str) -> bool {
    let (rest, build) = match text.split_once('+') {
        Some((rest, build)) => (rest, Some(build)),
        None => (text, None),
    };
    let (core, prerelease) = match rest.split_once('-') {
        Some((core, prerelease)) => (core, Some(prerelease)),
        None => (rest, None),
    };
    let number = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|b| b.is_ascii_digit())
            && (part == "0" || !part.starts_with('0'))
    };
    let identifiers = |text: &str, numbered: bool| {
        text.split('.').all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && (!numbered || !part.bytes().all(|b| b.is_ascii_digit()) || number(part))
        })
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| number(part))
        && prerelease.is_none_or(|text| identifiers(text, true))
        && build.is_none_or(|text| identifiers(text, false))
}

/// An absolute `http` or `https` URL with a host, a port only of digits, and
/// no whitespace or control characters.
#[must_use]
pub fn is_url(text: &str) -> bool {
    let lower = text.get(..8).unwrap_or(text).to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &text[8..]
    } else if lower.starts_with("http://") {
        &text[7..]
    } else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let (host, port) = match host_port.strip_prefix('[') {
        Some(bracketed) => match bracketed.split_once(']') {
            Some((host, "")) => (host, None),
            Some((host, after)) => match after.strip_prefix(':') {
                Some(port) => (host, Some(port)),
                None => return false,
            },
            None => return false,
        },
        None => match host_port.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        },
    };
    let port_ok = port
        .is_none_or(|port| port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok());
    !host.is_empty()
        && !host.contains(['[', ']'])
        && port_ok
        && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// A path relative to the repo root: `/`-separated parts, none empty, `.`,
/// `..` or padded with whitespace, no drive such as `C:`, and no backslash
/// or control character, so it reads the same on every machine.
#[must_use]
pub fn is_repo_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    !text.is_empty()
        && !drive
        && !text.chars().any(|c| c == '\\' || c.is_control())
        && text
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && part.trim() == part)
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
            // A Scheme field, not a column, so export carries it in `fields`.
            Field::custom(WAITING_ON, "Waiting on", T::Person, Vec::new()),
        ];
        let status = |id: &str, label: &str, category| Status {
            id: id.into(),
            label: label.into(),
            category,
            requires: Vec::new(),
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
            fields: None,
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
                | FieldType::Version
                | FieldType::Url
                | FieldType::FilePaths
                | FieldType::IssueReference
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
            "is",
            "blocked_by",
            "blocking",
            "session",
            "text",
            "project",
            crate::link::FIELD,
            crate::op::COMMENTS_FIELD,
            crate::op::COMMENT_BODY_FIELD,
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

    /// Every Issue type's id, in the Scheme's order.
    #[must_use]
    pub fn type_ids(&self) -> Vec<String> {
        self.types
            .iter()
            .map(|issue_type| issue_type.id.clone())
            .collect()
    }

    #[must_use]
    pub fn issue_type(&self, id: &str) -> Option<&IssueType> {
        self.types.iter().find(|issue_type| issue_type.id == id)
    }

    /// Whether an Issue of `issue_type` carries `field`: a fixed field always,
    /// any field for a type with no list or one the Scheme lacks.
    #[must_use]
    pub fn carries(&self, issue_type: &str, field: &Field) -> bool {
        field.is_fixed()
            || self
                .issue_type(issue_type)
                .and_then(|issue_type| issue_type.fields.as_ref())
                .is_none_or(|carried| carried.contains(&field.name))
    }

    /// The parent types an Issue of `issue_type` is limited to; `None` when
    /// any type may be its parent, as for a type the Scheme lacks.
    #[must_use]
    pub fn parent_types(&self, issue_type: &str) -> Option<&[String]> {
        match &self.issue_type(issue_type)?.parents {
            Parents::Any => None,
            Parents::Listed(types) => Some(types),
        }
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

    /// The fields an Issue of `issue_type` must hold in `status`: those the
    /// Status requires that the type carries.
    #[must_use]
    pub fn required_in(&self, issue_type: &str, status: &str) -> Vec<&Field> {
        let Some(status) = self
            .workflow(issue_type)
            .and_then(|workflow| workflow.status(status))
        else {
            return Vec::new();
        };
        status
            .requires
            .iter()
            .filter_map(|name| self.field(name))
            .filter(|field| self.carries(issue_type, field))
            .collect()
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
    fn a_version_is_semver_and_nothing_looser() {
        for good in [
            "0.0.0",
            "1.2.3",
            "10.20.30",
            "1.0.0-rc.1",
            "1.0.0-0.3.7",
            "1.0.0+20130313",
            "1.0.0-alpha+001",
            "1.0.0-x-y",
        ] {
            assert!(is_version(good), "{good}");
        }
        for bad in [
            "",
            "1",
            "1.2",
            "v1.2.3",
            "01.2.3",
            "1.02.3",
            "1.2.3.4",
            "1.2.3-",
            "1.2.3-01",
            "1.2.3+",
            "1.2.3-a..b",
            "1.2.3 ",
            "1.2.x",
        ] {
            assert!(!is_version(bad), "{bad}");
        }
    }

    #[test]
    fn a_url_is_http_or_https_with_a_host() {
        for good in [
            "https://example.com",
            "http://example.com/a?b#c",
            "HTTPS://x.io:8080/p",
            "https://user@host/x",
            "http://[::1]:8080/x",
            "https://host:65535",
            "https://host:0",
        ] {
            assert!(is_url(good), "{good}");
        }
        for bad in [
            "",
            "example.com",
            "ftp://example.com",
            "https://",
            "https:///path",
            "https://host/a b",
            "https://:80",
            "mailto:a@b.c",
            "https://host:garbage",
            "https://host:",
            "https://host:65536",
            "https://host:+80",
            "https://[invalid",
            "https://[::1]x",
            "http://[::1]8080/x",
        ] {
            assert!(!is_url(bad), "{bad}");
        }
    }

    #[test]
    fn a_repo_path_is_relative_slash_separated_and_climbs_nowhere() {
        for good in [
            "src/main.rs",
            "README.md",
            ".github/workflows/ci.yml",
            "a/b c/d.txt",
            "notes:/x.md",
            "src/a:b.rs",
        ] {
            assert!(is_repo_path(good), "{good}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "src/",
            "src//main.rs",
            "./src",
            "src/../x",
            "..",
            "src\\main.rs",
            "a\tb",
            "C:/Windows/system.ini",
            "c:",
            "C:outside.txt",
            "C:dir/file.rs",
            " lead.rs",
            "src/trail.rs ",
        ] {
            assert!(!is_repo_path(bad), "{bad}");
        }
    }

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
    fn a_workflow_names_finds_and_moves_its_statuses() {
        let scheme = Scheme::built_in();
        assert_eq!(scheme.type_ids()[..2], ["epic", "story"]);
        let workflow = scheme.workflow("task").unwrap();
        assert_eq!(workflow.named("IN PROGRESS").unwrap().id, "in_progress");
        assert_eq!(workflow.named("In_Review").unwrap().id, "in_review");
        assert!(workflow.named("active").is_none());
        let mut shadowed = workflow.clone();
        shadowed.statuses[1].label = "Done".into();
        assert_eq!(
            shadowed.named("done").unwrap().id,
            "done",
            "an id wins over another Status's label"
        );
        let active: Vec<&str> = workflow
            .of_category(Category::Active)
            .map(|status| status.id.as_str())
            .collect();
        assert_eq!(active, ["in_progress", "in_review"]);
        let ids = |moves: Vec<&Status>| -> Vec<String> {
            moves.into_iter().map(|status| status.id.clone()).collect()
        };
        assert_eq!(
            ids(workflow.moves_from(Some("todo"))),
            ["triage", "in_progress", "in_review", "done", "cancelled"]
        );
        assert_eq!(ids(workflow.moves_from(None)).len(), 6);
        assert_eq!(
            ids(workflow.moves_from(Some("gone"))).len(),
            6,
            "a Status the Workflow lacks may move to any, to repair it"
        );
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
        for reserved in [
            "status",
            "sort",
            "category",
            "is",
            "blocked_by",
            "blocking",
            "session",
            "text",
            "project",
            "links",
            "comments",
        ] {
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

    #[test]
    fn listed_transitions_restrict_only_the_statuses_they_list() {
        let mut workflow = Scheme::built_in().workflows[0].clone();
        workflow.transitions = Transitions::Listed(BTreeMap::from([
            ("todo".to_owned(), vec!["in_progress".to_owned()]),
            ("done".to_owned(), Vec::new()),
        ]));
        assert!(workflow.allows(Some("todo"), "in_progress"));
        assert!(!workflow.allows(Some("todo"), "done"));
        assert!(!workflow.allows(Some("done"), "todo"));
        assert!(workflow.allows(Some("triage"), "done"));
        assert!(workflow.allows(Some("gone"), "done"));
        assert!(workflow.allows(None, "done"));
        assert!(!workflow.allows(Some("triage"), "shipped"));
        assert_eq!(workflow.moves_from(Some("done")), Vec::<&Status>::new());
        let first = |from, category| {
            workflow
                .first_move(Some(from), category)
                .map(|s| s.id.as_str())
                .map_err(|gap| match gap {
                    MoveGap::NoStatus => Vec::new(),
                    MoveGap::Unreachable(moves) => {
                        moves.into_iter().map(|s| s.id.as_str()).collect()
                    }
                })
        };
        assert_eq!(first("todo", Category::Active), Ok("in_progress"));
        assert_eq!(first("todo", Category::Done), Err(vec!["in_progress"]));
        assert_eq!(first("in_review", Category::Done), Ok("done"));
        let mut bare = workflow.clone();
        bare.statuses
            .retain(|status| status.category != Category::Done);
        assert_eq!(
            bare.first_move(Some("todo"), Category::Done),
            Err(MoveGap::NoStatus)
        );
    }
}
