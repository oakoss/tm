use serde_json::Value;
use tm_core::Actor;

use crate::support::{Scratch, initialized, text, tm, tm_piped};

#[test]
fn a_comment_is_given_or_read_from_stdin_and_shows_in_order() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();

    let out = tm(&scratch, &repo, &["--json", "comment", "tm-1", "first"]);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["issue"]["key"], "TM-1");
    assert_eq!(doc["data"]["comment"]["body"], "first");
    assert_eq!(doc["data"]["comment"]["actor"], "jace");
    assert_eq!(doc["data"]["batch"], doc["data"]["comment"]["id"]);

    let out = tm_piped(
        &scratch,
        &repo,
        &["comment", "TM-1", "-"],
        "from stdin\nsecond line\n",
    );
    assert!(out.status.success(), "{out:?}");
    assert!(
        text(&out.stdout).starts_with("commented on TM-1\nundo this with: tm undo "),
        "{out:?}"
    );

    let out = tm(&scratch, &repo, &["--json", "show", "TM-1"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let bodies: Vec<&str> = doc["data"]["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|comment| comment["body"].as_str().unwrap())
        .collect();
    assert_eq!(bodies, ["first", "from stdin\nsecond line\n"]);

    let out = tm(&scratch, &repo, &["--json", "comment", "TM-9", "x"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"], "not_found", "{doc}");

    for blank in ["", "  \n"] {
        let out = tm(&scratch, &repo, &["--json", "comment", "TM-1", blank]);
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["error"], "invalid_value", "{blank:?}: {doc}");
        assert_eq!(doc["field"], "body");
    }
    let out = tm(&scratch, &repo, &["--json", "comment", "TM-1", "-"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"], "invalid_value", "empty stdin: {doc}");
    let out = tm(&scratch, &repo, &["--json", "show", "TM-1"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["comments"].as_array().unwrap().len(), 2);
}
