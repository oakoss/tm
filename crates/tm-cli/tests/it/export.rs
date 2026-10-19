use std::fs;
use std::path::Path;
use std::process::{Output, Stdio};

use serde_json::Value;

use crate::support::{Scratch, initialized, text, tm, tm_command};

/// `tm` on another machine: the same repo, another `TM_HOME`.
fn elsewhere(scratch: &Scratch, home: &Path, dir: &Path, args: &[&str]) -> Output {
    tm_command()
        .args(args)
        .current_dir(dir)
        .env("TM_HOME", home)
        .env("TM_ACTOR", "jace")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn doc(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|err| panic!("{err}: {out:?}"))
}

/// A repo with two Issues, a Link, a comment and its retraction; and its
/// export, saved to a file.
fn exported(scratch: &Scratch) -> (std::path::PathBuf, std::path::PathBuf) {
    let (repo, _store, _) = initialized(scratch);
    for title in ["first", "second"] {
        let out = tm(
            scratch,
            &repo,
            &["create", "--type", "task", "--title", title],
        );
        assert!(out.status.success(), "{out:?}");
    }
    tm(scratch, &repo, &["link", "TM-1", "blocks", "TM-2"]);
    let out = tm(scratch, &repo, &["--json", "comment", "TM-1", "said"]);
    let batch = doc(&out)["data"]["batch"].as_str().unwrap().to_owned();
    tm(scratch, &repo, &["undo", &batch]);
    let out = tm(scratch, &repo, &["export"]);
    assert!(out.status.success(), "{out:?}");
    let file = scratch.0.join("backup.jsonl");
    fs::write(&file, &out.stdout).unwrap();
    (repo, file)
}

#[test]
fn a_fresh_clone_restores_its_workspace_from_an_export() {
    let scratch = Scratch::new();
    let (repo, file) = exported(&scratch);
    let home = scratch.0.join("other-home");
    let file = file.to_str().unwrap();

    let out = elsewhere(&scratch, &home, &repo, &["--json", "list"]);
    assert_eq!(doc(&out)["error"], "unknown_workspace", "{out:?}");
    assert_eq!(doc(&out)["fix"], "tm import FILE");

    let out = elsewhere(
        &scratch,
        &home,
        &repo,
        &["--json", "import", file, "--dry-run"],
    );
    assert_eq!(doc(&out)["data"]["created"], true, "{out:?}");
    assert!(!home.join("workspaces").exists(), "a dry run makes nothing");

    let out = elsewhere(&scratch, &home, &repo, &["--json", "import", file]);
    let restored = doc(&out);
    assert_eq!(restored["data"]["already_held"], false, "{out:?}");
    assert!(
        restored["warnings"][0]
            .as_str()
            .unwrap()
            .contains("stop using tm"),
        "{restored}"
    );
    for args in [["--json", "show", "TM-1"], ["--json", "list", "sort:key"]] {
        assert_eq!(
            doc(&elsewhere(&scratch, &home, &repo, &args)),
            doc(&tm(&scratch, &repo, &args)),
            "{args:?}"
        );
    }

    let out = elsewhere(&scratch, &home, &repo, &["export"]);
    let after_header =
        |bytes: &[u8]| -> Vec<String> { text(bytes).lines().skip(1).map(str::to_owned).collect() };
    let first = after_header(&fs::read(file).unwrap());
    let again = after_header(&out.stdout);
    assert_eq!(again, first, "the same log exports the same lines");

    let out = elsewhere(&scratch, &home, &repo, &["--json", "import", file]);
    assert_eq!(doc(&out)["data"]["already_held"], true, "{out:?}");
}

#[test]
fn a_repo_mapped_to_another_workspace_or_a_damaged_file_is_refused() {
    let scratch = Scratch::new();
    let (_, file) = exported(&scratch);
    let other = Scratch::new();
    let (repo, _store, _) = initialized(&other);
    let file = file.to_str().unwrap();

    let out = tm(&other, &repo, &["--json", "import", file]);
    assert_eq!(doc(&out)["error"], "workspace_mismatch", "{out:?}");
    assert_eq!(out.status.code(), Some(1));

    let damaged = other.0.join("damaged.jsonl");
    let mut lines: Vec<String> = text(&fs::read(file).unwrap())
        .lines()
        .map(str::to_owned)
        .collect();
    lines[1] = "{not json".into();
    fs::write(&damaged, lines.join("\n")).unwrap();
    let out = tm(
        &other,
        &repo,
        &["--json", "import", damaged.to_str().unwrap()],
    );
    assert_eq!(doc(&out)["error"], "invalid_export", "{out:?}");
    assert_eq!(doc(&out)["line"], 2);
    assert_eq!(
        doc(&out)["message"],
        "line 2 of the export: the line is not JSON, at column 2"
    );
}

