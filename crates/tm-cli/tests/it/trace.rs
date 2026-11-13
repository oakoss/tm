//! `TM_LOG` traces to a file under tm's home, never to stderr (ADR 0020).

use std::fs;

use crate::support::{Scratch, initialized, jace, text};

#[test]
fn a_traced_command_logs_to_the_home_and_prints_as_untraced() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    let run = |log: Option<&str>| {
        let mut command = jace(
            &scratch,
            &repo,
            &["--json", "create", "--type", "task", "--title", "t"],
        );
        if let Some(log) = log {
            command.env("TM_LOG", log);
        }
        command.output().unwrap()
    };
    let traced = run(Some("debug"));
    assert!(traced.status.success(), "{traced:?}");
    assert_eq!(text(&traced.stderr), "");
    let doc: serde_json::Value = serde_json::from_slice(&traced.stdout).unwrap();
    assert_eq!(doc["data"]["key"], "TM-1", "{doc}");

    let log = fs::read_to_string(scratch.home().join("logs").join("tm.log")).unwrap();
    assert!(log.contains("command{name=\"create\""), "{log}");
    assert!(log.contains("write{drafts="), "{log}");
    assert!(!log.contains('\x1b'), "no colour codes: {log}");

    let quiet = run(None);
    assert!(quiet.status.success(), "{quiet:?}");
    assert_eq!(
        fs::read_to_string(scratch.home().join("logs").join("tm.log")).unwrap(),
        log,
        "unset, nothing is traced"
    );
}

#[test]
fn a_log_that_cannot_be_written_warns_in_text_and_stays_silent_in_json() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    fs::write(scratch.home().join("logs"), "not a directory").unwrap();
    let out = jace(&scratch, &repo, &["list"])
        .env("TM_LOG", "debug")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let stderr = text(&out.stderr);
    assert!(
        stderr.starts_with("tm: warning: TM_LOG is set, but ")
            && stderr.contains("cannot be written"),
        "{stderr}"
    );

    let out = jace(&scratch, &repo, &["--json", "list"])
        .env("TM_LOG", "debug")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(text(&out.stderr), "");
    serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap();
}

#[test]
fn an_unreadable_filter_warns_only_in_text_and_a_crate_filter_keeps_the_command() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    let run = |args: &[&str], log: &str| {
        jace(&scratch, &repo, args)
            .env("TM_LOG", log)
            .output()
            .unwrap()
    };
    let out = run(&["--json", "list"], "tm_store=debug,===");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(text(&out.stderr), "");
    let out = run(&["list"], "tm_store=debug,===");
    assert_eq!(
        text(&out.stderr),
        "tm: warning: TM_LOG's \"===\" is not a filter tm can read, so it is left out\n"
    );

    let path = scratch.home().join("logs").join("tm.log");
    let scan = ["git", "scan", "--dry-run"];
    run(&scan, "debug");
    assert!(fs::read_to_string(&path).unwrap().contains("git{"));
    fs::remove_file(&path).unwrap();
    run(&scan, "tm_store=debug");
    run(
        &["create", "--type", "task", "--title", "t"],
        "tm_store=debug",
    );
    let log = fs::read_to_string(&path).unwrap();
    let write = log.lines().find(|line| line.contains("written")).unwrap();
    assert!(write.contains("command{name=\"create\" pid="), "{log}");
    assert!(!log.contains("git{"), "tm-cli's own lines stay off: {log}");
}
