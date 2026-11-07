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
fn the_version_is_a_document_naming_it() {
    let doc = document(&tm(&["--version", "--json"], &[]));
    assert_eq!(doc["data"], json!({ "version": env!("CARGO_PKG_VERSION") }));
}

#[test]
fn a_usage_error_names_the_missing_arguments() {
    let doc = document(&tm(&["handoff", "X-1", "--json"], &[]));
    assert_eq!(
        doc["message"],
        "the following required arguments were not provided: --to <AGENT>, <NOTE>"
    );
}

#[test]
fn next_refuses_paths_with_peek_since_a_peek_takes_no_claim() {
    let doc = document(&tm(&["next", "--peek", "--paths", "src/**", "--json"], &[]));
    assert_eq!(doc["error"], "usage");
    assert!(
        doc["message"].as_str().unwrap().contains("--paths"),
        "{doc}"
    );
}

#[test]
fn the_version_without_json_is_text() {
    let output = tm(&["--version"], &[]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(text, format!("tm {}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn a_usage_error_keeps_the_values_clap_allows() {
    let doc = document(&tm(&["setup", "bogus", "--json"], &[]));
    assert_eq!(
        doc["message"],
        "invalid value 'bogus' for '<HARNESS>' [possible values: claude, codex]"
    );
}

#[test]
fn a_hook_that_fails_to_parse_under_json_says_why_in_text() {
    for (args, envs) in [
        (&["hook", "pre-edit"][..], &[("TM_JSON", "1")][..]),
        (&["--json", "hook", "pre-edit"], &[]),
    ] {
        let output = tm(args, envs);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        let said = String::from_utf8(output.stderr).unwrap();
        assert!(said.contains("<HARNESS>"), "{args:?}: {said}");
    }
}

#[test]
fn json_mode_stays_on_where_no_hook_runs() {
    for args in [
        &["--json", "--", "hook"][..],
        &["--json", "-V", "hook"],
        &["--json", "--help", "hook"],
        &["--json", "-hV", "hook"],
        &["--json", "--bogus", "hook"],
    ] {
        document(&tm(args, &[]));
    }
}

#[test]
fn a_hook_still_accepts_json_after_its_action() {
    let scratch = crate::support::Scratch::new();
    let output = crate::support::tm_piped(
        &scratch,
        &scratch.0,
        &["hook", "pre-edit", "claude", "--json"],
        r#"{"tool_input":{"file_path":"/x"}}"#,
    );
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
}

#[test]
fn tm_json_selects_json_mode_without_the_flag() {
    let output = tm(&["scheme", "show"], &[("TM_JSON", "1")]);
    assert!(output.status.success());
    assert_eq!(document(&output)["data"]["name"], "default");
}

#[test]
fn help_and_bare_tm_are_documents_in_json_mode() {
    let mut helps = Vec::new();
    for args in [&["--json"][..], &["--help", "--json"][..]] {
        let output = tm(args, &[]);
        assert!(output.status.success(), "{args:?}");
        let help = document(&output)["data"]["help"]
            .as_str()
            .unwrap()
            .to_owned();
        for heading in ["\nEveryday:\n", "\nSetup and data:\n", "\nOptions:\n"] {
            assert!(help.contains(heading), "{args:?}: {help}");
        }
        assert!(!help.contains("\nCommands:\n"), "{args:?}: {help}");
        helps.push(help);
    }
    assert_eq!(helps[0], helps[1], "bare tm and tm --help print one help");
    let text = String::from_utf8(tm(&["--help"], &[]).stdout).unwrap();
    assert_eq!(
        text.trim_end(),
        helps[1].trim_end(),
        "the text and JSON helps"
    );
}

#[test]
fn tm_help_help_keeps_its_own_usage_under_the_grouped_listing() {
    let output = tm(&["help", "help"], &[]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Usage: tm help [COMMAND]..."), "{text}");
    for listing in ["\nCommands:\n", "\nEveryday:\n"] {
        assert!(!text.contains(listing), "{text}");
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
