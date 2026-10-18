use std::process::Command;

fn tm(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tm"))
        .args(args)
        .output()
        .expect("tm binary should start")
}

#[test]
fn scheme_show_json_lists_the_default_types_fields_and_statuses() {
    let output = tm(&["scheme", "show", "--json"]);
    assert!(output.status.success());
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["schema_version"], 1);
    let scheme = &document["data"];
    let ids = |list: &serde_json::Value, key: &str| -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|item| item[key].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        ids(&scheme["types"], "id"),
        [
            "epic", "story", "task", "bug", "sub_task", "spike", "design"
        ]
    );
    assert_eq!(
        scheme["status_categories"],
        serde_json::json!(["triage", "todo", "active", "done", "cancelled"])
    );
    let workflow = &scheme["workflows"][0];
    assert_eq!(workflow["start"], "todo");
    assert_eq!(workflow["transitions"], "any");
    let cancelled = workflow["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|status| status["id"] == "cancelled")
        .unwrap();
    assert_eq!(cancelled["category"], "cancelled");
    let title = scheme["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["name"] == "title")
        .unwrap();
    assert_eq!(title["required"], true);
    assert_eq!(title["type"], "text");
    assert!(ids(&scheme["fields"], "name").contains(&"labels".to_owned()));
    assert_eq!(
        ids(&scheme["priorities"], "id"),
        ["urgent", "high", "medium", "low", "none"]
    );
}

#[test]
fn an_unknown_command_prints_usage_and_fails() {
    let output = tm(&["scheme", "edit"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Usage: tm scheme"));
}

#[test]
fn scheme_show_with_a_type_gives_its_workflow_and_fields() {
    let output = tm(&["scheme", "show", "Bug", "--json"]);
    assert!(output.status.success(), "{output:?}");
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let data = &doc["data"];
    assert_eq!(data["type"]["id"], "bug");
    assert_eq!(data["workflow"]["start"], "todo");
    assert_eq!(data["workflow"]["statuses"][2]["id"], "in_progress");
    assert_eq!(data["workflow"]["statuses"][2]["category"], "active");
    let title = data["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["name"] == "title")
        .unwrap();
    assert_eq!(title["required"], true);

    let output = tm(&["scheme", "show", "task"]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.starts_with("type task (Task)\nworkflow default, starting in todo\n"),
        "{text}"
    );
    assert!(text.contains("field title (Title) [required]\n"), "{text}");
    assert!(
        !text.contains("field key"),
        "derived fields are left out of text: {text}"
    );

    let output = tm(&["scheme", "show", "sub-task", "--json"]);
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        doc["data"]["type"]["id"], "sub_task",
        "found by its label: {doc}"
    );

    let output = tm(&["scheme", "show", "saga", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["error"], "invalid_value");
    assert_eq!(doc["allowed"].as_array().unwrap().len(), 7);
}
