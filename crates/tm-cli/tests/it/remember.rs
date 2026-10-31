//! `tm remember`, `tm memories` and `tm forget`, and the memories `tm prime`
//! shows every session (ADR 0017).

use std::path::Path;
use std::process::{Output, Stdio};

use serde_json::Value;

use crate::support::{Scratch, initialized, jace, text, tm};

fn data(out: &Output) -> Value {
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone()
}

fn claude(scratch: &Scratch, repo: &Path, args: &[&str]) -> Output {
    jace(scratch, repo, args)
        .env("TM_AGENT", "claude")
        .env("TM_SESSION", "s1")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn a_memory_is_remembered_listed_shown_and_forgotten() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    let out = tm(&scratch, &repo, &["remember", "Run the tests with -race"]);
    let said = text(&out.stdout);
    assert!(
        said.starts_with(
            "remembered run-the-tests-with: Run the tests with -race\nundo this with: tm undo "
        ),
        "{said}"
    );
    let named = data(&tm(
        &scratch,
        &repo,
        &[
            "--json",
            "remember",
            "--key",
            "deploys",
            "Deploys go\nthrough staging",
        ],
    ));
    assert_eq!(named["created"], true);
    assert_eq!(named["memory"]["key"], "deploys");

    let listed = text(&tm(&scratch, &repo, &["memories"]).stdout);
    assert!(
        listed.starts_with("deploys  Deploys go  (jace, under a minute ago)\n"),
        "{listed}"
    );
    assert!(listed.contains("run-the-tests-with  Run the tests with -race  (jace"));
    let found = text(&tm(&scratch, &repo, &["memories", "RACE"]).stdout);
    assert!(found.starts_with("run-the-tests-with  "), "{found}");
    assert!(!found.contains("deploys"), "{found}");
    let shown = text(&tm(&scratch, &repo, &["memories", "deploys"]).stdout);
    assert!(
        shown.ends_with("\n  Deploys go\n  through staging\n"),
        "{shown}"
    );
    let none = text(&tm(&scratch, &repo, &["memories", "kubernetes"]).stdout);
    assert_eq!(none, "no memory holds kubernetes\n");
    let by_word = text(&tm(&scratch, &repo, &["memories", "staging"]).stdout);
    assert!(
        by_word.starts_with("no memory has the key staging; these hold the word:\ndeploys  "),
        "{by_word}"
    );

    let forgot = text(&tm(&scratch, &repo, &["forget", "deploys"]).stdout);
    assert!(forgot.starts_with("forgot deploys\n"), "{forgot}");
    let all = data(&tm(&scratch, &repo, &["--json", "memories"]));
    assert_eq!(all["total"], 1);
    let again = tm(&scratch, &repo, &["--json", "forget", "deploys"]);
    assert!(!again.status.success());
    let doc: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(
        (&doc["error"], &doc["fix"]),
        (&"not_found".into(), &"tm memories".into())
    );
}

#[test]
fn a_named_key_replaces_and_warns_when_it_was_someone_elses() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    data(&tm(
        &scratch,
        &repo,
        &["--json", "remember", "--key", "db", "Use Postgres"],
    ));
    let out = claude(&scratch, &repo, &["remember", "--key", "db", "Use SQLite"]);
    assert!(out.status.success(), "{out:?}");
    assert!(text(&out.stdout).starts_with("db now reads: Use SQLite\n"));
    assert!(
        text(&out.stderr).contains("db held what jace wrote, now replaced: Use Postgres"),
        "{out:?}"
    );
    let shown = text(&tm(&scratch, &repo, &["memories"]).stdout);
    assert!(shown.starts_with("db  Use SQLite  (claude, "), "{shown}");
}

#[test]
fn a_memory_like_another_is_kept_apart_and_named() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    data(&tm(
        &scratch,
        &repo,
        &["--json", "remember", "Tests need the race flag"],
    ));
    let out = tm(
        &scratch,
        &repo,
        &["remember", "Tests need the race flag always"],
    );
    assert!(
        text(&out.stdout).starts_with("remembered tests-need-the-race-2: "),
        "{out:?}"
    );
    assert!(
        text(&out.stderr).contains(
            "this reads like tests-need-the-race; tm remember --key tests-need-the-race \"...\" updates it"
        ),
        "{out:?}"
    );
}

#[test]
fn prime_shows_the_newest_memories_and_counts_the_rest() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    for n in 1..=12 {
        let key = format!("fact-{n}");
        data(&tm(
            &scratch,
            &repo,
            &[
                "--json",
                "remember",
                "--key",
                &key,
                &format!("Fact number {n}"),
            ],
        ));
    }
    let prime = text(&claude(&scratch, &repo, &["prime"]).stdout);
    assert!(
        prime.contains("Remember (tm remember \"...\" adds one):\n  fact-12  Fact number 12  (jace, under a minute ago)\n"),
        "{prime}"
    );
    assert!(prime.contains("  and 2 more: tm memories\n"), "{prime}");
    assert!(!prime.contains("fact-1  "), "the oldest falls off: {prime}");
    let prime = data(&claude(&scratch, &repo, &["--json", "prime"]));
    assert_eq!(prime["memories"]["total"], 12);
    assert_eq!(prime["memories"]["memories"].as_array().unwrap().len(), 10);
}

#[test]
fn prime_ages_a_memory_by_the_clock_for_anyone() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    data(&tm(
        &scratch,
        &repo,
        &["--json", "remember", "--key", "race", "Tests need -race"],
    ));
    let three_days_ms = 3 * 86_400_000;
    store
        .connection()
        .execute(
            "UPDATE memories SET written_ms = written_ms - ?1",
            [three_days_ms],
        )
        .unwrap();
    let human = text(&tm(&scratch, &repo, &["prime"]).stdout);
    assert!(
        human.contains("  race  Tests need -race  (jace, 3d ago)\n"),
        "{human}"
    );
    let agent = text(&claude(&scratch, &repo, &["prime"]).stdout);
    assert!(agent.contains("(jace, 3d ago)"), "{agent}");
}

#[test]
fn a_blank_or_overlong_memory_is_refused() {
    let scratch = Scratch::new();
    let (repo, _, _) = initialized(&scratch);
    for body in [" ", &"x".repeat(501)] {
        let out = tm(&scratch, &repo, &["--json", "remember", body]);
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["error"], "invalid_value", "{out:?}");
    }
    let long = tm(&scratch, &repo, &["remember", &"x".repeat(501)]);
    assert!(text(&long.stderr).contains("(tm comment KEY)"), "{long:?}");
    let empty = text(&tm(&scratch, &repo, &["memories"]).stdout);
    assert_eq!(
        empty,
        "no memories in this Project; tm remember \"...\" adds one\n"
    );
}
