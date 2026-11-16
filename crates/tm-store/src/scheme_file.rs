//! The Scheme read from config (ADR 0022): the built-in Scheme, merged with
//! the Workspace's `scheme.toml`, merged with a Project's tables in the repo's
//! `.config/tm/scheme.toml`. Each layer merges over the one below by id.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tm_core::scheme::{Choice, Field, FieldType, Scheme, SchemeError, WAITING_ON};
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
    types: Option<Spanned<toml::Value>>,
    workflows: Option<Spanned<toml::Value>>,
    link_types: Option<Spanned<toml::Value>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Remove {
    #[serde(default)]
    fields: Vec<Spanned<String>>,
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
    let mut scheme = Scheme::built_in();
    if let Some(source) = workspace {
        let layer: Layer = source.parse()?;
        merge(&mut scheme, &layer, source)?;
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
        merge(&mut merged, layer, source)?;
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
    let workspace_path = workspace_dir.join(WORKSPACE_FILE);
    let repo_path = repo_dir.map(|dir| dir.join(REPO_FILE));
    let workspace_text = text(workspace_dir, &workspace_path)?;
    let repo_text = match (repo_dir, &repo_path) {
        (Some(dir), Some(path)) => text(dir, path)?,
        _ => None,
    };
    resolve(
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

/// `layer` over `scheme`: its removals, then its fields, each replacing an
/// inherited one of its id whole.
fn merge(scheme: &mut Scheme, layer: &Layer, source: Source<'_>) -> Result<(), SchemeFileError> {
    for (table, value) in [
        ("types", &layer.types),
        ("workflows", &layer.workflows),
        ("link_types", &layer.link_types),
    ] {
        if let Some(value) = value {
            return Err(source.error(
                value.span().start,
                format!("{table} is not read by this tm yet; only fields and remove.fields are"),
            ));
        }
    }
    for (at, id) in layer.remove.fields.iter().enumerate() {
        let name = id.get_ref();
        if layer.remove.fields[..at]
            .iter()
            .any(|before| before.get_ref() == name)
        {
            return Err(source.error(id.span().start, format!("remove.fields names {name} twice")));
        }
        match scheme.field(name) {
            None => {
                return Err(source.error(
                    id.span().start,
                    format!("remove.fields names {name}, which no layer below defines"),
                ));
            }
            Some(field) if fixed(field) => {
                return Err(source.error(
                    id.span().start,
                    format!("{name} is built in and cannot be removed"),
                ));
            }
            Some(_) => scheme.fields.retain(|field| field.name != *name),
        }
    }
    for (id, spec) in &layer.fields {
        let name = id.get_ref();
        if let Some(existing) = scheme.field(name) {
            if fixed(existing) {
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
    }
    Ok(())
}

/// A field no Scheme file may remove or replace: a built-in one, or the
/// question `tm ask` sets.
fn fixed(field: &Field) -> bool {
    field.builtin || field.name == WAITING_ON
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
        let scheme = resolve(
            Some(at(WORKSPACE, workspace)),
            Some(at(REPO, repo)),
            &["TM", "OPS"],
            "TM",
        )
        .unwrap();
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
            fault("\n\n[types.incident]\nlabel = \"Incident\"\n", ""),
            format!(
                "{WORKSPACE}:3: types is not read by this tm yet; only fields and remove.fields are"
            )
        );
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
