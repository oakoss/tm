use serde_json::Value;
use tm_core::Actor;

use crate::support::{Scratch, initialized, text, tm, tm_command, tm_piped};

/// A mistyped Key is refused before stdin is read, so the text piped in is
/// left unread for the command run again: an empty pipe still reports the
/// Key, not the empty text.
#[test]
fn a_mistyped_key_is_named_before_stdin_is_read() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["t", "child"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    let out = tm(&scratch, &repo, &["edit", "TM-2", "--parent=TM-1"]);
    assert!(out.status.success(), "{out:?}");
    // The store trims a label before it looks for control characters.
    for label in ["--label=bug\n", "--label=\tdocs"] {
        let out = tm(&scratch, &repo, &["edit", "TM-1", label]);
        assert!(out.status.success(), "{label:?}: {out:?}");
    }
    let piped = "--set=description=-";
    let create = ["create", "--type=task", "--title=c", "--parent=TM-9", piped];
    let both = ["edit", "TM-1", "--label=x", "--remove-label=x", piped];
    let unset = ["edit", "TM-1", "--unset=priority", piped];
    let field = ["create", "--type=task", "--title=c", "--set=bogus=-"];
    let kind = ["create", "--type=chore", "--title=c", piped];
    let computed = ["create", "--type=task", "--title=c", "--set=reporter=-"];
    let label = ["create", "--type=task", "--title=c", "--label= ", piped];
    let cleared = ["edit", "TM-1", "--unset=created", piped];
    let blank = ["edit", "TM-1", "--label= ", piped];
    let bell = ["edit", "TM-1", "--label=a\u{7}", piped];
    let own = ["edit", "TM-1", "--parent=TM-1", piped];
    let below = ["edit", "TM-1", "--parent=TM-2", piped];
    let required = ["edit", "TM-1", "--unset=title", piped];
    for (args, error) in [
        (&own[..], "invalid_value"),
        (&below, "cycle"),
        (&required, "missing_fields"),
        (&computed[..], "invalid_value"),
        (&label, "invalid_value"),
        (&cleared, "invalid_value"),
        (&blank, "invalid_value"),
        (&bell, "invalid_value"),
        (&["edit", "TM-1", "--set=agent=-"], "invalid_value"),
        (&["move", "TM-1", "done", "--set=key=-"], "invalid_value"),
        (&["comment", "TM-9", "-"][..], "not_found"),
        (&["edit", "TM-9", piped], "not_found"),
        (&["edit", "TM-1", "--parent", "TM-9", piped], "not_found"),
        (&["edit", "TM-1", "--set=bogus=-"], "unknown_field"),
        (&unset, "invalid_value"),
        (&both, "invalid_value"),
        (&["move", "TM-9", "done", piped], "not_found"),
        (&["move", "TM-1", "shipped", piped], "invalid_value"),
        (&["move", "TM-1", "done", "--set=bogus=-"], "unknown_field"),
        (&create, "not_found"),
        (&field, "unknown_field"),
        (&kind, "invalid_value"),
    ] {
        let out = tm_piped(&scratch, &repo, &[&["--json"], args].concat(), "");
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["error"], error, "{args:?}: {doc}");
        let said = doc["message"].as_str().unwrap();
        assert!(!said.contains("from stdin"), "{args:?}: {doc}");
        if error == "cycle" {
            let keys = serde_json::json!(["TM-1", "TM-2", "TM-1"]);
            assert_eq!(doc["keys"], keys, "named by Key: {doc}");
        }
    }
}

/// What a merge left inconsistent stays editable: the early checks refuse
/// only what the write itself would.
#[test]
fn an_edit_a_merge_left_inconsistent_still_writes() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["t", "child"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    let out = tm(&scratch, &repo, &["edit", "TM-2", "--parent=TM-1"]);
    assert!(out.status.success(), "{out:?}");
    let child = store.issue_by_key("TM-2").unwrap().unwrap();
    let (cycle, untitled, blank) = (
        "UPDATE issues SET parent_id = ?1 WHERE parent_id IS NULL",
        "UPDATE issues SET title = NULL WHERE title = 'child'",
        "UPDATE issues SET title = '   ' WHERE title = 't'",
    );
    let conn = store.connection();
    conn.execute(cycle, [child.as_bytes()]).unwrap();
    conn.execute(untitled, []).unwrap();
    conn.execute(blank, []).unwrap();

    let piped = "--set=description=-";
    for (text, args) in [
        ("cycle", ["--json", "edit", "TM-1", "--parent=TM-2", piped]),
        ("empty", ["--json", "edit", "TM-2", "--unset=title", piped]),
        ("blank", ["--json", "edit", "TM-1", "--unset=title", piped]),
    ] {
        let out = tm_piped(&scratch, &repo, &args, format!("{text}\n"));
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        let changed = doc["data"]["changed"].as_array().unwrap();
        let wrote = |change: &&Value| change["field"] == "description";
        let description = changed.iter().find(wrote).unwrap();
        assert_eq!(description["after"], text, "{args:?}: {doc}");
    }
}

/// Who writes is known before stdin is read, so a missing identity keeps
/// the piped text too.
#[test]
fn a_missing_identity_is_named_before_stdin_is_read() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "t")
        .unwrap();
    for args in [
        &["comment", "TM-1", "-"][..],
        &["edit", "TM-1", "--set=description=-"],
        &["move", "TM-1", "done", "--set=description=-"],
        &["create", "--type=task", "--title=c", "--set=description=-"],
    ] {
        let out = tm_command()
            .arg("--json")
            .args(args)
            .current_dir(&repo)
            .env("TM_HOME", scratch.home())
            .env("GIT_CONFIG_GLOBAL", scratch.git_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["error"], "no_identity", "{args:?}: {doc}");
    }
}

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
    let out = tm_piped(
        &scratch,
        &repo,
        &["--json", "comment", "TM-1", "-"],
        b"\xff",
    );
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"], "invalid_value", "{doc}");
    assert_eq!(doc["field"], "body");
    assert_eq!(doc["reason"], "is not UTF-8");
    let out = tm_piped(&scratch, &repo, &["--json", "comment", "TM-1", "-"], "  \n");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["reason"], "is blank", "a blank piped body: {doc}");
    let said = doc["message"].as_str().unwrap();
    assert_eq!(said, "body from stdin is blank", "the pipe was empty");
    let out = tm(&scratch, &repo, &["--json", "show", "TM-1"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["comments"].as_array().unwrap().len(), 2);
}
