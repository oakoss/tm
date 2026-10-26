//! `tm show` prints an Issue's version, and `tm edit` and `tm move` given
//! `--if-version` refuse a write to a field changed since.

use std::path::Path;
use std::process::Output;

use serde_json::Value;

use crate::support::{Scratch, initialized, text, tm};

fn run(scratch: &Scratch, repo: &Path, words: &str) -> Output {
    tm(scratch, repo, &words.split(' ').collect::<Vec<_>>())
}

fn doc(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

fn version(scratch: &Scratch, repo: &Path) -> i64 {
    doc(&run(scratch, repo, "--json show TM-1"))["data"]["version"]
        .as_i64()
        .unwrap()
}

#[test]
fn show_and_a_write_give_the_version_and_a_stale_one_is_refused() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    run(&scratch, &repo, "create --type task --title t");
    run(&scratch, &repo, "create --type epic --title e");
    let read = version(&scratch, &repo);
    let shown = text(&run(&scratch, &repo, "show TM-1").stdout);
    assert!(shown.contains(&format!("\nversion: {read}\n")), "{shown}");

    let theirs = "edit TM-1 --set description=theirs --parent TM-2";
    assert!(run(&scratch, &repo, theirs).status.success());
    let now = version(&scratch, &repo);
    assert!(now > read);

    // The piped ones have empty stdin: read first, it would be refused as
    // blank instead.
    for (words, field) in [
        ("edit TM-1 --set description=mine", "description"),
        ("move TM-1 todo --set description=mine", "description"),
        ("edit TM-1 --set description=-", "description"),
        ("move TM-1 in_progress --set description=-", "description"),
        ("edit TM-1 --set title=- --no-parent", "parent"),
    ] {
        let words = format!("--json {words} --if-version {read}");
        let err = doc(&run(&scratch, &repo, &words));
        assert_eq!(err["error"], "stale_value", "{words}: {err}");
        let current = err["current"].as_object().unwrap();
        assert_eq!(current.keys().collect::<Vec<_>>(), [field], "{err}");
        assert_eq!(err["based_on"], read, "{err}");
        assert_eq!(err["version"], now, "{err}");
        assert_eq!(err["fix"], "tm show TM-1", "{err}");
    }
    let words = "--json edit TM-1 --set description=x";
    let err = doc(&run(&scratch, &repo, words));
    assert_eq!(err["error"], Value::Null, "a write based on what it read");
    for words in [
        "--json edit TM-1 --set description=y --if-version 99999",
        "--json edit TM-1 --set description=x --if-version 99999",
        "--json move TM-1 todo --if-version 99999",
    ] {
        let err = doc(&run(&scratch, &repo, words));
        assert_eq!(err["error"], "invalid_value", "ahead: {err}");
        assert_eq!(err["field"], "--if-version", "{err}");
    }

    let words = format!("--json edit TM-1 --set priority=high --if-version {read}");
    let out = run(&scratch, &repo, &words);
    assert!(out.status.success(), "another field: {out:?}");
    let words = format!("--json move TM-1 in_progress --if-version {read}");
    assert!(run(&scratch, &repo, &words).status.success(), "the status");
}
