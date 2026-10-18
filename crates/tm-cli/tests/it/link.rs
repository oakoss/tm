use std::path::Path;

use serde_json::{Value, json};
use tm_core::Actor;
use tm_store::Store;

use crate::support::{Scratch, initialized, text, tm};

fn run(scratch: &Scratch, repo: &Path, args: &[&str]) -> (Option<i32>, Value) {
    let out = tm(scratch, repo, &[&["--json"], args].concat());
    assert_eq!(text(&out.stderr), "", "nothing reaches stderr in JSON mode");
    (
        out.status.code(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

fn count(store: &Store, sql: &str) -> i64 {
    store
        .connection()
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
}

#[test]
fn a_link_written_from_either_end_is_one_link() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["a", "b"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }

    let (code, doc) = run(
        &scratch,
        &repo,
        &["link", "TM-2", "is", "blocked", "by", "TM-1"],
    );
    assert_eq!(code, Some(0), "{doc}");
    assert_eq!(doc["data"]["link"]["type"], "blocks");
    assert_eq!(doc["data"]["link"]["name"], "is blocked by");
    assert_eq!(doc["data"]["link"]["from"]["key"], "TM-2");
    let batch = doc["data"]["batch"].as_str().unwrap().to_owned();
    let (code, doc) = run(&scratch, &repo, &["link", "TM-1", "BLOCKS", "TM-2"]);
    assert_eq!(code, Some(0));
    assert_eq!(
        doc["data"]["link"]["name"], "blocks",
        "the canonical name, not as typed"
    );
    assert_eq!(count(&store, "SELECT count(*) FROM links"), 1);
    assert_eq!(
        count(
            &store,
            "SELECT count(*) FROM field_versions WHERE field = 'links'"
        ),
        1
    );
    let logged: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM ops WHERE op_id = unhex(replace(?1, '-', ''))",
            [&batch],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(logged, 1, "the batch names the Link's operation");

    let (_, doc) = run(&scratch, &repo, &["show", "TM-2"]);
    assert_eq!(doc["data"]["links"][0]["name"], "is blocked by");

    let out = tm(&scratch, &repo, &["unlink", "TM-1", "blocks", "TM-2"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(text(&out.stdout), "TM-1 no longer blocks TM-2\n");
    assert_eq!(count(&store, "SELECT count(*) FROM links"), 0);
}

#[test]
fn unlinking_a_link_the_pair_does_not_hold_writes_nothing() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["a", "b"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    run(&scratch, &repo, &["link", "TM-1", "relates_to", "TM-2"]);
    let before = count(&store, "SELECT count(*) FROM ops");

    let (code, doc) = run(
        &scratch,
        &repo,
        &["unlink", "TM-2", "is", "blocked", "by", "TM-1"],
    );
    assert_eq!(code, Some(1));
    assert_eq!(doc["error"], "not_linked", "{doc}");
    assert_eq!(doc["type"], "blocks");
    assert_eq!(doc["held"], json!(["relates to"]));
    assert_eq!(count(&store, "SELECT count(*) FROM ops"), before);

    let (code, _) = run(
        &scratch,
        &repo,
        &["unlink", "TM-2", "relates", "to", "TM-1"],
    );
    assert_eq!(code, Some(0), "a symmetric Link unlinks from either end");
    assert_eq!(count(&store, "SELECT count(*) FROM links"), 0);

    run(&scratch, &repo, &["link", "TM-1", "blocks", "TM-2"]);
    let before = count(&store, "SELECT count(*) FROM ops");
    let (code, doc) = run(&scratch, &repo, &["unlink", "TM-2", "blocks", "TM-1"]);
    assert_eq!(code, Some(1), "the Link runs the other way: {doc}");
    assert_eq!(doc["held"], json!(["is blocked by"]));
    assert_eq!(count(&store, "SELECT count(*) FROM ops"), before);
}

#[test]
fn link_refuses_an_unknown_name_too_few_words_and_a_cycle() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["a", "b"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    let (code, doc) = run(&scratch, &repo, &["link", "TM-1", "unblocks", "TM-2"]);
    assert_eq!(code, Some(1));
    assert_eq!(doc["error"], "invalid_value", "{doc}");
    assert!(
        doc["allowed"]
            .as_array()
            .unwrap()
            .contains(&json!("is blocked by"))
    );
    let (code, doc) = run(&scratch, &repo, &["link", "TM-1", "TM-2"]);
    assert_eq!(
        (code, doc["error"].as_str()),
        (Some(2), Some("usage")),
        "{doc}"
    );
    let (code, doc) = run(&scratch, &repo, &["link", "TM-1", "blocks", "TM-9"]);
    assert_eq!(
        (code, doc["error"].as_str()),
        (Some(1), Some("not_found")),
        "{doc}"
    );

    run(&scratch, &repo, &["link", "TM-1", "blocks", "TM-2"]);
    let (code, doc) = run(&scratch, &repo, &["link", "TM-2", "blocks", "TM-1"]);
    assert_eq!(code, Some(1));
    assert_eq!(doc["error"], "cycle", "{doc}");
}
