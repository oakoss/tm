use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use jiff::ToSpan as _;
use serde_json::Value;

use crate::support::{Scratch, initialized, tm, tm_command};

fn create(scratch: &Scratch, repo: &Path, title: &str) -> Value {
    let out = tm(
        scratch,
        repo,
        &["--json", "create", "--type", "task", "--title", title],
    );
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}

/// The Workspace's exports directory, and the names in it.
fn exports(scratch: &Scratch) -> (PathBuf, Vec<String>) {
    let workspaces: Vec<_> = fs::read_dir(scratch.home().join("workspaces"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(workspaces.len(), 1);
    let dir = scratch.home().join("exports").join(&workspaces[0]);
    let mut names: Vec<String> = fs::read_dir(&dir)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|name| name != ".lock")
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    (dir, names)
}

fn today() -> jiff::civil::Date {
    jiff::Zoned::now().date()
}

#[test]
fn a_days_first_write_backs_up_and_later_ones_leave_it() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let (_, names) = exports(&scratch);
    assert_eq!(names, Vec::<String>::new(), "tm init takes no backup");
    let refused = tm(&scratch, &repo, &["comment", "TM-404", "x"]);
    assert!(!refused.status.success(), "{refused:?}");
    let (_, names) = exports(&scratch);
    assert_eq!(names, Vec::<String>::new(), "a write of nothing takes none");

    let first = create(&scratch, &repo, "first");
    assert_eq!(first["warnings"], serde_json::json!([]), "{first}");
    let (dir, names) = exports(&scratch);
    assert_eq!(names, [format!("{}.jsonl", today())]);
    let file = dir.join(&names[0]);
    let backed_up = fs::read(&file).unwrap();

    create(&scratch, &repo, "second");
    let after = fs::read(&file).unwrap();
    assert_eq!(after, backed_up, "the day's backup stands");

    // The backup restores on another machine, holding the first write.
    let home = scratch.0.join("other-home");
    let out = tm_command()
        .args(["--json", "import"])
        .arg(&file)
        .current_dir(&scratch.0)
        .env("TM_HOME", &home)
        .env("TM_ACTOR", "jace")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let restored: Value = serde_json::from_slice(&out.stdout).unwrap();
    let ops = tm_store::read_export(&mut backed_up.as_slice()).unwrap().1;
    assert_eq!(restored["data"]["ops"], ops, "{restored}");
}

#[test]
fn a_backup_collects_old_ones_and_leaves_what_tm_did_not_write() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    create(&scratch, &repo, "first");
    let (dir, names) = exports(&scratch);
    let backup = fs::read(dir.join(&names[0])).unwrap();
    fs::remove_file(dir.join(&names[0])).unwrap();

    let yesterday = format!("{}.jsonl", today() - 1.day());
    for name in [
        "2001-01-01.jsonl",
        "2001-01-02.jsonl",
        "2999-01-01.jsonl",
        yesterday.as_str(),
        "2001-01-03.json",
        "notes.txt",
    ] {
        fs::write(dir.join(name), &backup).unwrap();
    }
    fs::create_dir(dir.join("2001-01-04.jsonl")).unwrap();
    let killed = dir.join(".tm-import-killed");
    fs::create_dir_all(&killed).unwrap();
    fs::write(killed.join("export.jsonl"), "partial").unwrap();
    let other = dir.parent().unwrap().join("another-workspace");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("2001-01-01.jsonl"), &backup).unwrap();

    let done = create(&scratch, &repo, "second");
    assert_eq!(done["warnings"], serde_json::json!([]), "{done}");
    let (_, names) = exports(&scratch);
    let mut expected = vec![
        "2001-01-03.json".to_owned(),
        "2001-01-04.jsonl".to_owned(),
        "2999-01-01.jsonl".to_owned(),
        yesterday,
        format!("{}.jsonl", today()),
        "notes.txt".to_owned(),
    ];
    expected.sort();
    assert_eq!(names, expected);
    let theirs = other.join("2001-01-01.jsonl");
    assert!(theirs.exists(), "another Workspace's");
}

#[test]
fn a_config_tm_cannot_read_backs_up_and_removes_nothing() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    create(&scratch, &repo, "first");
    let (dir, names) = exports(&scratch);
    fs::rename(dir.join(&names[0]), dir.join("2001-01-01.jsonl")).unwrap();
    fs::write(scratch.home().join("config.toml"), "[backups]\nkeep = 1\n").unwrap();
    fs::create_dir(dir.join(".tm-import-killed")).unwrap();

    let done = create(&scratch, &repo, "second");
    let warnings = done["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{done}");
    let warning = warnings[0].as_str().unwrap();
    assert!(
        warning.contains("config.toml") && warning.ends_with("; no old backup was removed"),
        "{warning}"
    );
    let (_, names) = exports(&scratch);
    assert_eq!(
        names,
        ["2001-01-01.jsonl".to_owned(), format!("{}.jsonl", today())],
        "a killed export's staging is removed whatever the policy"
    );
}

