//! JSON mode: one document on stdout, nothing on stderr (ADR 0011).

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};

fn tm(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tm"));
    command.args(args).env_remove("TM_JSON");
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("tm binary should start")
}

/// The one document on stdout, with nothing on stderr.
fn document(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    serde_json::from_str(&text).unwrap()
}

#[test]
fn a_usage_error_is_a_document_and_exits_2() {
    let output = tm(&["scheme", "edit", "--json"], &[]);
    assert_eq!(output.status.code(), Some(2));
    let doc = document(&output);
    assert_eq!(doc["error"], "usage");
    assert_eq!(doc["schema_version"], 1);
    assert_eq!(doc["warnings"], json!([]));
    assert!(doc["message"].as_str().unwrap().contains("edit"), "{doc}");
}

#[test]
fn tm_json_selects_json_mode_without_the_flag() {
    let output = tm(&["scheme", "show"], &[("TM_JSON", "1")]);
    assert!(output.status.success());
    assert_eq!(document(&output)["data"]["name"], "default");
}

#[test]
fn help_and_bare_tm_are_documents_in_json_mode() {
    for args in [&["--json"][..], &["--help", "--json"][..]] {
        let output = tm(args, &[]);
        assert!(output.status.success(), "{args:?}");
        let doc = document(&output);
        assert!(
            doc["data"]["help"].as_str().unwrap().contains("Usage"),
            "{args:?}"
        );
    }
}

#[test]
fn a_refusal_is_a_document_on_stdout_and_exits_1() {
    let scratch =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("tm-json-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    // A repo of its own, so init reaches the home check wherever the build is.
    let made = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&scratch)
        .status()
        .unwrap();
    assert!(made.success());
    let output = Command::new(env!("CARGO_BIN_EXE_tm"))
        .args(["init", "--json"])
        .current_dir(&scratch)
        .env("TM_HOME", "relative/home")
        .env_remove("TM_JSON")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
    assert_eq!(output.status.code(), Some(1));
    let doc = document(&output);
    assert_eq!(doc["error"], "no_home", "{doc}");
    assert!(doc.get("data").is_none());
}

#[test]
fn a_missing_subcommand_is_a_usage_error() {
    let text = tm(&["git"], &[]);
    assert_eq!(text.status.code(), Some(2));
    let json = tm(&["git"], &[("TM_JSON", "1")]);
    assert_eq!(json.status.code(), Some(2));
    assert_eq!(document(&json)["error"], "usage");
}

#[test]
fn output_that_cannot_be_written_fails() {
    for args in ["scheme show --json", "scheme show"] {
        let status = Command::new("sh")
            .args([
                "-c",
                &format!("\"$0\" {args} 1</dev/null"),
                env!("CARGO_BIN_EXE_tm"),
            ])
            .env_remove("TM_JSON")
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(1), "{args} with stdout read-only");
    }
}

#[test]
fn text_mode_errors_go_to_stderr() {
    let output = tm(&["scheme", "edit"], &[]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Usage"));
}
