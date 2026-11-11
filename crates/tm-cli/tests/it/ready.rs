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

#[test]
fn ready_and_prime_show_the_mapped_projects_queue_or_every_one_without_it() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    let (web, _) = store.create_project(&jace, "WEB", "Web").unwrap();
    store.create_issue(&jace, project, "task", "here").unwrap();
    store.create_issue(&jace, web, "task", "there").unwrap();
    let (listed, _) = store.create_issue(&jace, web, "task", "listed").unwrap();
    let (list, _) = store
        .create_shortlist(&jace, web, "theirs", &["bob".to_owned()], None)
        .unwrap();
    store
        .shortlist_add(&jace, list, &[listed], tm_store::Place::End)
        .unwrap();
    drop(store);
    let passed = |doc: &Value| doc["data"]["ready"]["on_shortlists"].clone();
    let keys = |doc: &Value| -> Vec<String> {
        doc["data"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|issue| issue["key"].as_str().unwrap().to_owned())
            .collect()
    };

    let out = tm(&scratch, &repo, &["ready", "--json"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(keys(&doc), ["TM-1"], "{doc}");
    assert_eq!(doc["data"]["total"], 1);
    assert_eq!(doc["data"]["issues"][0]["project"], "TM");
    assert_eq!(doc["warnings"], json!([]), "{doc}");
    let primed = text(&tm(&scratch, &repo, &["prime"]).stdout);
    assert!(
        primed.contains("Ready (1)") && !primed.contains("WEB-1"),
        "{primed}"
    );
    let doc: Value =
        serde_json::from_slice(&tm(&scratch, &repo, &["prime", "--json"]).stdout).unwrap();
    assert_eq!(passed(&doc), 0, "{doc}");

    let file = repo.join(".config/tm.toml");
    let mapped = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, mapped.replace("\"TM\"", "\"NOPE\"")).unwrap();
    let out = tm(&scratch, &repo, &["ready", "--json"]);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let mut every = keys(&doc);
    every.sort();
    assert_eq!(every, ["TM-1", "WEB-1", "WEB-2"], "{doc}");
    let warned = doc["warnings"][0].as_str().unwrap_or_default();
    assert!(
        warned.starts_with("the ready queue spans every Project"),
        "{doc}"
    );
    let primed = text(&tm(&scratch, &repo, &["prime"]).stdout);
    assert!(primed.contains("WEB-1"), "{primed}");
    let doc: Value =
        serde_json::from_slice(&tm(&scratch, &repo, &["prime", "--json"]).stdout).unwrap();
    assert_eq!(passed(&doc), 1, "{doc}");
}