#[test]
fn a_dated_export_collects_old_backups() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    create(&scratch, &repo, "first");
    let (dir, names) = exports(&scratch);
    let backup = fs::read(dir.join(&names[0])).unwrap();
    fs::write(dir.join("2001-01-01.jsonl"), &backup).unwrap();
    fs::create_dir(dir.join(".tm-import-killed")).unwrap();

    let out = tm(&scratch, &repo, &["--json", "export", "--dated"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(exports(&scratch).1, [format!("{}.jsonl", today())]);

    // A config it cannot read keeps every backup, and says so.
    fs::write(dir.join("2001-01-01.jsonl"), &backup).unwrap();
    fs::write(scratch.home().join("config.toml"), "[backups\n").unwrap();
    let out = tm(&scratch, &repo, &["--json", "export", "--dated"]);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let warning = doc["warnings"][0].as_str().unwrap_or_default();
    assert!(warning.ends_with("; no old backup was removed"), "{doc}");
    assert!(dir.join("2001-01-01.jsonl").exists());

    // A failure names the path it failed on.
    let blocked = dir.parent().unwrap();
    fs::remove_dir_all(blocked).unwrap();
    fs::write(blocked, "in the way").unwrap();
    let out = tm(&scratch, &repo, &["--json", "export", "--dated"]);
    assert!(!out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"], "io", "{doc}");
    assert!(
        doc["path"]
            .as_str()
            .is_some_and(|path| Path::new(path).starts_with(blocked)),
        "{doc}"
    );
}

#[test]
fn collection_reports_what_it_could_not_remove_and_removes_the_rest() {
    use std::os::unix::fs::PermissionsExt as _;
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    create(&scratch, &repo, "first");
    let (dir, names) = exports(&scratch);
    let backup = fs::read(dir.join(&names[0])).unwrap();
    fs::remove_file(dir.join(&names[0])).unwrap();
    fs::write(dir.join("2001-01-01.jsonl"), &backup).unwrap();
    let stuck = dir.join(".tm-import-stuck");
    fs::create_dir_all(stuck.join("inner")).unwrap();
    fs::write(stuck.join("inner").join("export.jsonl"), "partial").unwrap();
    fs::set_permissions(stuck.join("inner"), fs::Permissions::from_mode(0o500)).unwrap();

    let done = create(&scratch, &repo, "second");
    fs::set_permissions(stuck.join("inner"), fs::Permissions::from_mode(0o700)).unwrap();
    let warnings = done["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{done}");
    let warning = warnings[0].as_str().unwrap();
    assert!(
        warning.starts_with("old staging was not removed: ")
            && warning.contains(".tm-import-stuck"),
        "{warning}"
    );
    assert!(
        !exports(&scratch).1.contains(&"2001-01-01.jsonl".to_owned()),
        "the rest is collected"
    );
}

#[test]
fn nothing_is_removed_through_a_linked_exports_directory() {
    for whole in [false, true] {
        let scratch = Scratch::new();
        let (repo, _store, _) = initialized(&scratch);
        create(&scratch, &repo, "first");
        let (dir, names) = exports(&scratch);
        fs::remove_file(dir.join(&names[0])).unwrap();
        fs::write(dir.join("2001-01-01.jsonl"), "theirs").unwrap();
        // Either the exports directory itself, or the Workspace's in it.
        let linked = if whole {
            dir.parent().unwrap().to_owned()
        } else {
            dir.clone()
        };
        let elsewhere = scratch.0.join("elsewhere");
        fs::rename(&linked, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &linked).unwrap();

        let done = create(&scratch, &repo, "second");
        let warnings = done["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1, "{done}");
        assert!(
            warnings[0].as_str().unwrap().ends_with(&format!(
                "{} is a symbolic link, and tm removes nothing through one",
                linked.display()
            )),
            "{done}"
        );
        assert!(dir.join("2001-01-01.jsonl").exists(), "whole: {whole}");
    }
}

#[test]
fn something_else_at_todays_name_warns_and_an_empty_file_is_replaced() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    create(&scratch, &repo, "first");
    let (dir, names) = exports(&scratch);
    let file = dir.join(&names[0]);

    fs::write(&file, "").unwrap();
    let done = create(&scratch, &repo, "second");
    assert_eq!(done["warnings"], serde_json::json!([]), "{done}");
    let replaced = fs::read(&file).unwrap();
    assert!(tm_store::read_export(&mut replaced.as_slice()).is_ok());

    fs::remove_file(&file).unwrap();
    fs::create_dir(&file).unwrap();
    let done = create(&scratch, &repo, "third");
    let warnings = done["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{done}");
    assert!(
        warnings[0].as_str().unwrap().ends_with(
            "is not a file tm wrote, so it was left alone and old backups were not removed"
        ),
        "{done}"
    );
    assert!(file.is_dir());
}

#[test]
fn a_config_can_turn_backups_off() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    fs::write(
        scratch.home().join("config.toml"),
        "[backups]\nenabled = false\n",
    )
    .unwrap();
    let done = create(&scratch, &repo, "first");
    assert_eq!(done["warnings"], serde_json::json!([]), "{done}");
    let (dir, _) = exports(&scratch);
    assert!(!dir.exists(), "nothing staged, so nothing written");

    // Backups, and a file named like staging, are not staging to clear.
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("2000-01-01.jsonl"), "kept").unwrap();
    fs::write(dir.join("2001-01-01.jsonl"), "kept").unwrap();
    fs::write(dir.join(".tm-import-file"), "not staging").unwrap();
    create(&scratch, &repo, "unlocked");
    assert!(!dir.join(".lock").exists(), "no lock taken");
    fs::remove_file(dir.join(".tm-import-file")).unwrap();

    // A killed export's staging still goes; backups stay, the oldest too.
    fs::create_dir_all(dir.join(".tm-import-killed")).unwrap();
    let done = create(&scratch, &repo, "second");
    assert_eq!(done["warnings"], serde_json::json!([]), "{done}");
    let kept = ["2000-01-01.jsonl", "2001-01-01.jsonl"];
    assert_eq!(exports(&scratch).1, kept);

    // Staging it cannot clear is a warning.
    fs::create_dir_all(dir.join(".tm-import-killed")).unwrap();
    fs::remove_file(dir.join(".lock")).unwrap();
    fs::create_dir(dir.join(".lock")).unwrap();
    let done = create(&scratch, &repo, "third");
    let warning = done["warnings"][0].as_str().unwrap_or_default();
    assert!(
        warning.starts_with("old staging was not removed: "),
        "{done}"
    );
}

