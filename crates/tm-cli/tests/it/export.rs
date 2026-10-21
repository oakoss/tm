use std::fs;
use std::path::Path;
use std::process::{Output, Stdio};

use serde_json::{Value, json};
use tm_core::Actor;
use tm_store::workspace::{PathProject, RepoFile};

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

#[test]
fn a_dated_export_lands_in_tms_home_once_per_change() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let dated = |scratch: &Scratch| {
        let out = tm(scratch, &repo, &["--json", "export", "--dated"]);
        assert!(out.status.success(), "{out:?}");
        doc(&out)["data"].clone()
    };

    let first = dated(&scratch);
    assert_eq!(first["written"], true, "{first}");
    let file = Path::new(first["file"].as_str().unwrap()).to_owned();
    assert!(
        file.starts_with(scratch.home().join("exports")),
        "under tm's home: {}",
        file.display()
    );
    let date = file.file_stem().unwrap().to_str().unwrap();
    assert!(
        jiff::civil::Date::strptime("%Y-%m-%d", date).is_ok(),
        "named by date: {date}"
    );
    let written = fs::read(&file).unwrap();
    assert_eq!(
        tm_store::read_export(&mut written.as_slice()).unwrap().1 as u64,
        first["ops"].as_u64().unwrap(),
        "readable as an export"
    );

    let again = dated(&scratch);
    assert_eq!(again["written"], false, "nothing changed: {again}");
    assert_eq!(again["file"], first["file"]);
    assert_eq!(fs::read(&file).unwrap(), written, "left as it was");

    let out = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "new"],
    );
    assert!(out.status.success(), "{out:?}");
    let changed = dated(&scratch);
    assert_eq!(changed["written"], true, "{changed}");
    assert_eq!(
        changed["file"], first["file"],
        "the same day's file, replaced"
    );
    assert!(changed["ops"].as_u64() > first["ops"].as_u64());
    let dir = file.parent().unwrap();
    let staged: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().starts_with(".tm-import-"))
        .collect();
    assert_eq!(
        staged,
        Vec::<std::ffi::OsString>::new(),
        "no staged copy left"
    );

    let out = tm(&scratch, &repo, &["export", "--dated"]);
    assert_eq!(
        text(&out.stdout),
        format!("Nothing changed since {}; wrote nothing\n", file.display())
    );

    // A file dated after today, from a clock that ran ahead, is passed over.
    fs::write(dir.join("2999-01-01.jsonl"), &written).unwrap();
    assert_eq!(dated(&scratch)["written"], false);

    // A backup whose header would not restore is replaced, though its body matches.
    let held = fs::read_to_string(&file).unwrap();
    let body = held.split_once('\n').unwrap().1;
    fs::write(&file, format!("{{not a header\n{body}")).unwrap();
    assert_eq!(dated(&scratch)["written"], true);
    let restored = fs::read(&file).unwrap();
    assert!(tm_store::read_export(&mut restored.as_slice()).is_ok());
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
        &["--json", "import", "--records", file, "--dry-run"],
    );
    assert_eq!(doc(&out)["error"], "unknown_workspace", "{out:?}");

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

    elsewhere(
        &scratch,
        &home,
        &repo,
        &["create", "--type", "task", "--title", "since"],
    );
    let out = elsewhere(&scratch, &home, &repo, &["--json", "import", file]);
    assert_eq!(doc(&out)["error"], "workspace_not_empty", "{out:?}");
    assert_eq!(doc(&out)["fix"], "tm import --records FILE");
}

