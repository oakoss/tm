//! A View changed from `tm tui`, written into the repo file it came from
//! with the rest of that file kept as written.

use std::fs;
use std::path::Path;

use tm_store::view::View;
use tm_store::workspace::RepoFile;
use tm_tui::ViewEdit;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

/// The repo file text `before` becomes with `edit` made, named `file` in
/// any refusal.
///
/// # Errors
///
/// Why the change cannot be made: the View changed since it was read, the
/// name is taken, the Views are written in a form tm does not edit, or the
/// result would read back as anything but this one change.
pub(crate) fn changed(before: &str, edit: &ViewEdit, file: &str) -> Result<String, String> {
    let read = RepoFile::parse(before).map_err(|why| format!("{file}: {why}"))?;
    let at = match &edit.was {
        Some(was) => Some(
            read.views
                .iter()
                .position(|view| view == was)
                .ok_or_else(|| {
                    format!("{file} changed since the Views were read; press v to read them again")
                })?,
        ),
        None => None,
    };
    if let Some(becomes) = &edit.becomes {
        let taken = read
            .views
            .iter()
            .enumerate()
            .any(|(other, view)| Some(other) != at && view.name == becomes.name);
        if taken {
            return Err(format!(
                "{file} already has a View named {:?}",
                becomes.name
            ));
        }
    }
    let mut expected = read.clone();
    match (at, &edit.becomes) {
        (Some(at), Some(becomes)) => expected.views[at] = becomes.clone(),
        (Some(at), None) => {
            expected.views.remove(at);
        }
        (None, Some(becomes)) => expected.views.push(becomes.clone()),
        (None, None) => return Ok(before.to_owned()),
    }

    let mut doc: DocumentMut = before.parse().map_err(|err| format!("{file}: {err}"))?;
    let tables = match doc.get_mut("views") {
        None => {
            doc.insert("views", Item::ArrayOfTables(ArrayOfTables::new()));
            doc["views"].as_array_of_tables_mut()
        }
        Some(item) => item.as_array_of_tables_mut(),
    }
    .ok_or_else(|| {
        format!("{file} writes its Views inline; write them as [[views]] tables to edit them here")
    })?;
    match (at, &edit.becomes) {
        (Some(at), Some(becomes)) => {
            let table = tables
                .get_mut(at)
                .ok_or_else(|| format!("{file}: View {at} is missing"))?;
            // What is written above its header, such as a comment, stays.
            let above = table.decor().clone();
            *table = table_of(becomes)?;
            *table.decor_mut() = above;
        }
        (Some(at), None) => {
            tables.remove(at);
        }
        (None, Some(becomes)) => {
            let mut table = table_of(becomes)?;
            // Set apart from whatever ends the file before it.
            table.decor_mut().set_prefix("\n");
            tables.push(table);
        }
        (None, None) => {}
    }
    if tables.is_empty() {
        doc.remove("views");
    }
    let after = doc.to_string();
    match RepoFile::parse(&after) {
        Ok(written) if written == expected => Ok(after),
        _ => Err(format!(
            "tm could not write that change to {file} without disturbing the rest of it"
        )),
    }
}

/// Whether the person at `tm tui` may change the Views in `path`: never in
/// a read-only Snapshot, nor as an Agent, as a file outside the Workspace
/// is written only on a person's yes.
pub(crate) fn editable(path: &Path, snapshot: bool, agent: bool) -> Result<(), String> {
    let file = path.display().to_string();
    if snapshot {
        Err(format!(
            "this is a read-only Snapshot, so {file} is not edited here"
        ))
    } else if agent {
        Err(format!(
            "an Agent is shown the Views; a person edits {file} from their own terminal"
        ))
    } else {
        writable(path, &file)
    }
}

/// Refuses a symlink, whose rename would replace the link with a copy, and
/// a read-only file.
pub(crate) fn writable(path: &Path, file: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(format!(
            "{file} is a symlink, so tm leaves it alone; edit the file it points to instead"
        )),
        Ok(meta) if meta.permissions().readonly() => {
            Err(format!("{file} is read-only, so tm leaves it alone"))
        }
        _ => Ok(()),
    }
}

/// `view` as a `[[views]]` table with no position, so it is written where
/// it is put.
fn table_of(view: &View) -> Result<Table, String> {
    let doc: DocumentMut = view.snippet().parse().map_err(|err| format!("{err}"))?;
    let mut table = doc["views"]
        .as_array_of_tables()
        .and_then(|tables| tables.get(0))
        .cloned()
        .ok_or_else(|| "a View did not render as a table".to_owned())?;
    unplace(&mut table);
    Ok(table)
}