#[test]
fn a_write_with_backups_off_does_not_wait_for_a_running_export() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    fs::write(
        scratch.home().join("config.toml"),
        "[backups]\nenabled = false\n",
    )
    .unwrap();
    let (dir, _) = exports(&scratch);
    fs::create_dir_all(dir.join(".tm-import-running")).unwrap();
    // As a running export holds it.
    let lock = fs::File::create(dir.join(".lock")).unwrap();
    lock.lock().unwrap();

    let mut child = tm_command()
        .args(["--json", "create", "--type", "task", "--title", "held"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let finished = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if started.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(lock);
    assert!(
        finished.is_some_and(|status| status.success()),
        "the write waited on the lock"
    );
    assert!(dir.join(".tm-import-running").exists(), "the export's own");
}

#[test]
fn a_first_dated_export_has_nothing_to_compare_and_no_warning() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let out = tm(&scratch, &repo, &["--json", "export", "--dated"]);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["written"], true, "{doc}");
    assert_eq!(doc["warnings"], serde_json::json!([]), "{doc}");
}

#[test]
fn a_dated_export_says_when_it_cannot_read_the_newest() {
    use std::os::unix::fs::PermissionsExt as _;
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    create(&scratch, &repo, "first");
    let (dir, names) = exports(&scratch);
    let file = dir.join(&names[0]);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();

    let out = tm(&scratch, &repo, &["--json", "export", "--dated"]);
    let _ = fs::set_permissions(&file, fs::Permissions::from_mode(0o600));
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["written"], true, "{doc}");
    let warning = doc["warnings"][0].as_str().unwrap_or_default();
    assert!(
        warning.starts_with(&file.display().to_string())
            && warning.ends_with("; wrote a new export rather than compare with it"),
        "{doc}"
    );
}

#[test]
fn a_backup_that_cannot_be_staged_names_the_exports_directory() {
    use std::os::unix::fs::PermissionsExt as _;
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let (dir, _) = exports(&scratch);
    fs::create_dir_all(&dir).unwrap();
    fs::File::create(dir.join(".lock")).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();

    let done = create(&scratch, &repo, "first");
    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    let warning = done["warnings"][0].as_str().unwrap_or_default();
    assert!(
        warning.starts_with(&format!(
            "today's backup was not written: {}: ",
            dir.display()
        )),
        "{done}"
    );
}

#[test]
fn a_backup_that_cannot_be_written_warns_and_the_write_stands() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    fs::write(scratch.home().join("exports"), "in the way").unwrap();
    fs::write(scratch.home().join("config.toml"), "[backups\n").unwrap();

    let done = create(&scratch, &repo, "first");
    let warnings: Vec<&str> = done["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning.as_str().unwrap())
        .collect();
    assert_eq!(warnings.len(), 2, "{done}");
    assert!(
        warnings[0].starts_with("today's backup was not written: "),
        "{done}"
    );
    assert!(warnings[1].contains("config.toml"), "{done}");
    let out = tm(&scratch, &repo, &["show", "TM-1"]);
    assert!(out.status.success(), "the write stands: {out:?}");
}
