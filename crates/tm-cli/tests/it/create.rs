use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use tm_core::Actor;
use tm_store::Store;
use tm_store::workspace::{PathProject, RepoFile};
use uuid::Uuid;

use crate::support::{Scratch, initialized, text, tm, tm_piped};

fn create(scratch: &Scratch, dir: &Path, args: &[&str]) -> (bool, Value) {
    let out = tm(scratch, dir, &[&["create", "--json"], args].concat());
    assert_eq!(text(&out.stderr), "", "nothing reaches stderr in JSON mode");
    (
        out.status.success(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

/// The operations written for `issue`, in log order, with their batch.
fn written(store: &Store, issue: Uuid) -> Vec<(Uuid, Uuid)> {
    let mut stmt = store
        .connection()
        .prepare(
            "SELECT o.op_id, b.batch FROM ops o JOIN op_batches b ON b.op_id = o.op_id
             WHERE o.issue_id = ?1 ORDER BY o.seq",
        )
        .unwrap();
    stmt.query_map([issue.as_bytes()], |row| {
        let id = |i: usize| {
            row.get::<_, Vec<u8>>(i)
                .map(|b| Uuid::from_slice(&b).unwrap())
        };
        Ok((id(0)?, id(1)?))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[test]
fn create_writes_one_batch_and_answers_in_the_show_shape() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let (parent, _) = store
        .create_issue(&Actor::human("jace"), project, "epic", "p")
        .unwrap();
    let (ok, doc) = create(
        &scratch,
        &repo,
        &[
            "--type",
            "bug",
            "--title",
            "Crash on start",
            "--set",
            "priority=high",
            "--set",
            "archived=false",
            "--label",
            "ui",
            "--label",
            "Crash",
            "--parent",
            "tm-1",
        ],
    );
    assert!(ok, "{doc}");
    let data = &doc["data"];
    assert_eq!(data["key"], "TM-2");
    assert_eq!(data["project"], "TM");
    assert_eq!(data["fields"]["type"], "bug");
    assert_eq!(data["fields"]["status"], "todo");
    assert_eq!(data["fields"]["priority"], "high");
    assert_eq!(data["fields"]["reporter"], "jace");
    assert_eq!(
        data["parent"],
        json!({ "id": parent.to_string(), "key": "TM-1" })
    );
    assert_eq!(
        data["sets"]["labels"],
        json!({ "crash": "Crash", "ui": "ui" })
    );
    assert_eq!(doc["warnings"], json!([]));

    let issue: Uuid = data["id"].as_str().unwrap().parse().unwrap();
    let ops = written(&store, issue);
    assert!(ops.len() > 1, "{ops:?}");
    assert_eq!(data["batch"], json!(ops[0].0.to_string()));
    assert!(ops.iter().all(|(_, batch)| *batch == ops[0].0), "{ops:?}");
}

#[test]
fn without_a_type_create_names_type_with_its_allowed_ids_and_not_status() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    let (ok, doc) = create(&scratch, &repo, &["--title", "it's x", "--label=-wip"]);
    assert!(!ok);
    assert_eq!(doc["error"], "missing_fields", "{doc}");
    assert_eq!(
        doc["fields"],
        json!([{
            "name": "type",
            "allowed": ["epic", "story", "task", "bug", "sub_task", "spike", "design"],
        }])
    );
    assert_eq!(
        doc["fix"],
        r"tm create --title='it'\''s x' --label=-wip --type <type>"
    );
    assert!(doc.get("issue").is_none(), "{doc}");
    let count: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM ops WHERE kind != 'create_project'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "nothing is written");
    let rerun = tm(
        &scratch,
        &repo,
        &[
            "create",
            "--json",
            "--title=it's x",
            "--label=-wip",
            "--type",
            "task",
        ],
    );
    assert!(rerun.status.success(), "{rerun:?}");

    let (_, doc) = create(&scratch, &repo, &["--type", "task"]);
    assert_eq!(doc["error"], "missing_fields", "{doc}");
    assert_eq!(doc["fields"], json!([{ "name": "title" }]));
    assert_eq!(doc["fix"], "tm create --type=task --title <title>");
}

#[test]
fn a_set_that_is_malformed_or_names_a_flagged_field_is_a_usage_error() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    for set in [
        "priority",
        "type=bug",
        "title=x",
        "parent=TM-1",
        "labels=ui",
    ] {
        let out = tm(
            &scratch,
            &repo,
            &[
                "create", "--json", "--type", "task", "--title", "t", "--set", set,
            ],
        );
        assert_eq!(out.status.code(), Some(2), "{set}: {out:?}");
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["error"], "usage", "{set}: {doc}");
    }
}

#[test]
fn a_field_reads_stdin_without_its_trailing_newline() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    let piped = |args: &[&str], input: &[u8]| {
        let out = tm_piped(
            &scratch,
            &repo,
            &[&["create", "--json", "--title", "t"], args].concat(),
            input,
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    let task = ["--type", "task", "--set", "description=-"];

    let doc = piped(&task, b"line one\nline two  \r\n");
    assert_eq!(
        doc["data"]["fields"]["description"], "line one\nline two  ",
        "only trailing line breaks are dropped: {doc}"
    );

    let doc = piped(&["--set", "description=-"], b"a\nb'c\n");
    assert_eq!(doc["error"], "missing_fields", "{doc}");
    assert_eq!(
        doc["fix"], "tm create --title=t --set='description=a\nb'\\''c' --type <type>",
        "the fix carries the value stdin gave"
    );

    let doc = piped(&[&task[..], &["--set", "description=x"]].concat(), b"y");
    assert_eq!(doc["error"], "invalid_value", "{doc}");
    assert_eq!(doc["field"], "description");

    let doc = piped(&task, b"\xff\n");
    assert_eq!(doc["error"], "invalid_value", "{doc}");
    assert_eq!(doc["reason"], "is not UTF-8");
    assert_eq!(doc["field"], "description");
    assert_eq!(doc["message"], "description from stdin is not UTF-8");

    let doc = piped(&task, b" \n");
    assert_eq!(doc["error"], "invalid_value", "{doc}");
    assert_eq!(doc["field"], "description");
    assert!(
        doc["reason"]
            .as_str()
            .unwrap()
            .contains("leave the field out"),
        "a blank value points to leaving the field out: {doc}"
    );

    let doc = piped(&[&task[..], &["--set", "assignee=-"]].concat(), b"x");
    assert_eq!(doc["error"], "invalid_value", "{doc}");
    assert_eq!(
        doc["message"],
        "only one field can read stdin, not description, assignee"
    );

    let issues: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM issues", [], |row| row.get(0))
        .unwrap();
    assert_eq!(issues, 1, "a refused value creates nothing");
}

#[test]
fn create_refuses_what_the_scheme_or_store_does_not_have() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    let task = ["--type", "task", "--title", "t"];
    for (args, code) in [
        (&[&task[..], &["--parent", "TM-9"]].concat(), "not_found"),
        (&[&task[..], &["--set", "nope=1"]].concat(), "unknown_field"),
        (
            &[&task[..], &["--set", "archived=yes"]].concat(),
            "invalid_value",
        ),
        (&vec!["--type", "saga", "--title", "t"], "invalid_value"),
    ] {
        let (ok, doc) = create(&scratch, &repo, args);
        let extra = &args[2..];
        assert!(!ok, "{extra:?}");
        assert_eq!(doc["error"], code, "{extra:?}: {doc}");
    }
}

#[test]
fn create_in_a_mapped_directory_files_into_that_project() {
    let scratch = Scratch::new();
    let (repo, mut store, _) = initialized(&scratch);
    store
        .create_project(&Actor::human("jace"), "TUI", "Terminal UI")
        .unwrap();
    let path = repo.join(".config/tm.toml");
    let mut file = RepoFile::parse(&fs::read_to_string(&path).unwrap()).unwrap();
    file.paths.push(PathProject {
        prefix: "crates/tui".into(),
        project: "TUI".into(),
    });
    fs::write(&path, file.render()).unwrap();
    let nested = repo.join("crates/tui/src");
    fs::create_dir_all(&nested).unwrap();

    let (ok, doc) = create(&scratch, &nested, &["--type", "task", "--title", "t"]);
    assert!(ok, "{doc}");
    assert_eq!(doc["data"]["key"], "TUI-1");
    let (ok, doc) = create(&scratch, &repo, &["--type", "task", "--title", "t"]);
    assert!(ok, "{doc}");
    assert_eq!(doc["data"]["key"], "TM-1");
}
