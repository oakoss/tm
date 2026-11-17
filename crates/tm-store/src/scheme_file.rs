//! The Scheme read from config (ADR 0022): the built-in Scheme, merged with
//! the Workspace's `scheme.toml`, merged with a Project's tables in the repo's
//! `.config/tm/scheme.toml`. Each layer merges over the one below by id.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tm_core::scheme::{
    Category, Choice, Field, FieldType, IssueType, Parents, Scheme, SchemeError, Status,
    Transitions, Workflow, is_id,
};
use toml::Spanned;

/// The Workspace layer, in the Workspace directory.
pub const WORKSPACE_FILE: &str = "scheme.toml";

/// The repo layer, under the directory the repo file maps from.
pub const REPO_FILE: &str = ".config/tm/scheme.toml";

/// The field types a Scheme file may give a field.
const FIELD_TYPES: [(&str, FieldType); 9] = [
    ("text", FieldType::Text),
    ("rich_text", FieldType::RichText),
    ("number", FieldType::Number),
    ("date", FieldType::Date),
    ("datetime", FieldType::Datetime),
    ("boolean", FieldType::Boolean),
    ("single_select", FieldType::SingleSelect),
    ("multi_select", FieldType::MultiSelect),
    ("person", FieldType::Person),
];

/// A Scheme file this tm cannot use, at a line of it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}:{line}: {message}", path.display())]
pub struct SchemeFileError {
    pub path: PathBuf,
    pub line: usize,
    pub message: String,
}