fn unplace(table: &mut Table) {
    table.set_position(None);
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(inner) => unplace(inner),
            Item::ArrayOfTables(tables) => tables.iter_mut().for_each(unplace),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use tm_store::view::{Column, Layout, Swimlanes};

    use super::*;

    const FILE: &str = r#"workspace = "01a15a5a-ff7b-740f-a358-13853ef2142a"
project = "TM"

# Mine, kept as I wrote it.
[[views]]
name = "Mine"
filter = "assignee:@me"

[[views]]
name = "Flow"
layout = "board"

[[views.columns]]
status = "todo"
filter = "priority:high"

[views.swimlanes]
field = "assignee"

# The last one.
[[views]]
name = "Bugs"
filter = "type:bug"

[[paths]]
prefix = "web"
project = "WEB"
"#;

    fn views() -> Vec<View> {
        RepoFile::parse(FILE).unwrap().views
    }

    fn renamed(view: &View, name: &str) -> View {
        View {
            name: name.into(),
            ..view.clone()
        }
    }

    #[test]
    fn a_view_changes_in_place_keeping_the_rest_as_written() {
        let views = views();
        let mut flow = renamed(&views[1], "Flow board");
        flow.columns.push(Column {
            status: "done".into(),
            filter: String::new(),
        });
        flow.swimlanes = Some(Swimlanes {
            field: "priority".into(),
            filter: "-priority:none".into(),
        });
        let edit = ViewEdit {
            was: Some(views[1].clone()),
            becomes: Some(flow.clone()),
        };
        let after = changed(FILE, &edit, "tm.toml").unwrap();
        assert!(after.contains("# Mine, kept as I wrote it.\n[[views]]\nname = \"Mine\""));
        assert!(after.contains("# The last one.\n[[views]]\nname = \"Bugs\""));
        assert!(
            after.ends_with("[[paths]]\nprefix = \"web\"\nproject = \"WEB\"\n"),
            "{after}"
        );
        let read = RepoFile::parse(&after).unwrap().views;
        assert_eq!(read, [views[0].clone(), flow, views[2].clone()]);

        let edit = ViewEdit {
            was: Some(views[2].clone()),
            becomes: Some(renamed(&views[2], "Defects")),
        };
        let after = changed(FILE, &edit, "tm.toml").unwrap();
        assert!(
            after.contains("\n\n# The last one.\n[[views]]\nname = \"Defects\""),
            "what is above its header stays: {after}"
        );
    }

    #[test]
    fn a_view_is_added_after_the_rest_or_removed() {
        let views = views();
        let new = View {
            name: "New".into(),
            filter: "label:x".into(),
            layout: Layout::List,
            project: None,
            columns: Vec::new(),
            swimlanes: None,
        };
        let add = ViewEdit {
            was: None,
            becomes: Some(new.clone()),
        };
        let after = changed(FILE, &add, "tm.toml").unwrap();
        let mut expected = views.clone();
        expected.push(new);
        assert_eq!(RepoFile::parse(&after).unwrap().views, expected);
        assert!(
            after.contains("name = \"Bugs\"\nfilter = \"type:bug\"\n\n[[views]]\nname = \"New\""),
            "set apart from the View before: {after}"
        );

        let remove = ViewEdit {
            was: Some(views[1].clone()),
            becomes: None,
        };
        let after = changed(FILE, &remove, "tm.toml").unwrap();
        assert_eq!(
            RepoFile::parse(&after).unwrap().views,
            [views[0].clone(), views[2].clone()]
        );
        assert!(!after.contains("[views.swimlanes]"), "{after}");
        assert!(after.contains("# The last one."), "{after}");
    }

    #[test]
    fn a_change_is_refused_when_it_cannot_be_made_as_asked() {
        let views = views();
        let mut gone = views[0].clone();
        gone.filter = "something else".into();
        let stale = ViewEdit {
            was: Some(gone),
            becomes: None,
        };
        let why = changed(FILE, &stale, "tm.toml").unwrap_err();
        assert!(why.contains("changed since the Views were read"), "{why}");

        let taken = ViewEdit {
            was: Some(views[0].clone()),
            becomes: Some(renamed(&views[0], "Bugs")),
        };
        let why = changed(FILE, &taken, "tm.toml").unwrap_err();
        assert!(why.contains("already has a View named \"Bugs\""), "{why}");

        let inline = "workspace = \"01a15a5a-ff7b-740f-a358-13853ef2142a\"\nproject = \"TM\"\nviews = [{ name = \"Mine\" }]\n";
        let mine = RepoFile::parse(inline).unwrap().views.remove(0);
        let edit = ViewEdit {
            was: Some(mine.clone()),
            becomes: Some(renamed(&mine, "Ours")),
        };
        let why = changed(inline, &edit, "tm.toml").unwrap_err();
        assert!(why.contains("inline"), "{why}");
    }

    #[test]
    fn the_first_view_starts_a_views_table_and_the_last_removed_ends_it() {
        let bare = "workspace = \"01a15a5a-ff7b-740f-a358-13853ef2142a\"\nproject = \"TM\"\n";
        let view = View {
            name: "Only".into(),
            filter: String::new(),
            layout: Layout::Board,
            project: Some("TM".into()),
            columns: Vec::new(),
            swimlanes: None,
        };
        let add = ViewEdit {
            was: None,
            becomes: Some(view.clone()),
        };
        let after = changed(bare, &add, "tm.toml").unwrap();
        assert_eq!(
            RepoFile::parse(&after).unwrap().views,
            std::slice::from_ref(&view)
        );
        let remove = ViewEdit {
            was: Some(view),
            becomes: None,
        };
        assert_eq!(changed(&after, &remove, "tm.toml").unwrap(), bare);
    }
}
