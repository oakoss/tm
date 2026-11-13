//! A person's write records their name from git's `user.name` (ADR 0019).

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::process::{Output, Stdio};

use tm_store::Store;

use crate::support::{Scratch, initialized, jace, text};

/// `tm` in `repo` as the person git calls `kim@example.com` and `name`, with
/// no `TM_ACTOR`.
fn as_git_user(
    scratch: &Scratch,
    repo: &std::path::Path,
    name: impl AsRef<OsStr>,
    args: &[&str],
) -> Output {
    let mut command = jace(scratch, repo, args);
    command
        .env_remove("TM_ACTOR")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "user.email")
        .env("GIT_CONFIG_VALUE_0", "kim@example.com")
        .env("GIT_CONFIG_KEY_1", "user.name")
        .env("GIT_CONFIG_VALUE_1", name)
        .stdin(Stdio::null());
    command.output().unwrap()
}

fn reopen(scratch: &Scratch) -> Store {
    let mut workspaces = std::fs::read_dir(scratch.home().join("workspaces")).unwrap();
    Store::open(&workspaces.next().unwrap().unwrap().path().join("store.db")).unwrap()
}

fn names_written(store: &Store) -> i64 {
    store
        .connection()
        .query_row(
            "SELECT count(*) FROM ops WHERE kind = 'set_name'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn a_persons_write_records_their_git_name_and_undo_still_names_their_write() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    let reopen = || reopen(&scratch);
    let create = ["create", "--type", "task", "--title", "a"];
    let made = as_git_user(&scratch, &repo, "Kim Lee", &create);
    assert!(made.status.success(), "{made:?}");
    assert_eq!(
        reopen().name_of("kim@example.com").unwrap().as_deref(),
        Some("Kim Lee")
    );
    let again = as_git_user(&scratch, &repo, "Kim Lee", &create);
    assert!(again.status.success(), "{again:?}");
    assert_eq!(
        names_written(&reopen()),
        1,
        "an unchanged name is not written"
    );

    let renamed = as_git_user(&scratch, &repo, "Kim Park", &create);
    assert!(renamed.status.success(), "{renamed:?}");
    assert_eq!(names_written(&reopen()), 2);
    let undone = as_git_user(&scratch, &repo, "Kim Park", &["undo"]);
    assert!(undone.status.success(), "{undone:?}");
    assert!(text(&undone.stdout).contains("TM-3"), "{undone:?}");
    let undone = as_git_user(&scratch, &repo, "Kim Park", &["undo"]);
    assert!(
        text(&undone.stdout).contains("TM-2"),
        "a bare undo steps past the name to the write before: {undone:?}"
    );
    assert_eq!(
        reopen().name_of("kim@example.com").unwrap().as_deref(),
        Some("Kim Park")
    );
}

#[test]
fn under_tm_actor_or_as_an_agent_git_names_someone_else() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    let create = ["create", "--type", "task", "--title", "a"];
    let mut as_actor = jace(&scratch, &repo, &create);
    as_actor
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "user.name")
        .env("GIT_CONFIG_VALUE_0", "Kim Lee")
        .stdin(Stdio::null());
    assert!(as_actor.output().unwrap().status.success());
    let mut as_agent = jace(&scratch, &repo, &create);
    as_agent
        .env_remove("TM_ACTOR")
        .env("TM_AGENT", "claude")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "user.email")
        .env("GIT_CONFIG_VALUE_0", "kim@example.com")
        .env("GIT_CONFIG_KEY_1", "user.name")
        .env("GIT_CONFIG_VALUE_1", "Kim Lee")
        .stdin(Stdio::null());
    let out = as_agent.output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(names_written(&reopen(&scratch)), 0);
}

#[test]
fn only_a_command_that_wrote_records_a_name_and_one_tm_refuses_is_said() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    let made = as_git_user(
        &scratch,
        &repo,
        "Kim",
        &["create", "--type", "task", "--title", "a"],
    );
    assert!(made.status.success(), "{made:?}");
    for args in [&["undo", "--dry-run"][..], &["move", "TM-99", "done"][..]] {
        as_git_user(&scratch, &repo, "Kim Renamed", args);
    }
    assert_eq!(
        names_written(&reopen(&scratch)),
        1,
        "a dry run and a refusal write nothing"
    );

    let long = "k".repeat(201);
    let edited = as_git_user(
        &scratch,
        &repo,
        &long,
        &["edit", "TM-1", "--set", "title=b"],
    );
    assert!(edited.status.success(), "the write lands: {edited:?}");
    assert!(
        text(&edited.stderr).contains("cannot record your name"),
        "{edited:?}"
    );
    assert_eq!(names_written(&reopen(&scratch)), 1);
}

#[test]
fn a_blank_name_is_not_recorded_and_one_not_utf8_is_said() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    let create = ["create", "--type", "task", "--title", "a"];
    let blank = as_git_user(&scratch, &repo, " ", &create);
    assert!(blank.status.success(), "{blank:?}");
    assert!(!text(&blank.stderr).contains("your name"), "{blank:?}");
    let garbled = as_git_user(&scratch, &repo, OsStr::from_bytes(b"Kim \xffLee"), &create);
    assert!(garbled.status.success(), "{garbled:?}");
    assert!(
        text(&garbled.stderr).contains("user.name is not UTF-8"),
        "{garbled:?}"
    );
    assert_eq!(names_written(&reopen(&scratch)), 0);
}

#[test]
fn undoing_a_name_by_its_batch_says_names_are_not_undone() {
    let scratch = Scratch::new();
    let (repo, store, _) = initialized(&scratch);
    drop(store);
    let made = as_git_user(
        &scratch,
        &repo,
        "Kim",
        &["create", "--type", "task", "--title", "a"],
    );
    assert!(made.status.success(), "{made:?}");
    let batch: Vec<u8> = reopen(&scratch)
        .connection()
        .query_row(
            "SELECT b.batch FROM op_batches b JOIN ops o ON o.op_id = b.op_id
             WHERE o.kind = 'set_name'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let batch = uuid::Uuid::from_slice(&batch).unwrap().to_string();
    let undone = as_git_user(&scratch, &repo, "Kim", &["undo", &batch]);
    assert!(!undone.status.success(), "{undone:?}");
    assert!(
        text(&undone.stderr).contains("recorded a name, which undo does not revert"),
        "{undone:?}"
    );
}
