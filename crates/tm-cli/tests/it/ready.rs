use serde_json::{Value, json};
use tm_core::{Actor, Change};
use tm_store::Draft;

use crate::support::{Scratch, initialized, text, tm};

#[test]
fn ready_lists_startable_work_with_what_each_unblocks() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let out = tm(&scratch, &repo, &["ready"]);
    assert_eq!(text(&out.stdout), "nothing is ready\n");

    let jace = Actor::human("jace");
    let mut new = |title: &str| store.create_issue(&jace, project, "task", title).unwrap().0;
    let first = new("first");
    let urgent = new("urgent");
    let held = new("held");
    store
        .append(Draft {
            actor: jace.clone(),
            project,
            subject: urgent,
            change: Change::SetField {
                field: "priority".into(),
                label: String::new(),
                value: json!("urgent"),
                value_label: None,
            },
        })
        .unwrap();
    store.link(&jace, "blocks", first, held).unwrap();

    let out = tm(&scratch, &repo, &["ready", "--json"]);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let data = &doc["data"];
    assert_eq!(data["total"], 2);
    assert_eq!(data["issues"][0]["key"], "TM-2", "most urgent first: {doc}");
    assert_eq!(data["issues"][0]["unblocks"], json!([]));
    assert_eq!(data["issues"][1]["key"], "TM-1");
    assert_eq!(data["issues"][1]["project"], "TM");
    assert_eq!(
        data["issues"][1]["unblocks"],
        json!([{ "id": held.to_string(), "key": "TM-3" }])
    );

    let out = tm(&scratch, &repo, &["ready"]);
    let shown = text(&out.stdout);
    assert_eq!(shown.lines().count(), 2, "{shown}");
    assert!(shown.ends_with("first  (unblocks TM-3)\n"), "{shown}");

    let out = tm(&scratch, &repo, &["ready", "--json", "--limit", "1"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["issues"].as_array().unwrap().len(), 1);
    assert_eq!(doc["data"]["total"], 2);
}