#[test]
fn outside_a_repo_import_makes_the_workspace_and_rebuild_replays_it() {
    let scratch = Scratch::new();
    let (repo, file) = exported(&scratch);
    let home = scratch.0.join("other-home");
    let outside = scratch.0.join("elsewhere");
    fs::create_dir_all(&outside).unwrap();

    let out = elsewhere(
        &scratch,
        &home,
        &outside,
        &["--json", "import", file.to_str().unwrap()],
    );
    assert_eq!(doc(&out)["data"]["created"], true, "{out:?}");

    let out = elsewhere(&scratch, &home, &repo, &["--json", "rebuild"]);
    let rebuilt = doc(&out);
    assert_eq!(rebuilt["data"]["unapplied"], 0, "{out:?}");
    assert!(rebuilt["data"]["ops"].as_i64().unwrap() > 0);
}

#[test]
fn a_dry_run_leaves_an_existing_store_as_it_was() {
    let scratch = Scratch::new();
    let (repo, file) = exported(&scratch);
    let home = scratch.0.join("other-home");
    let file = file.to_str().unwrap();
    let out = elsewhere(&scratch, &home, &repo, &["--json", "import", file]);
    let workspace = doc(&out)["data"]["workspace"].as_str().unwrap().to_owned();
    let store = home.join("workspaces").join(&workspace).join("store.db");
    let version = || -> i64 {
        rusqlite::Connection::open(&store)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    };
    rusqlite::Connection::open(&store)
        .unwrap()
        .execute_batch("PRAGMA user_version = 0")
        .unwrap();
    let data = || fs::read(&store).unwrap();
    let before = data();

    let out = elsewhere(
        &scratch,
        &home,
        &repo,
        &["--json", "import", file, "--dry-run"],
    );
    assert_eq!(doc(&out)["data"]["already_held"], true, "{out:?}");
    assert!(data() == before, "the store's data is not written");
    assert_eq!(
        version(),
        0,
        "opening the copy, not the store, ran the rebuild"
    );
    let leftovers: Vec<_> = fs::read_dir(home.join("workspaces"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, [std::ffi::OsString::from(&workspace)]);
}

#[test]
fn a_workspace_directory_another_process_made_is_left_alone() {
    let scratch = Scratch::new();
    let (repo, file) = exported(&scratch);
    let home = scratch.0.join("other-home");
    let first = elsewhere(
        &scratch,
        &home,
        &repo,
        &["--json", "import", file.to_str().unwrap(), "--dry-run"],
    );
    let workspace = doc(&first)["data"]["workspace"]
        .as_str()
        .unwrap()
        .to_owned();
    let claimed = home.join("workspaces").join(&workspace);
    fs::create_dir_all(&claimed).unwrap();

    let out = elsewhere(
        &scratch,
        &home,
        &repo,
        &["--json", "import", file.to_str().unwrap()],
    );
    assert_eq!(doc(&out)["error"], "incomplete_workspace", "{out:?}");
    let out = elsewhere(
        &scratch,
        &home,
        &repo,
        &["--json", "import", file.to_str().unwrap(), "--dry-run"],
    );
    assert_eq!(doc(&out)["error"], "incomplete_workspace", "{out:?}");
    assert_eq!(
        fs::read_dir(&claimed).unwrap().count(),
        0,
        "nothing moved in"
    );
}

#[test]
fn an_export_that_cannot_be_written_fails() {
    let scratch = Scratch::new();
    let (repo, _) = exported(&scratch);
    // More than the 8 KiB write buffer, so the failure meets a write, not
    // only the final flush.
    for n in 0..12 {
        let title = format!("filler {n}");
        tm(
            &scratch,
            &repo,
            &["create", "--type", "task", "--title", &title],
        );
    }
    let out = tm_command()
        .args(["export"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .stdout(fs::File::open("/dev/null").unwrap())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(text(&out.stderr).contains("stdout"), "{out:?}");
}

#[test]
fn export_is_jsonl_in_either_mode() {
    let scratch = Scratch::new();
    let (repo, _) = exported(&scratch);
    let out = tm(&scratch, &repo, &["--json", "export"]);
    assert!(out.status.success(), "{out:?}");
    let first: Value = serde_json::from_str(text(&out.stdout).lines().next().unwrap()).unwrap();
    assert_eq!(first["_type"], "header");
    assert_eq!(text(&out.stderr), "");
}
