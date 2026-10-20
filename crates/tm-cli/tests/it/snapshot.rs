use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use tm_store::Store;
use tm_store::workspace::Workspace;
use uuid::Uuid;

use crate::support::{Scratch, initialized, tm};

fn snapshot(scratch: &Scratch, store: &Store) -> (PathBuf, Uuid) {
    let id = store.workspace_id().unwrap().unwrap();
    (Workspace::at(&scratch.home(), id).snapshot_path(), id)
}

fn titles(path: &std::path::Path, workspace: Uuid) -> Vec<String> {
    Store::open_snapshot(path, workspace)
        .unwrap()
        .connection()
        .prepare("SELECT title FROM issues ORDER BY title")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn create(scratch: &Scratch, repo: &std::path::Path) {
    let out = tm(
        scratch,
        repo,
        &["create", "--type", "task", "--title", "one"],
    );
    assert!(out.status.success(), "{out:?}");
}

#[test]
fn writes_publish_the_snapshot_and_reads_do_not() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    let (path, workspace) = snapshot(&scratch, &store);
    assert!(titles(&path, workspace).is_empty(), "init published one");

    create(&scratch, &repo);
    assert_eq!(titles(&path, workspace), ["one"]);

    fs::remove_file(&path).unwrap();
    let reads = [
        &["list"][..],
        &["ready"],
        &["show", "TM-1"],
        &["prime"],
        &["undo", "--dry-run"],
    ];
    for read in reads {
        let out = tm(&scratch, &repo, read);
        assert!(out.status.success(), "{read:?}: {out:?}");
    }
    assert!(!path.exists(), "a read published");

    let out = tm(&scratch, &repo, &["move", "TM-1", "in_progress"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(titles(&path, workspace), ["one"]);
}

#[test]
fn rebuild_and_a_record_import_publish() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    let (path, workspace) = snapshot(&scratch, &store);

    fs::remove_file(&path).unwrap();
    let out = tm(&scratch, &repo, &["rebuild"]);
    assert!(out.status.success(), "{out:?}");
    assert!(path.exists());

    let records = scratch.0.join("records.jsonl");
    let record = json!({"_type": "issue", "source": "t:1", "project": "TM", "type": "task", "title": "imported", "status": "todo"});
    fs::write(&records, format!("{record}\n")).unwrap();
    let out = tm(
        &scratch,
        &repo,
        &["import", "--records", records.to_str().unwrap()],
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(titles(&path, workspace), ["imported"]);
}

#[test]
fn restoring_an_export_publishes_into_a_new_or_existing_workspace() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    let (_, workspace) = snapshot(&scratch, &store);
    create(&scratch, &repo);
    let out = tm(&scratch, &repo, &["export"]);
    assert!(out.status.success(), "{out:?}");
    let file = scratch.0.join("export.jsonl");
    fs::write(&file, &out.stdout).unwrap();

    let other = Scratch::new();
    let path = Workspace::at(&other.home(), workspace).snapshot_path();
    let import = || tm(&other, &other.0, &["import", file.to_str().unwrap()]);
    let out = import();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(titles(&path, workspace), ["one"], "new Workspace");

    fs::remove_file(&path).unwrap();
    let out = import();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(titles(&path, workspace), ["one"], "existing Workspace");
}

#[test]
fn a_failed_publish_is_a_warning_after_the_stores_own() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    let (path, _) = snapshot(&scratch, &store);
    store
        .connection()
        .execute(
            "INSERT INTO unapplied (op_id, reason) VALUES (?1, ?2)",
            (Uuid::now_v7().as_bytes().to_vec(), "unknown field"),
        )
        .unwrap();
    fs::remove_file(&path).unwrap();
    fs::create_dir_all(path.join("in-the-way")).unwrap();
    let unapplied = "1 logged operation did not apply in full; tm doctor will list it";

    let out = tm(
        &scratch,
        &repo,
        &["--json", "create", "--type", "task", "--title", "one"],
    );
    assert!(out.status.success(), "the write stands: {out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let warnings = doc["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 2, "{doc}");
    assert_eq!(warnings[0], unapplied);
    assert!(
        warnings[1].as_str().unwrap().starts_with(&format!(
            "the read-only snapshot was not updated: {}: ",
            path.display()
        )),
        "{doc}"
    );

    let out = tm(&scratch, &repo, &["--json", "comment", "TM-404", "x"]);
    assert!(!out.status.success());
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        doc["warnings"],
        json!([unapplied]),
        "nothing written, nothing published"
    );
}