/// The store crate's copy of this repo's `bd export`.
fn beads_export() -> String {
    format!(
        "{}/../tm-store/tests/fixtures/beads.jsonl",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn a_beads_export_lands_in_the_mapped_project_with_bd_readys_queue() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let file = beads_export();

    let out = tm(
        &scratch,
        &repo,
        &["--json", "import", "--from", "beads", &file, "--dry-run"],
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(doc(&out)["data"]["created"], 16);
    assert_eq!(
        text(&tm(&scratch, &repo, &["ready"]).stdout),
        "nothing is ready\n"
    );

    let out = tm(&scratch, &repo, &["import", "--from", "beads", &file]);
    assert!(out.status.success(), "{out:?}");
    let printed = text(&out.stdout);
    assert!(
        printed.starts_with("imported 16 records: 16 created, 0 updated, 0 unchanged"),
        "{printed}"
    );
    assert!(
        printed.contains(
            "not kept: dependency created_at (35), dependency created_by (35), started_at (7)"
        ),
        "{printed}"
    );

    let out = tm(&scratch, &repo, &["--json", "ready"]);
    let keys: Vec<Value> = doc(&out)["data"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["key"].clone())
        .collect();
    let mut keys: Vec<&str> = keys.iter().filter_map(Value::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["TM-1", "TM-10", "TM-11", "TM-12", "TM-13", "TM-14"]);

    let out = tm(
        &scratch,
        &repo,
        &["--json", "import", "--from", "beads", &file],
    );
    assert_eq!(doc(&out)["data"]["unchanged"], 16, "{out:?}");
}

#[test]
fn a_beads_import_files_into_the_project_its_directory_maps_to() {
    let scratch = Scratch::new();
    let (repo, mut store, _) = initialized(&scratch);
    store
        .create_project(&Actor::human("jace"), "TUI", "Terminal UI")
        .unwrap();
    let path = repo.join(".config/tm.toml");
    let mut file = RepoFile::parse(&fs::read_to_string(&path).unwrap()).unwrap();
    for (prefix, project) in [("crates/tui", "TUI"), ("crates/gone", "GONE")] {
        file.paths.push(PathProject {
            prefix: prefix.into(),
            project: project.into(),
        });
        fs::create_dir_all(repo.join(prefix)).unwrap();
    }
    fs::write(&path, file.render()).unwrap();
    let beads = scratch.0.join("beads.jsonl");
    fs::write(&beads, r#"{"id":"b-1","title":"one"}"#).unwrap();
    let beads = beads.to_str().unwrap();

    for dry_run in [true, false] {
        let mut args = vec!["--json", "import", "--from", "beads", beads];
        if dry_run {
            args.push("--dry-run");
        }
        let out = tm(&scratch, &repo.join("crates/gone"), &args);
        let refused = doc(&out);
        assert_eq!(
            (&refused["error"], &refused["reference"]),
            (&json!("not_found"), &json!("GONE")),
            "{refused}"
        );
    }

    let out = tm(
        &scratch,
        &repo.join("crates/tui"),
        &["import", "--from", "beads", beads],
    );
    assert!(out.status.success(), "{out:?}");
    let out = tm(&scratch, &repo, &["--json", "show", "TUI-1"]);
    assert_eq!(doc(&out)["data"]["fields"]["title"], "one", "{out:?}");
}

#[test]
fn what_a_beads_import_does_not_keep_prints_escaped() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let beads = scratch.0.join("beads.jsonl");
    let bead = json!({"id": "b-1", "title": "one", "x\u{1b}[31mevil": "y"});
    fs::write(&beads, bead.to_string()).unwrap();
    let out = tm(
        &scratch,
        &repo,
        &["import", "--from", "beads", beads.to_str().unwrap()],
    );
    let printed = text(&out.stdout);
    assert!(printed.contains("not kept: x"), "{printed}");
    assert!(!printed.contains('\u{1b}'), "{printed:?}");
}

#[test]
fn from_and_records_together_is_a_usage_error() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let out = tm(
        &scratch,
        &repo,
        &["import", "--records", "--from", "beads", &beads_export()],
    );
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

#[test]
fn a_records_dry_run_leaves_the_store_unopened_and_output_escaped() {
    let scratch = Scratch::new();
    let (repo, file) = exported(&scratch);
    let store = fs::read_dir(scratch.home().join("workspaces"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("store.db");
    rusqlite::Connection::open(&store)
        .unwrap()
        .execute_batch("PRAGMA user_version = 0")
        .unwrap();

    // A label tm never held, carrying a terminal escape, on an Issue of
    // this Workspace: a conflict the summary prints.
    let mut lines: Vec<Value> = text(&fs::read(&file).unwrap())
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let issue = lines
        .iter_mut()
        .find(|line| line["_type"] == "issue")
        .unwrap();
    issue["labels"] = serde_json::json!(["x\u{1b}[2J"]);
    let crafted = scratch.0.join("crafted.jsonl");
    let joined: Vec<String> = lines.iter().map(Value::to_string).collect();
    fs::write(&crafted, joined.join("\n")).unwrap();

    let out = tm(
        &scratch,
        &repo,
        &[
            "import",
            "--records",
            crafted.to_str().unwrap(),
            "--dry-run",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let printed = text(&out.stdout);
    assert!(printed.contains("conflict: tm:"), "{printed}");
    assert!(!printed.contains('\u{1b}'), "{printed:?}");
    let version: i64 = rusqlite::Connection::open(&store)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        version, 0,
        "the dry run opened the real store, which rebuilt it"
    );
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
    assert_eq!(doc(&out)["fix"], "tm import --records FILE");
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
        "line 2 of the file: the line is not JSON, at column 2"
    );
}

/// The refusals a restore meets name `--records`, which brings the Issues
/// into this repo's Workspace, once.
#[test]
fn records_bring_an_export_into_a_workspace_a_restore_refuses() {
    let scratch = Scratch::new();
    let (_, file) = exported(&scratch);
    let other = Scratch::new();
    let (repo, _store, _) = initialized(&other);
    let file = file.to_str().unwrap();

    let out = tm(
        &other,
        &repo,
        &["--json", "import", "--records", file, "--dry-run"],
    );
    assert_eq!(doc(&out)["data"]["created"], 2, "{out:?}");
    assert_eq!(doc(&out)["data"]["dry_run"], true);
    assert_eq!(
        doc(&out)["data"]["records"][0]["batches"],
        serde_json::json!([]),
        "a dry run's batches do not exist"
    );
    let listed = |out: &Output| doc(out)["data"]["issues"].as_array().unwrap().len();
    assert_eq!(
        listed(&tm(&other, &repo, &["--json", "list"])),
        0,
        "a dry run writes nothing"
    );

    let out = tm(&other, &repo, &["--json", "import", "--records", file]);
    let imported = doc(&out);
    assert_eq!(imported["data"]["created"], 2, "{out:?}");
    let batch = imported["data"]["records"][0]["batches"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let out = tm(&other, &repo, &["--json", "undo", &batch, "--dry-run"]);
    assert!(
        out.status.success(),
        "a named undo takes an import batch: {out:?}"
    );
    assert!(
        imported["warnings"][0]
            .as_str()
            .unwrap()
            .contains("not its history"),
        "{imported}"
    );
    let shown = doc(&tm(&other, &repo, &["--json", "show", "TM-1"]));
    assert_eq!(shown["data"]["fields"]["title"], "first", "{shown}");
    assert_eq!(shown["data"]["links"][0]["other"]["key"], "TM-2");

    let out = tm(&other, &repo, &["import", "--records", file]);
    assert!(
        text(&out.stdout).starts_with("imported 2 records: 0 created, 0 updated, 2 unchanged"),
        "{out:?}"
    );

    let out = tm(&other, &repo, &["--json", "import", file]);
    assert_eq!(doc(&out)["error"], "workspace_mismatch");
    let mut newer: Vec<Value> = text(&fs::read(file).unwrap())
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    newer[0]["op_format"] = serde_json::json!(99);
    let newer_file = other.0.join("newer.jsonl");
    let lines: Vec<String> = newer.iter().map(Value::to_string).collect();
    fs::write(&newer_file, lines.join("\n")).unwrap();
    let elsewhere = other.0.join("outside");
    fs::create_dir_all(&elsewhere).unwrap();
    let out = tm(
        &other,
        &elsewhere,
        &["--json", "import", newer_file.to_str().unwrap()],
    );
    assert_eq!(doc(&out)["error"], "export_format_changed", "{out:?}");
    assert_eq!(doc(&out)["fix"], "tm import --records FILE");
}

#[test]
fn a_refused_records_file_says_what_stays_and_prints_escaped() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let record = |source: &str, parent: Option<&str>| {
        let mut record = serde_json::json!({
            "_type": "issue", "source": source, "project": "TM", "type": "task",
            "title": "t", "status": "todo",
        });
        if let Some(parent) = parent {
            record["parent"] = serde_json::json!(parent);
        }
        record.to_string()
    };
    let write = |name: &str, lines: &[String]| {
        let path = scratch.0.join(name);
        fs::write(&path, lines.join("\n")).unwrap();
        path.to_str().unwrap().to_owned()
    };

    let missing = write(
        "missing.jsonl",
        &[record("b:1", None), record("b:2", Some("b:none"))],
    );
    let out = tm(
        &scratch,
        &repo,
        &["--json", "import", "--records", &missing, "--dry-run"],
    );
    assert_eq!(doc(&out)["written"], serde_json::json!([]), "{out:?}");
    assert!(
        doc(&out)["message"]
            .as_str()
            .unwrap()
            .contains("nothing was written"),
        "a dry run's copy is gone: {out:?}"
    );

    let escaped = "b:\u{1b}[2J";
    let twice = write(
        "twice.jsonl",
        &[record(escaped, None), record(escaped, None)],
    );
    let out = tm(&scratch, &repo, &["import", "--records", &twice]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(!text(&out.stderr).contains('\u{1b}'), "{out:?}");
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