/// One layer's tables. The repo file holds one per Project it maps.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Layer {
    #[serde(default)]
    fields: BTreeMap<Spanned<String>, FieldSpec>,
    #[serde(default)]
    remove: Remove,
    #[serde(default)]
    types: BTreeMap<Spanned<String>, TypeSpec>,
    #[serde(default)]
    workflows: BTreeMap<Spanned<String>, WorkflowSpec>,
    link_types: Option<Spanned<toml::Value>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Remove {
    #[serde(default)]
    fields: Vec<Spanned<String>>,
    #[serde(default)]
    types: Vec<Spanned<String>>,
    #[serde(default)]
    workflows: Vec<Spanned<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeSpec {
    label: Option<String>,
    workflow: Spanned<String>,
    fields: Option<Vec<Spanned<String>>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowSpec {
    start: Spanned<String>,
    statuses: Vec<StatusSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusSpec {
    id: Spanned<String>,
    label: Option<String>,
    category: Spanned<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldSpec {
    label: Option<String>,
    #[serde(rename = "type")]
    field_type: Spanned<String>,
    #[serde(default)]
    options: Vec<ChoiceSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChoiceSpec {
    id: String,
    label: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepoLayers {
    #[serde(default)]
    projects: BTreeMap<Spanned<String>, Layer>,
}

/// A Scheme file's path and text.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    pub path: &'a Path,
    pub text: &'a str,
}

impl Source<'_> {
    fn error(&self, at: usize, message: impl Into<String>) -> SchemeFileError {
        let at = at.min(self.text.len());
        SchemeFileError {
            path: self.path.to_path_buf(),
            line: self.text[..at].matches('\n').count() + 1,
            message: message.into(),
        }
    }

    fn parse<T: for<'de> Deserialize<'de>>(&self) -> Result<T, SchemeFileError> {
        toml::from_str(self.text).map_err(|err| {
            let at = err.span().map_or(0, |span| span.start);
            self.error(at, err.message().to_owned())
        })
    }
}

/// The Scheme for `project`: the built-in one merged with `workspace`, then
/// with `repo`'s tables for `project`. `mapped` is every Project key the repo
/// file maps; the repo file may name no other, and every Project's tables are
/// checked, whichever one resolves. Reads no files.
///
/// # Errors
///
/// [`SchemeFileError`] at the first fault, in the order the layers merge.
pub fn resolve(
    workspace: Option<Source<'_>>,
    repo: Option<Source<'_>>,
    mapped: &[&str],
    project: &str,
) -> Result<Scheme, SchemeFileError> {
    layered(workspace, repo, mapped, project).map(|layered| layered.scheme)
}

/// The layer a Scheme field comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    BuiltIn,
    Workspace,
    Repo,
}

/// A resolved Scheme, with the layer each of its fields comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layered {
    pub scheme: Scheme,
    /// The fields a file defines, by name.
    pub origins: BTreeMap<String, Origin>,
    /// The Issue types a file defines, by id.
    pub type_origins: BTreeMap<String, Origin>,
    /// The Workflows a file defines, by id.
    pub workflow_origins: BTreeMap<String, Origin>,
}

impl Layered {
    /// The built-in Scheme, which no file changes.
    #[must_use]
    pub fn built_in() -> Self {
        Self {
            scheme: Scheme::built_in(),
            origins: BTreeMap::new(),
            type_origins: BTreeMap::new(),
            workflow_origins: BTreeMap::new(),
        }
    }

    /// Where the field `name` comes from; built in when no file defines it.
    #[must_use]
    pub fn origin(&self, name: &str) -> Origin {
        self.origins.get(name).copied().unwrap_or(Origin::BuiltIn)
    }

    /// Where the Issue type `id` comes from.
    #[must_use]
    pub fn type_origin(&self, id: &str) -> Origin {
        self.type_origins
            .get(id)
            .copied()
            .unwrap_or(Origin::BuiltIn)
    }

    /// Where the Workflow `id` comes from.
    #[must_use]
    pub fn workflow_origin(&self, id: &str) -> Origin {
        self.workflow_origins
            .get(id)
            .copied()
            .unwrap_or(Origin::BuiltIn)
    }
}

/// [`resolve`], keeping the layer each field comes from.
///
/// # Errors
///
/// As [`resolve`].
pub fn layered(
    workspace: Option<Source<'_>>,
    repo: Option<Source<'_>>,
    mapped: &[&str],
    project: &str,
) -> Result<Layered, SchemeFileError> {
    let mut scheme = Layered::built_in();
    if let Some(source) = workspace {
        let layer: Layer = source.parse()?;
        merge(&mut scheme, &layer, source, Origin::Workspace)?;
    }
    let Some(source) = repo else {
        return Ok(scheme);
    };
    let layers: RepoLayers = source.parse()?;
    let mut seen: Vec<&str> = Vec::new();
    let mut ours = None;
    for (key, layer) in &layers.projects {
        let name = key.get_ref();
        if seen.iter().any(|seen| seen.eq_ignore_ascii_case(name)) {
            return Err(source.error(key.span().start, format!("Project {name} has tables twice")));
        }
        seen.push(name);
        if !mapped
            .iter()
            .any(|mapped| mapped.eq_ignore_ascii_case(name))
        {
            return Err(source.error(
                key.span().start,
                format!(
                    "Project {name} is not one the repo file maps; it maps {}",
                    mapped.join(", ")
                ),
            ));
        }
        let mut merged = scheme.clone();
        merge(&mut merged, layer, source, Origin::Repo)?;
        if name.eq_ignore_ascii_case(project) {
            ours = Some(merged);
        }
    }
    Ok(ours.unwrap_or(scheme))
}

/// [`resolve`] reading the Workspace layer from `workspace_dir` and the repo
/// layer from under `repo_dir`; a missing file is no layer.
///
/// # Errors
///
/// As [`resolve`], or a file that exists but cannot be read.
pub fn read(
    workspace_dir: &Path,
    repo_dir: Option<&Path>,
    mapped: &[&str],
    project: &str,
) -> Result<Scheme, SchemeFileError> {
    read_layered(workspace_dir, repo_dir, mapped, project).map(|layered| layered.scheme)
}

/// [`read`], keeping the layer each field comes from.
///
/// # Errors
///
/// As [`read`].
pub fn read_layered(
    workspace_dir: &Path,
    repo_dir: Option<&Path>,
    mapped: &[&str],
    project: &str,
) -> Result<Layered, SchemeFileError> {
    let workspace_path = workspace_dir.join(WORKSPACE_FILE);
    let repo_path = repo_dir.map(|dir| dir.join(REPO_FILE));
    let workspace_text = text(workspace_dir, &workspace_path)?;
    let repo_text = match (repo_dir, &repo_path) {
        (Some(dir), Some(path)) => text(dir, path)?,
        _ => None,
    };
    layered(
        workspace_text.as_deref().map(|text| Source {
            path: &workspace_path,
            text,
        }),
        repo_text
            .as_deref()
            .zip(repo_path.as_deref())
            .map(|(text, path)| Source { path, text }),
        mapped,
        project,
    )
}

/// The text of the Scheme file at `path` under `base`. It is absent when a
/// part of the path is missing or is a file where a directory would be; a
/// link on the way whose target is gone means a Scheme that cannot be read.
fn text(base: &Path, path: &Path) -> Result<Option<String>, SchemeFileError> {
    let refused = |err: std::io::Error| SchemeFileError {
        path: path.to_path_buf(),
        line: 1,
        message: format!("cannot be read: {err}"),
    };
    let err = match fs::read_to_string(path) {
        Ok(text) => return Ok(Some(text)),
        Err(err) if matches!(err.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => err,
        Err(err) => return Err(refused(err)),
    };
    let mut at = base.to_path_buf();
    for part in path.strip_prefix(base).unwrap_or(path).components() {
        at.push(part);
        let Ok(found) = fs::symlink_metadata(&at) else {
            return Ok(None);
        };
        if found.is_symlink() && fs::metadata(&at).is_err() {
            return Err(refused(err));
        }
        if !found.is_symlink() && !found.is_dir() && at != path {
            return Ok(None);
        }
    }
    Err(refused(err))
}

/// `layer` over `layered`: its removals, then its fields, each replacing an
/// inherited one of its id whole and coming from `origin`.
fn merge(
    layered: &mut Layered,
    layer: &Layer,
    source: Source<'_>,
    origin: Origin,
) -> Result<(), SchemeFileError> {
    if let Some(value) = &layer.link_types {
        return Err(source.error(value.span().start, "link_types is not read by this tm yet"));
    }
    let Layered {
        scheme,
        origins,
        type_origins,
        workflow_origins,
    } = layered;
    removed(source, "fields", &layer.remove.fields, |name| {
        scheme.field(name).map(|field| !field.is_fixed())
    })?;
    removed(source, "types", &layer.remove.types, |id| {
        scheme.issue_type(id).map(|_| true)
    })?;
    removed(source, "workflows", &layer.remove.workflows, |id| {
        scheme
            .workflows
            .iter()
            .any(|workflow| workflow.name == id)
            .then_some(true)
    })?;
    for name in &layer.remove.fields {
        scheme.fields.retain(|field| field.name != *name.get_ref());
        origins.remove(name.get_ref());
    }
    for id in &layer.remove.types {
        scheme
            .types
            .retain(|issue_type| issue_type.id != *id.get_ref());
        type_origins.remove(id.get_ref());
    }
    for id in &layer.remove.workflows {
        scheme
            .workflows
            .retain(|workflow| workflow.name != *id.get_ref());
        workflow_origins.remove(id.get_ref());
    }
    for (id, spec) in &layer.workflows {
        let workflow = workflow(source, id, spec)?;
        scheme.workflows.retain(|kept| kept.name != workflow.name);
        scheme.workflows.push(workflow);
        workflow_origins.insert(id.get_ref().clone(), origin);
    }
    for (id, spec) in &layer.types {
        let name = id.get_ref();
        if !is_id(name) {
            return Err(source.error(
                id.span().start,
                SchemeError::InvalidId(name.clone()).to_string(),
            ));
        }
        scheme.types.retain(|kept| kept.id != *name);
        scheme.types.push(IssueType {
            id: name.clone(),
            label: spec.label.clone().unwrap_or_else(|| name.clone()),
            workflow: spec.workflow.get_ref().clone(),
            parents: Parents::Any,
            fields: spec
                .fields
                .as_ref()
                .map(|fields| fields.iter().map(|field| field.get_ref().clone()).collect()),
        });
        type_origins.insert(name.clone(), origin);
    }
    every_type_has_its_workflow(scheme, layer, source)?;
    merge_fields(scheme, origins, layer, source, origin)?;
    every_type_has_its_fields(scheme, layer, source)
}

/// Refuses `layer` when a type it defines lists a field twice, a fixed one,
/// or one no layer defines, or when it removes a field a type still lists.
fn every_type_has_its_fields(
    scheme: &Scheme,
    layer: &Layer,
    source: Source<'_>,
) -> Result<(), SchemeFileError> {
    for (id, spec) in &layer.types {
        let listed = spec.fields.as_deref().unwrap_or_default();
        for (at, field) in listed.iter().enumerate() {
            let (type_id, name) = (id.get_ref(), field.get_ref());
            let message = if listed[..at].iter().any(|before| before.get_ref() == name) {
                format!("type {type_id} lists field {name} twice")
            } else {
                match scheme.field(name) {
                    None => format!(
                        "type {type_id} lists field {name}, which neither this layer nor one below defines"
                    ),
                    Some(def) if def.is_fixed() => {
                        format!("type {type_id} lists field {name}, which every type carries")
                    }
                    Some(_) => continue,
                }
            };
            return Err(source.error(field.span().start, message));
        }
    }
    for name in &layer.remove.fields {
        if scheme.field(name.get_ref()).is_some() {
            continue;
        }
        if let Some(user) = scheme.types.iter().find(|issue_type| {
            issue_type
                .fields
                .as_ref()
                .is_some_and(|fields| fields.contains(name.get_ref()))
        }) {
            return Err(source.error(
                name.span().start,
                format!(
                    "field {} is removed, but type {} lists it",
                    name.get_ref(),
                    user.id
                ),
            ));
        }
    }
    Ok(())
}

/// Refuses `layer` when it leaves a type naming a Workflow that does not
/// exist, or no type at all.
fn every_type_has_its_workflow(
    scheme: &Scheme,
    layer: &Layer,
    source: Source<'_>,
) -> Result<(), SchemeFileError> {
    for (id, spec) in &layer.types {
        let wanted = spec.workflow.get_ref();
        if !scheme
            .workflows
            .iter()
            .any(|workflow| workflow.name == *wanted)
        {
            return Err(source.error(
                spec.workflow.span().start,
                format!(
                    "type {} names workflow {wanted}, which no layer defines",
                    id.get_ref()
                ),
            ));
        }
    }
    for id in &layer.remove.workflows {
        if scheme
            .workflows
            .iter()
            .any(|workflow| workflow.name == *id.get_ref())
        {
            continue;
        }
        if let Some(user) = scheme
            .types
            .iter()
            .find(|issue_type| issue_type.workflow == *id.get_ref())
        {
            return Err(source.error(
                id.span().start,
                format!(
                    "workflow {} is removed, but type {} uses it",
                    id.get_ref(),
                    user.id
                ),
            ));
        }
    }
    if scheme.types.is_empty() {
        let at = layer.remove.types.first().map_or(0, |id| id.span().start);
        return Err(source.error(at, "no Issue type is left; a Scheme needs one"));
    }
    Ok(())
}

/// `layer`'s fields over `scheme`, each replacing an inherited one of its id
/// whole and coming from `origin`.
fn merge_fields(
    scheme: &mut Scheme,
    origins: &mut BTreeMap<String, Origin>,
    layer: &Layer,
    source: Source<'_>,
    origin: Origin,
) -> Result<(), SchemeFileError> {
    for (id, spec) in &layer.fields {
        let name = id.get_ref();
        if let Some(existing) = scheme.field(name) {
            if existing.is_fixed() {
                return Err(source.error(
                    id.span().start,
                    format!("{name} is built in and cannot be replaced"),
                ));
            }
            scheme.fields.retain(|field| field.name != *name);
        }
        let field_type = FIELD_TYPES
            .iter()
            .find(|(type_id, _)| *type_id == spec.field_type.get_ref())
            .map(|(_, field_type)| *field_type)
            .ok_or_else(|| {
                let types: Vec<&str> = FIELD_TYPES.iter().map(|(id, _)| *id).collect();
                source.error(
                    spec.field_type.span().start,
                    format!(
                        "{:?} is not a field type; one of: {}",
                        spec.field_type.get_ref(),
                        types.join(", ")
                    ),
                )
            })?;
        let select = matches!(field_type, FieldType::SingleSelect | FieldType::MultiSelect);
        if select == spec.options.is_empty() {
            let message = if select {
                format!("{name} is a {} with no options", spec.field_type.get_ref())
            } else {
                format!("{name} has options, which only single_select and multi_select take")
            };
            return Err(source.error(spec.field_type.span().start, message));
        }
        let options = spec
            .options
            .iter()
            .map(|option| Choice {
                id: option.id.clone(),
                label: option.label.clone().unwrap_or_else(|| option.id.clone()),
            })
            .collect();
        let label = spec.label.as_deref().unwrap_or(name);
        scheme
            .add_field(Field::custom(name, label, field_type, options))
            .map_err(|err: SchemeError| source.error(id.span().start, err.to_string()))?;
        origins.insert(name.clone(), origin);
    }
    Ok(())
}

/// Checks the ids `remove.TABLE` lists: each once, and each one a layer below
/// defines and `removable` allows (`None` when undefined, `Some(false)` when
/// built in).
fn removed(
    source: Source<'_>,
    table: &str,
    ids: &[Spanned<String>],
    removable: impl Fn(&str) -> Option<bool>,
) -> Result<(), SchemeFileError> {
    for (at, id) in ids.iter().enumerate() {
        let name = id.get_ref();
        let message = if ids[..at].iter().any(|before| before.get_ref() == name) {
            format!("remove.{table} names {name} twice")
        } else {
            match removable(name) {
                None => format!("remove.{table} names {name}, which no layer below defines"),
                Some(false) => format!("{name} is built in and cannot be removed"),
                Some(true) => continue,
            }
        };
        return Err(source.error(id.span().start, message));
    }
    Ok(())
}

/// The Workflow `spec` describes, refused at the line of its fault.
fn workflow(
    source: Source<'_>,
    id: &Spanned<String>,
    spec: &WorkflowSpec,
) -> Result<Workflow, SchemeFileError> {
    let name = id.get_ref();
    let invalid = |at: &Spanned<String>| {
        source.error(
            at.span().start,
            SchemeError::InvalidId(at.get_ref().clone()).to_string(),
        )
    };
    if !is_id(name) {
        return Err(invalid(id));
    }
    if spec.statuses.is_empty() {
        return Err(source.error(id.span().start, format!("workflow {name} has no statuses")));
    }
    let mut statuses: Vec<Status> = Vec::new();
    for status in &spec.statuses {
        let status_id = status.id.get_ref();
        if !is_id(status_id) {
            return Err(invalid(&status.id));
        }
        if statuses.iter().any(|seen| seen.id == *status_id) {
            return Err(source.error(
                status.id.span().start,
                format!("workflow {name} lists status {status_id} twice"),
            ));
        }
        let category = Category::ALL
            .into_iter()
            .find(|category| category.id() == status.category.get_ref())
            .ok_or_else(|| {
                let ids: Vec<&str> = Category::ALL.iter().map(|category| category.id()).collect();
                source.error(
                    status.category.span().start,
                    format!(
                        "{:?} is not a status category; one of: {}",
                        status.category.get_ref(),
                        ids.join(", ")
                    ),
                )
            })?;
        statuses.push(Status {
            id: status_id.clone(),
            label: status.label.clone().unwrap_or_else(|| status_id.clone()),
            category,
        });
    }
    let start = spec.start.get_ref();
    if !statuses.iter().any(|status| status.id == *start) {
        return Err(source.error(
            spec.start.span().start,
            format!("workflow {name} starts in {start}, which is not one of its statuses"),
        ));
    }
    Ok(Workflow {
        name: name.clone(),
        start: start.clone(),
        statuses,
        transitions: Transitions::Any,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKSPACE: &str = "/ws/scheme.toml";
    const REPO: &str = "/repo/.config/tm/scheme.toml";

    fn at<'a>(path: &'a str, text: &'a str) -> Source<'a> {
        Source {
            path: Path::new(path),
            text,
        }
    }

    fn fault(workspace: &str, repo: &str) -> String {
        let workspace = (!workspace.is_empty()).then(|| at(WORKSPACE, workspace));
        let repo = (!repo.is_empty()).then(|| at(REPO, repo));
        resolve(workspace, repo, &["TM"], "TM")
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn no_file_is_the_built_in_scheme() {
        assert_eq!(
            resolve(None, None, &["TM"], "TM").unwrap(),
            Scheme::built_in()
        );
        let empty = resolve(Some(at(WORKSPACE, "")), Some(at(REPO, "")), &["TM"], "TM");
        assert_eq!(empty.unwrap(), Scheme::built_in());
    }

    #[test]
    fn a_projects_tables_merge_over_the_workspaces_by_id() {
        let workspace = r#"
[fields.area]
type = "number"

[fields.tier]
type = "single_select"
options = [{ id = "gold", label = "Gold" }]
"#;
        let repo = r#"
[projects.tm.fields.tier]
label = "Tier"
type = "text"

[projects.tm.fields.due]
type = "date"

[projects.tm.remove]
fields = ["area"]

[projects.OPS.fields.other]
type = "text"
"#;
        let layered = layered(
            Some(at(WORKSPACE, workspace)),
            Some(at(REPO, repo)),
            &["TM", "OPS"],
            "TM",
        )
        .unwrap();
        let origins: Vec<(&str, Origin)> = layered
            .origins
            .iter()
            .map(|(name, origin)| (name.as_str(), *origin))
            .collect();
        assert_eq!(
            origins,
            [("due", Origin::Repo), ("tier", Origin::Repo)],
            "area removed, other in another Project"
        );
        assert_eq!(layered.origin("title"), Origin::BuiltIn);
        let scheme = layered.scheme;
        assert_eq!(scheme.field("area"), None);
        assert_eq!(scheme.field("other"), None, "another Project's table");
        let tier = scheme.field("tier").unwrap();
        assert_eq!(
            (tier.label.as_str(), tier.field_type),
            ("Tier", FieldType::Text)
        );
        assert!(tier.options.is_empty(), "replaced whole");
        assert_eq!(scheme.field("due").unwrap().label, "due");
    }

    #[test]
    fn each_fault_names_its_file_and_line() {
        assert_eq!(
            fault("[fields.area]\ntype = \"colour\"\n", ""),
            format!(
                "{WORKSPACE}:2: \"colour\" is not a field type; one of: text, rich_text, number, date, datetime, boolean, single_select, multi_select, person"
            )
        );
        assert_eq!(
            fault("\n[fields.title]\ntype = \"text\"\n", ""),
            format!("{WORKSPACE}:2: title is built in and cannot be replaced")
        );
        assert_eq!(
            fault("[remove]\nfields = [\"waiting_on\"]\n", ""),
            format!("{WORKSPACE}:2: waiting_on is built in and cannot be removed")
        );
        assert_eq!(
            fault("[remove]\nfields = [\"gone\"]\n", ""),
            format!("{WORKSPACE}:2: remove.fields names gone, which no layer below defines")
        );
        assert_eq!(
            fault("", "[projects.OPS.fields.a]\ntype = \"text\"\n"),
            format!("{REPO}:1: Project OPS is not one the repo file maps; it maps TM")
        );
        assert_eq!(
            fault("\n\n[link_types.feeds]\nlabel = \"Feeds\"\n", ""),
            format!("{WORKSPACE}:3: link_types is not read by this tm yet")
        );
        let incident = "[workflows.incident]\nstart = \"open\"\nstatuses = [{ id = \"open\", category = \"todo\" }]\n";
        for (layer, line, message) in [
            (
                "[types.page]\nworkflow = \"gone\"\n",
                2,
                "type page names workflow gone, which no layer defines",
            ),
            (
                "[workflows.w]\nstart = \"later\"\nstatuses = [{ id = \"now\", category = \"todo\" }]\n",
                2,
                "workflow w starts in later, which is not one of its statuses",
            ),
            (
                "[workflows.w]\nstart = \"a\"\nstatuses = []\n",
                1,
                "workflow w has no statuses",
            ),
            (
                "[workflows.w]\nstart = \"a\"\nstatuses = [\n  { id = \"a\", category = \"todo\" },\n  { id = \"a\", category = \"done\" },\n]\n",
                5,
                "workflow w lists status a twice",
            ),
            (
                "[workflows.w]\nstart = \"a\"\nstatuses = [{ id = \"a\", category = \"open\" }]\n",
                3,
                "\"open\" is not a status category; one of: triage, todo, active, done, cancelled",
            ),
            (
                "[remove]\nworkflows = [\"default\"]\n",
                2,
                "workflow default is removed, but type epic uses it",
            ),
            (
                "[remove]\ntypes = [\"epic\", \"story\", \"task\", \"bug\", \"sub_task\", \"spike\", \"design\"]\n",
                2,
                "no Issue type is left; a Scheme needs one",
            ),
            (
                "[remove]\ntypes = [\"saga\"]\n",
                2,
                "remove.types names saga, which no layer below defines",
            ),
            (
                "[types.Page]\nworkflow = \"default\"\n",
                1,
                "\"Page\" must be lowercase letters, digits and underscores, starting with a letter",
            ),
        ] {
            assert_eq!(
                fault(layer, ""),
                format!("{WORKSPACE}:{line}: {message}"),
                "{layer}"
            );
        }
        assert!(
            resolve(Some(at(WORKSPACE, incident)), None, &["TM"], "TM").is_ok(),
            "a workflow no type uses yet is allowed"
        );
    }

    #[test]
    fn types_and_workflows_merge_by_id_and_keep_their_origins() {
        let workspace = r#"
[workflows.incident]
start = "open"
statuses = [
  { id = "open", label = "Open", category = "todo" },
  { id = "fixing", category = "active" },
  { id = "resolved", category = "done" },
]

[types.incident]
label = "Incident"
workflow = "incident"

[remove]
types = ["design"]
"#;
        let repo = r#"
[projects.TM.workflows.default]
start = "todo"
statuses = [
  { id = "todo", label = "To do", category = "todo" },
  { id = "done", label = "Done", category = "done" },
]

[projects.TM.types.spike]
label = "Research"
workflow = "default"
"#;
        let layered = layered(
            Some(at(WORKSPACE, workspace)),
            Some(at(REPO, repo)),
            &["TM"],
            "TM",
        )
        .unwrap();
        let scheme = &layered.scheme;
        assert_eq!(scheme.issue_type("design"), None);
        assert_eq!(scheme.issue_type("spike").unwrap().label, "Research");
        assert_eq!(
            scheme.category("incident", "fixing"),
            Some(tm_core::scheme::Category::Active)
        );
        assert_eq!(
            scheme
                .workflow("incident")
                .unwrap()
                .status("fixing")
                .unwrap()
                .label,
            "fixing"
        );
        assert_eq!(
            scheme.category("task", "in_progress"),
            None,
            "default replaced whole"
        );
        assert_eq!(scheme.workflow("task").unwrap().start, "todo");
        assert_eq!(layered.type_origin("incident"), Origin::Workspace);
        assert_eq!(layered.type_origin("spike"), Origin::Repo);
        assert_eq!(layered.type_origin("task"), Origin::BuiltIn);
        assert_eq!(layered.workflow_origin("default"), Origin::Repo);
        assert_eq!(layered.workflow_origin("incident"), Origin::Workspace);
    }

    #[test]
    fn a_type_follows_its_workflow_when_a_higher_layer_replaces_it() {
        let workspace = "[workflows.w]\nstart = \"a\"\nstatuses = [{ id = \"a\", category = \"todo\" }]\n[types.t]\nworkflow = \"w\"\n";
        let repo = "[projects.TM.workflows.w]\nstart = \"b\"\nstatuses = [{ id = \"b\", category = \"active\" }]\n";
        let scheme = resolve(
            Some(at(WORKSPACE, workspace)),
            Some(at(REPO, repo)),
            &["TM"],
            "TM",
        )
        .unwrap();
        assert_eq!(scheme.workflow("t").unwrap().start, "b");
        assert_eq!(
            scheme.issue_type("t").unwrap().label,
            "t",
            "label defaults to id"
        );
    }

    #[test]
    fn a_workflow_is_removed_when_unused_or_redefined_in_the_same_layer() {
        let workspace =
            "[workflows.w]\nstart = \"a\"\nstatuses = [{ id = \"a\", category = \"todo\" }]\n";
        let repo = "[projects.TM.remove]\nworkflows = [\"w\"]\n";
        let layered = layered(
            Some(at(WORKSPACE, workspace)),
            Some(at(REPO, repo)),
            &["TM"],
            "TM",
        )
        .unwrap();
        assert!(
            layered
                .scheme
                .workflows
                .iter()
                .all(|workflow| workflow.name != "w")
        );
        assert_eq!(layered.workflow_origin("w"), Origin::BuiltIn);

        let redefined = "[remove]\nworkflows = [\"default\"]\n[workflows.default]\nstart = \"a\"\nstatuses = [{ id = \"a\", category = \"todo\" }]\n";
        let scheme = resolve(Some(at(WORKSPACE, redefined)), None, &["TM"], "TM").unwrap();
        assert_eq!(scheme.workflow("epic").unwrap().start, "a");
    }

    #[test]
    fn a_type_carries_the_fields_it_lists() {
        let workspace = "[fields.severity]\ntype = \"text\"\n[fields.area]\ntype = \"text\"\n[types.bug]\nworkflow = \"default\"\nfields = [\"severity\"]\n[types.chore]\nworkflow = \"default\"\nfields = []\n";
        let scheme = resolve(Some(at(WORKSPACE, workspace)), None, &["TM"], "TM").unwrap();
        let field = |name: &str| scheme.field(name).unwrap();
        assert!(scheme.carries("bug", field("severity")));
        assert!(!scheme.carries("bug", field("area")));
        assert!(!scheme.carries("chore", field("severity")));
        assert!(scheme.carries("chore", field("waiting_on")));
        assert!(scheme.carries("task", field("area")));

        let repo = "[projects.TM.fields.area]\ntype = \"number\"\n";
        let scheme = resolve(
            Some(at(WORKSPACE, workspace)),
            Some(at(REPO, repo)),
            &["TM"],
            "TM",
        )
        .unwrap();
        assert_eq!(
            scheme.issue_type("bug").unwrap().fields,
            Some(vec!["severity".to_owned()]),
            "replacing a field keeps the types that list it"
        );
    }

    #[test]
    fn a_type_lists_each_field_once_and_only_defined_unfixed_ones() {
        let typed = |fields: &str| {
            format!(
                "[fields.area]\ntype = \"text\"\n[types.bug]\nworkflow = \"default\"\nfields = {fields}\n"
            )
        };
        assert_eq!(
            fault(&typed("[\"area\", \"area\"]"), ""),
            format!("{WORKSPACE}:5: type bug lists field area twice")
        );
        assert_eq!(
            fault(&typed("[\"gone\"]"), ""),
            format!(
                "{WORKSPACE}:5: type bug lists field gone, which neither this layer nor one below defines"
            )
        );
        assert_eq!(
            fault(&typed("[\"priority\"]"), ""),
            format!("{WORKSPACE}:5: type bug lists field priority, which every type carries")
        );
        assert_eq!(
            fault(
                &typed("[\"area\"]"),
                "[projects.TM.remove]\nfields = [\"area\"]\n"
            ),
            format!("{REPO}:2: field area is removed, but type bug lists it")
        );
        let redefined = "[projects.TM.remove]\nfields = [\"area\"]\n[projects.TM.types.bug]\nworkflow = \"default\"\n";
        let scheme = resolve(
            Some(at(WORKSPACE, &typed("[\"area\"]"))),
            Some(at(REPO, redefined)),
            &["TM"],
            "TM",
        )
        .unwrap();
        assert!(scheme.field("area").is_none());
        assert_eq!(scheme.issue_type("bug").unwrap().fields, None);
    }

    #[test]
    fn field_and_project_faults_name_their_file_and_line() {
        assert!(
            fault("[fields.Area]\ntype = \"text\"\n", "").starts_with(&format!("{WORKSPACE}:1: ")),
            "an id add_field refuses"
        );
        assert!(
            fault("fields = 3\n", "").starts_with(&format!("{WORKSPACE}:1: ")),
            "a TOML fault"
        );
        assert!(
            fault("", "[fields.a]\ntype = \"text\"\n").starts_with(&format!("{REPO}:1: ")),
            "the repo file holds Projects only"
        );
        assert_eq!(
            fault("[fields.waiting_on]\ntype = \"text\"\n", ""),
            format!("{WORKSPACE}:1: waiting_on is built in and cannot be replaced")
        );
        assert_eq!(
            fault(
                "[fields.area]\ntype = \"number\"\noptions = [{ id = \"a\" }]\n",
                ""
            ),
            format!(
                "{WORKSPACE}:2: area has options, which only single_select and multi_select take"
            )
        );
        assert_eq!(
            fault("[fields.tier]\ntype = \"single_select\"\n", ""),
            format!("{WORKSPACE}:2: tier is a single_select with no options")
        );
        assert_eq!(
            fault(
                "",
                "[projects.TM.fields.a]\ntype = \"text\"\n[projects.tm.fields.b]\ntype = \"text\"\n"
            ),
            format!("{REPO}:3: Project tm has tables twice")
        );
        assert_eq!(
            fault(
                "[fields.a]\ntype = \"text\"\n",
                "[projects.TM.remove]\nfields = [\"a\", \"a\"]\n"
            ),
            format!("{REPO}:2: remove.fields names a twice")
        );
        let other = "[projects.OPS.fields.a]\ntype = \"colour\"\n";
        let err = resolve(None, Some(at(REPO, other)), &["TM", "OPS"], "TM").unwrap_err();
        assert_eq!(
            err.line, 2,
            "another Project's tables are checked too: {err}"
        );
    }

    #[test]
    fn a_scheme_file_that_exists_but_cannot_be_read_is_refused() {
        let dir = std::env::temp_dir().join(format!("tm-scheme-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(dir.join(WORKSPACE_FILE)).unwrap();
        let err = read(&dir, None, &["TM"], "TM").unwrap_err();
        assert!(
            err.message.starts_with("cannot be read"),
            "a directory: {err}"
        );
        fs::remove_dir_all(dir.join(WORKSPACE_FILE)).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("gone.toml"), dir.join(WORKSPACE_FILE)).unwrap();
            let err = read(&dir, None, &["TM"], "TM").unwrap_err();
            assert!(
                err.message.starts_with("cannot be read"),
                "a dangling link: {err}"
            );
            fs::remove_file(dir.join(WORKSPACE_FILE)).unwrap();

            let repo = dir.join("repo");
            fs::create_dir_all(repo.join(".config")).unwrap();
            std::os::unix::fs::symlink(dir.join("moved"), repo.join(".config/tm")).unwrap();
            let err = read(&dir, Some(&repo), &["TM"], "TM").unwrap_err();
            assert!(
                err.message.starts_with("cannot be read"),
                "a dangling link to the directory: {err}"
            );
        }
        let plain = dir.join("plain");
        fs::create_dir_all(&plain).unwrap();
        fs::write(plain.join(".config"), "").unwrap();
        let scheme = read(&dir, Some(&plain), &["TM"], "TM").unwrap();
        assert_eq!(
            scheme,
            Scheme::built_in(),
            "a file named .config holds no Scheme"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
