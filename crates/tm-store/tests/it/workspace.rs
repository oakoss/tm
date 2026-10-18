use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use tm_store::StoreError;
use tm_store::workspace::{
    self, PathProject, RepoFile, Workspace, WorkspaceError, discover, repo_root,
};
use uuid::Uuid;

use crate::support::TempDir;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn repo_file(workspace: Uuid, project: &str) -> String {
    RepoFile {
        workspace,
        project: project.into(),
        paths: Vec::new(),
    }
    .render()
}

/// A repo at `dir` mapped to `project`.
fn repo(dir: &Path, project: &str) {
    fs::create_dir_all(dir.join(".git")).unwrap();
    write(&dir.join(".tm.toml"), &repo_file(Uuid::now_v7(), project));
}

fn store_error(err: &WorkspaceError) -> Option<&StoreError> {
    match err {
        WorkspaceError::OpenStore { source, .. } => Some(source),
        _ => None,
    }
}

fn found_project(start: &Path) -> String {
    discover(start).unwrap().unwrap().file.project
}

fn with_paths(prefixes: &[&str]) -> String {
    let mut text = format!("workspace = \"{}\"\nproject = \"TM\"\n", Uuid::now_v7());
    for prefix in prefixes {
        let _ = write!(
            text,
            "\n[[paths]]\nprefix = \"{prefix}\"\nproject = \"SUB\"\n"
        );
    }
    text
}

#[test]
fn tm_home_overrides_the_home_directory_and_must_be_absolute() {
    let user = Some(PathBuf::from("/Users/someone"));
    assert_eq!(
        workspace::home(None, user.clone()).unwrap(),
        Path::new("/Users/someone/.tm")
    );
    assert_eq!(
        workspace::home(Some(OsString::new()), user.clone()).unwrap(),
        Path::new("/Users/someone/.tm")
    );
    assert_eq!(
        workspace::home(Some("/srv/tm".into()), user.clone()).unwrap(),
        Path::new("/srv/tm")
    );
    for (tm_home, user_home) in [
        (Some("tm".into()), user),
        (None, Some(PathBuf::from("someone"))),
    ] {
        let err = workspace::home(tm_home, user_home).unwrap_err();
        assert!(matches!(err, WorkspaceError::RelativeHome { .. }), "{err}");
    }
    for user_home in [None, Some(PathBuf::new())] {
        let err = workspace::home(None, user_home).unwrap_err();
        assert!(matches!(err, WorkspaceError::NoHome), "{err}");
    }
}

#[test]
fn a_repo_file_reads_back_what_it_renders() {
    let file = RepoFile {
        workspace: Uuid::now_v7(),
        project: "TM".into(),
        paths: vec![PathProject {
            prefix: "crates/tm-tui".into(),
            project: "TUI".into(),
        }],
    };
    assert_eq!(RepoFile::parse(&file.render()).unwrap(), file);
    let plain = repo_file(file.workspace, "TM");
    assert_eq!(
        plain,
        format!("workspace = \"{}\"\nproject = \"TM\"\n", file.workspace)
    );
}

#[test]
fn a_repo_file_refuses_fields_it_does_not_know() {
    let text = format!(
        "workspace = \"{}\"\nproject = \"TM\"\nserver = \"https://tm.example\"\n",
        Uuid::now_v7()
    );
    let err = RepoFile::parse(&text).unwrap_err();
    assert!(err.contains("server"), "{err}");
    assert!(err.contains("line 3"), "the error says where: {err}");
}

#[test]
fn a_prefix_is_a_plain_relative_path_mapped_once() {
    assert!(RepoFile::parse(&with_paths(&["crates/tm-tui", "crates/"])).is_ok());
    for bad in ["", "./crates", "/crates", "crates/../docs", ".."] {
        let err = RepoFile::parse(&with_paths(&[bad])).unwrap_err();
        assert!(err.contains("relative path"), "{bad:?}: {err}");
    }
    let err = RepoFile::parse(&with_paths(&["crates", "crates/"])).unwrap_err();
    assert!(err.contains("twice"), "{err}");
}

#[test]
fn a_repo_file_reads_project_keys_in_any_case() {
    let id = Uuid::now_v7();
    let file = RepoFile::parse(&format!("workspace = \"{id}\"\nproject = \"tm\"\n")).unwrap();
    assert_eq!(file.project, "TM");
    for bad in ["", "T M", "1TM", "T"] {
        let text = format!("workspace = \"{id}\"\nproject = \"{bad}\"\n");
        let err = RepoFile::parse(&text).unwrap_err();
        assert!(err.contains("not a Project key"), "{bad:?}: {err}");
    }
    let err = RepoFile::parse(&with_paths(&["crates"]).replace("\"SUB\"", "\"s b\"")).unwrap_err();
    assert!(err.contains("not a Project key"), "{err}");
}

#[test]
fn a_path_prefix_matches_whole_segments_and_the_longest_wins() {
    let file = RepoFile {
        workspace: Uuid::now_v7(),
        project: "TM".into(),
        paths: vec![
            PathProject {
                prefix: "crates".into(),
                project: "CRATES".into(),
            },
            PathProject {
                prefix: "crates/tm-tui".into(),
                project: "TUI".into(),
            },
        ],
    };
    assert_eq!(file.project_for(Path::new("")), "TM");
    assert_eq!(file.project_for(Path::new("docs")), "TM");
    assert_eq!(file.project_for(Path::new("crates/tm-store")), "CRATES");
    assert_eq!(file.project_for(Path::new("crates/tm-tui/src")), "TUI");
    assert_eq!(file.project_for(Path::new("crates/tm-tui2")), "CRATES");
    assert_eq!(file.project_for(Path::new("cratesx")), "TM");
}

#[test]
fn discovery_stops_at_the_repo_root() {
    let dir = TempDir::new();
    let outer = dir.path().to_path_buf();
    write(
        &outer.join(".config/tm.toml"),
        &repo_file(Uuid::now_v7(), "OUT"),
    );
    let repo = outer.join("repo");
    let deep = repo.join("src/deep");
    fs::create_dir_all(&deep).unwrap();
    fs::create_dir_all(repo.join(".git")).unwrap();
    assert_eq!(repo_root(&deep).unwrap(), Some(repo.clone()));
    assert_eq!(discover(&deep).unwrap(), None);
}

#[test]
fn discovery_follows_the_real_path_not_the_spelling() {
    let dir = TempDir::new();
    let outer = dir.path().join("outer");
    let other = dir.path().join("other");
    repo(&outer, "OUTER");
    repo(&other, "OTHER");
    fs::create_dir_all(other.join("src")).unwrap();
    symlink(other.join("src"), outer.join("link")).unwrap();
    assert_eq!(found_project(&outer.join("link")), "OTHER");
}

const CHILD_RELATIVE: &str = "TM_STORE_TEST_WORKSPACE_RELATIVE";

/// The working directory belongs to the whole process, so a child test
/// process starts from `repo/src/deep` and leaves a marker that it ran.
#[test]
fn a_relative_start_walks_the_real_directories() {
    if let Ok(marker) = std::env::var(CHILD_RELATIVE) {
        assert_eq!(found_project(Path::new(".")), "REL");
        fs::write(marker, "ran").unwrap();
        return;
    }
    let dir = TempDir::new();
    let repo_dir = dir.path().join("repo");
    repo(&repo_dir, "REL");
    let deep = repo_dir.join("src/deep");
    fs::create_dir_all(&deep).unwrap();
    let marker = dir.path().join("ran");
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "workspace::a_relative_start_walks_the_real_directories",
        ])
        .args(["--test-threads", "1", "--quiet"])
        .current_dir(&deep)
        .env(CHILD_RELATIVE, &marker)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(marker.exists(), "the child test did not run");
}

#[test]
fn a_file_named_like_a_directory_on_the_way_is_passed_over() {
    let dir = TempDir::new();
    let repo_dir = dir.path().join("repo");
    repo(&repo_dir, "TM");
    let pkg = repo_dir.join("pkg");
    write(&pkg.join(".config"), "not a directory\n");
    assert_eq!(found_project(&pkg), "TM");
}

#[test]
fn discovery_finds_the_nearest_file_and_a_git_file_marks_a_root() {
    let dir = TempDir::new();
    let repo = dir.path().join("worktree");
    write(&repo.join(".git"), "gitdir: /elsewhere/.git/worktrees/wt\n");
    let id = Uuid::now_v7();
    write(&repo.join(".tm.toml"), &repo_file(id, "TM"));
    let sub = repo.join("crates/tm-tui");
    fs::create_dir_all(&sub).unwrap();
    assert_eq!(repo_root(&sub).unwrap(), Some(repo.clone()));
    let found = discover(&sub).unwrap().unwrap();
    assert_eq!(found.path, repo.join(".tm.toml"));
    assert_eq!(found.dir, repo);
    assert_eq!(found.file.workspace, id);
}

#[test]
fn both_repo_files_in_one_directory_are_refused() {
    let dir = TempDir::new();
    let repo = dir.path().to_path_buf();
    fs::create_dir_all(repo.join(".git")).unwrap();
    write(&repo.join(".tm.toml"), &repo_file(Uuid::now_v7(), "TM"));
    write(
        &repo.join(".config/tm.toml"),
        &repo_file(Uuid::now_v7(), "TM"),
    );
    let err = discover(&repo).unwrap_err();
    assert!(matches!(err, WorkspaceError::TwoRepoFiles { .. }), "{err}");
}

#[test]
fn a_repo_file_that_cannot_be_checked_is_an_error_not_skipped() {
    let dir = TempDir::new();
    let outer = dir.path().join("outer");
    repo(&outer, "OUTER");
    let inner = outer.join("inner");
    write(
        &inner.join(".config/tm.toml"),
        &repo_file(Uuid::now_v7(), "INNER"),
    );
    let config = inner.join(".config");
    fs::set_permissions(&config, fs::Permissions::from_mode(0o000)).unwrap();
    let result = discover(&inner);
    fs::set_permissions(&config, fs::Permissions::from_mode(0o755)).unwrap();
    let err = result.unwrap_err();
    assert!(matches!(err, WorkspaceError::Io { .. }), "{err}");
}

#[test]
fn a_repo_file_that_is_not_a_file_is_refused() {
    let dir = TempDir::new();
    let outer = dir.path().join("outer");
    repo(&outer, "OUTER");
    let dangling = outer.join("dangling");
    fs::create_dir_all(&dangling).unwrap();
    symlink(dir.path().join("nowhere.toml"), dangling.join(".tm.toml")).unwrap();
    match discover(&dangling).unwrap_err() {
        WorkspaceError::InvalidRepoFile { reason, .. } => {
            assert!(reason.contains("missing file"), "{reason}");
        }
        other => panic!("{other}"),
    }
    let folder = outer.join("folder");
    fs::create_dir_all(folder.join(".tm.toml")).unwrap();
    let err = discover(&folder).unwrap_err();
    assert!(
        matches!(err, WorkspaceError::InvalidRepoFile { .. }),
        "{err}"
    );
}

#[test]
fn a_malformed_repo_file_names_itself() {
    let dir = TempDir::new();
    let repo = dir.path().to_path_buf();
    fs::create_dir_all(repo.join(".git")).unwrap();
    write(&repo.join(".config/tm.toml"), "project = \"TM\"\n");
    match discover(&repo).unwrap_err() {
        WorkspaceError::InvalidRepoFile { path, reason } => {
            assert_eq!(path, repo.join(".config/tm.toml"));
            assert!(reason.contains("workspace"), "{reason}");
        }
        other => panic!("{other}"),
    }
}

#[test]
fn a_workspace_this_machine_lacks_is_a_structured_error() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let repo = dir.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let id = Uuid::now_v7();
    write(&repo.join(".config/tm.toml"), &repo_file(id, "TM"));
    let found = discover(&repo).unwrap().unwrap();
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("a missing Workspace opened");
    };
    match err {
        WorkspaceError::UnknownWorkspace {
            id: missing,
            file,
            dir,
        } => {
            assert_eq!(missing, id);
            assert_eq!(file, repo.join(".config/tm.toml"));
            assert_eq!(dir, Workspace::at(&home, id).dir);
        }
        other => panic!("{other}"),
    }
    assert!(!home.exists(), "resolving creates nothing");
    let dir_only = Workspace::at(&home, id).dir;
    fs::create_dir_all(&dir_only).unwrap();
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("a Workspace without a store opened");
    };
    assert!(
        matches!(err, WorkspaceError::IncompleteWorkspace { .. }),
        "{err}"
    );
    fs::create_dir(dir_only.join("store.db")).unwrap();
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("a directory opened as a store");
    };
    assert!(matches!(err, WorkspaceError::Io { .. }), "{err}");
    fs::remove_dir_all(&dir_only).unwrap();
    let (created, _) = Workspace::create(&home, id).unwrap();
    let (opened, store) = workspace::workspace_for(&home, &found).unwrap();
    assert_eq!(opened, created);
    assert_eq!(store.workspace_id().unwrap(), Some(id));
}

/// A repo mapped to a Workspace directory whose store.db `store` puts in place.
fn workspace_with_store(dir: &Path, home: &Path, store: impl FnOnce(&Path)) -> workspace::Found {
    let repo_dir = dir.join("repo");
    fs::create_dir_all(repo_dir.join(".git")).unwrap();
    let id = Uuid::now_v7();
    write(&repo_dir.join(".tm.toml"), &repo_file(id, "TM"));
    let workspace = Workspace::at(home, id);
    fs::create_dir_all(&workspace.dir).unwrap();
    store(&workspace.store_path());
    discover(&repo_dir).unwrap().unwrap()
}

#[test]
fn opening_never_creates_or_adopts_a_database() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let target = dir.path().join("elsewhere.db");
    let found = workspace_with_store(dir.path(), &home, |store| {
        symlink(&target, store).unwrap();
    });
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("a dangling store link opened");
    };
    assert!(err.to_string().contains("missing file"), "{err}");
    assert!(!target.exists(), "nothing was created at the link's target");

    let dir = TempDir::new();
    let found = workspace_with_store(dir.path(), &home, |store| {
        fs::write(store, b"").unwrap();
    });
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("an empty store opened");
    };
    assert!(
        matches!(err, WorkspaceError::IncompleteWorkspace { .. }),
        "{err}"
    );

    let dir = TempDir::new();
    let foreign = dir.path().join("foreign.db");
    let found = workspace_with_store(dir.path(), &home, |store| {
        let conn = rusqlite::Connection::open(&foreign).unwrap();
        conn.execute_batch("CREATE TABLE t (x)").unwrap();
        drop(conn);
        symlink(&foreign, store).unwrap();
    });
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("another program's database opened");
    };
    assert!(
        matches!(err, WorkspaceError::IncompleteWorkspace { .. }),
        "{err}"
    );
    let conn = rusqlite::Connection::open(&foreign).unwrap();
    let tables: i64 = conn
        .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
        .unwrap();
    assert_eq!(tables, 1, "the foreign database is untouched");
}

/// A tm store of Workspace `id` at `path`, left the way an older tm would:
/// rollback journal and an unbuilt projection.
fn old_style_store(path: &Path, id: Uuid, format: i64) {
    let mut store = tm_store::Store::open(path).unwrap();
    store.set_workspace_id(id).unwrap();
    store
        .connection()
        .execute("UPDATE meta SET v = ?1 WHERE k = 'format'", [format])
        .unwrap();
    store
        .connection()
        .execute_batch("PRAGMA journal_mode = DELETE; PRAGMA user_version = 0;")
        .unwrap();
}

#[test]
fn a_store_that_is_refused_is_never_written() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let found = workspace_with_store(dir.path(), &home, |store| {
        old_style_store(store, Uuid::now_v7(), 1);
    });
    let store_path = Workspace::at(&home, found.file.workspace).store_path();
    let before = fs::read(&store_path).unwrap();
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("another Workspace's store opened");
    };
    assert!(
        matches!(
            store_error(&err),
            Some(StoreError::WorkspaceMismatch { .. })
        ),
        "{err}"
    );
    assert_eq!(fs::read(&store_path).unwrap(), before);

    let dir = TempDir::new();
    let found = workspace_with_store(dir.path(), &home, |store| {
        let own = store.parent().unwrap().file_name().unwrap();
        old_style_store(store, own.to_str().unwrap().parse().unwrap(), 999);
    });
    let store_path = Workspace::at(&home, found.file.workspace).store_path();
    let before = fs::read(&store_path).unwrap();
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("a newer store opened");
    };
    assert!(
        matches!(store_error(&err), Some(StoreError::NewerStore { .. })),
        "{err}"
    );
    assert_eq!(fs::read(&store_path).unwrap(), before);
}

#[test]
fn a_format_this_tm_cannot_read_is_refused_unwritten() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let found = workspace_with_store(dir.path(), &home, |store| {
        let own = store.parent().unwrap().file_name().unwrap();
        old_style_store(store, own.to_str().unwrap().parse().unwrap(), 0);
    });
    let store_path = Workspace::at(&home, found.file.workspace).store_path();
    let before = fs::read(&store_path).unwrap();
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("a format-0 store opened");
    };
    assert!(
        matches!(store_error(&err), Some(StoreError::Corrupt(_))),
        "{err}"
    );
    assert_eq!(fs::read(&store_path).unwrap(), before);

    let dir = TempDir::new();
    let found = workspace_with_store(dir.path(), &home, |store| {
        let conn = rusqlite::Connection::open(store).unwrap();
        conn.execute_batch("CREATE TABLE meta (k, v); INSERT INTO meta VALUES ('format', 0);")
            .unwrap();
    });
    let store_path = Workspace::at(&home, found.file.workspace).store_path();
    let before = fs::read(&store_path).unwrap();
    assert!(workspace::workspace_for(&home, &found).is_err());
    assert_eq!(fs::read(&store_path).unwrap(), before);

    for other in [
        "CREATE TABLE app_data (x)",
        "CREATE TABLE ops (x)",
        "CREATE TABLE ops (op_id, op_hash, hlc, payload)",
        "CREATE TABLE t (seq, op_id, op_hash, v, kind, replica_id, actor, project, issue_id,
           field, elem, hlc, base_seq, utc_ms, offset_min, payload, server_seq,
           server_received_ms, assigned_key);
         CREATE VIEW ops AS SELECT * FROM t",
        "CREATE VIRTUAL TABLE ops USING fts5(seq, op_id, op_hash, v, kind, replica_id, actor,
           project, issue_id, field, elem, hlc, base_seq, utc_ms, offset_min, payload,
           server_seq, server_received_ms, assigned_key)",
        "CREATE TABLE ops (seq, op_id, op_hash, v, kind, replica_id, actor, project, issue_id,
           field, elem, hlc, base_seq, utc_ms, offset_min, payload, server_seq,
           server_received_ms, assigned_key, extra)",
    ] {
        let dir = TempDir::new();
        let found = workspace_with_store(dir.path(), &home, |store| {
            let conn = rusqlite::Connection::open(store).unwrap();
            conn.execute_batch(&format!(
                "CREATE TABLE meta (k TEXT PRIMARY KEY, v);
                 INSERT INTO meta VALUES ('format', 1);
                 {other};"
            ))
            .unwrap();
        });
        let store_path = Workspace::at(&home, found.file.workspace).store_path();
        let before = fs::read(&store_path).unwrap();
        let Err(err) = workspace::workspace_for(&home, &found) else {
            panic!("another program's tables with {other} were adopted");
        };
        assert!(err.to_string().contains("operation log"), "{other}: {err}");
        assert_eq!(fs::read(&store_path).unwrap(), before, "{other}");
    }
}

#[test]
fn a_store_whose_log_lost_its_guard_is_refused() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let found = workspace_with_store(dir.path(), &home, |store| {
        let own = store.parent().unwrap().file_name().unwrap();
        old_style_store(store, own.to_str().unwrap().parse().unwrap(), 1);
        rusqlite::Connection::open(store)
            .unwrap()
            .execute_batch("DROP TRIGGER ops_append_only")
            .unwrap();
    });
    let store_path = Workspace::at(&home, found.file.workspace).store_path();
    let before = fs::read(&store_path).unwrap();
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("a store without its append-only guard opened");
    };
    assert!(err.to_string().contains("operation log"), "{err}");
    assert_eq!(fs::read(&store_path).unwrap(), before);
}

#[test]
fn a_tm_store_in_a_read_only_directory_says_so() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let id = Uuid::now_v7();
    drop(Workspace::create(&home, id).unwrap());
    let repo_dir = dir.path().join("repo");
    fs::create_dir_all(repo_dir.join(".git")).unwrap();
    write(&repo_dir.join(".tm.toml"), &repo_file(id, "TM"));
    let found = discover(&repo_dir).unwrap().unwrap();
    let workspace_dir = Workspace::at(&home, id).dir;
    let mode =
        |bits| fs::set_permissions(&workspace_dir, fs::Permissions::from_mode(bits)).unwrap();
    mode(0o555);
    let result = workspace::workspace_for(&home, &found);
    mode(0o755);
    let Err(err) = result else {
        panic!("a store in a read-only directory opened");
    };
    assert!(
        matches!(store_error(&err), Some(StoreError::ReadOnly)),
        "{err}"
    );
}

#[test]
fn a_store_that_cannot_be_used_is_named_not_called_missing() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let found = workspace_with_store(dir.path(), &home, |store| {
        let own = store.parent().unwrap().file_name().unwrap();
        old_style_store(store, own.to_str().unwrap().parse().unwrap(), 1);
    });
    let store_path = Workspace::at(&home, found.file.workspace).store_path();
    let mode = |bits| fs::set_permissions(&store_path, fs::Permissions::from_mode(bits)).unwrap();

    mode(0o000);
    let result = workspace::workspace_for(&home, &found);
    mode(0o644);
    let Err(err) = result else {
        panic!("an unreadable store opened");
    };
    assert!(matches!(err, WorkspaceError::OpenStore { .. }), "{err}");
    assert!(err.to_string().contains("store.db"), "{err}");

    mode(0o444);
    let result = workspace::workspace_for(&home, &found);
    mode(0o644);
    let Err(err) = result else {
        panic!("a read-only store opened");
    };
    assert!(
        matches!(store_error(&err), Some(StoreError::ReadOnly)),
        "{err}"
    );

    let workspace_dir = store_path.parent().unwrap();
    let dir_mode =
        |bits| fs::set_permissions(workspace_dir, fs::Permissions::from_mode(bits)).unwrap();
    dir_mode(0o555);
    let result = workspace::workspace_for(&home, &found);
    dir_mode(0o755);
    let Err(err) = result else {
        panic!("a store in a read-only directory opened");
    };
    assert!(
        matches!(store_error(&err), Some(StoreError::ReadOnly)),
        "{err}"
    );

    rusqlite::Connection::open(&store_path)
        .unwrap()
        .execute(
            "UPDATE meta SET v = 'not a uuid' WHERE k = 'workspace_id'",
            [],
        )
        .unwrap();
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("a malformed Workspace id opened");
    };
    assert!(
        matches!(store_error(&err), Some(StoreError::Corrupt(_))),
        "{err}"
    );
}

#[test]
fn a_damaged_store_reports_its_damage() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let found = workspace_with_store(dir.path(), &home, |store| {
        let own = store.parent().unwrap().file_name().unwrap();
        old_style_store(store, own.to_str().unwrap().parse().unwrap(), 1);
        let mut bytes = fs::read(store).unwrap();
        bytes[100..400].fill(0xA5);
        fs::write(store, bytes).unwrap();
    });
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("a damaged store opened");
    };
    assert!(matches!(err, WorkspaceError::OpenStore { .. }), "{err}");
}

#[test]
fn a_store_that_fails_to_open_is_not_optimized() {
    let dir = TempDir::new();
    let path = dir.path().join("format0.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE meta (k TEXT PRIMARY KEY, v ANY);
             INSERT INTO meta VALUES ('format', 0);
             CREATE TABLE big (x); CREATE INDEX big_x ON big (x);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000)
             INSERT INTO big SELECT i FROM n;",
        )
        .unwrap();
    assert!(tm_store::Store::open(&path).is_err());
    let stats: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name LIKE 'sqlite_stat%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stats, 0);
}

#[test]
fn a_foreign_wal_database_keeps_its_log() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let source = dir.path().join("foreign.db");
    let found = workspace_with_store(dir.path(), &home, |store| {
        let conn = rusqlite::Connection::open(&source).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;
             CREATE TABLE t (x); INSERT INTO t VALUES (1);",
        )
        .unwrap();
        // Copied while the writer is open, so the log is not yet folded in.
        fs::copy(&source, store).unwrap();
        fs::copy(
            source.with_extension("db-wal"),
            store.with_extension("db-wal"),
        )
        .unwrap();
        drop(conn);
    });
    let store_path = Workspace::at(&home, found.file.workspace).store_path();
    let before = fs::read(&store_path).unwrap();
    let wal = store_path.with_extension("db-wal");
    let wal_before = fs::read(&wal).unwrap();
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("another program's database opened");
    };
    assert!(
        matches!(err, WorkspaceError::IncompleteWorkspace { .. }),
        "{err}"
    );
    assert_eq!(fs::read(&store_path).unwrap(), before);
    assert_eq!(fs::read(&wal).unwrap(), wal_before);
}

#[test]
fn a_file_that_is_not_sqlite_is_an_incomplete_workspace() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let found = workspace_with_store(dir.path(), &home, |store| {
        fs::write(store, "not a database at all, just some text\n".repeat(200)).unwrap();
    });
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("a text file opened as a store");
    };
    assert!(
        matches!(err, WorkspaceError::IncompleteWorkspace { .. }),
        "{err}"
    );
}

#[test]
fn a_file_in_the_way_of_a_workspace_is_named() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let repo_dir = dir.path().join("repo");
    fs::create_dir_all(repo_dir.join(".git")).unwrap();
    let id = Uuid::now_v7();
    write(&repo_dir.join(".tm.toml"), &repo_file(id, "TM"));
    let found = discover(&repo_dir).unwrap().unwrap();
    write(&Workspace::at(&home, id).dir, "a file\n");
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("a file opened as a Workspace");
    };
    assert!(matches!(err, WorkspaceError::Io { .. }), "{err}");
    fs::remove_dir_all(&home).unwrap();
    write(&home.join("workspaces"), "a file\n");
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("a Workspace under a file opened");
    };
    assert!(matches!(err, WorkspaceError::Io { .. }), "{err}");
    fs::remove_dir_all(&home).unwrap();
    fs::create_dir_all(home.join("workspaces")).unwrap();
    symlink(dir.path().join("gone"), Workspace::at(&home, id).dir).unwrap();
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("a dangling Workspace link opened");
    };
    assert!(err.to_string().contains("missing directory"), "{err}");
}

#[test]
fn opening_finishes_an_interrupted_create_and_refuses_a_copied_store() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let repo_dir = dir.path().join("repo");
    fs::create_dir_all(repo_dir.join(".git")).unwrap();
    let id = Uuid::now_v7();
    write(&repo_dir.join(".tm.toml"), &repo_file(id, "TM"));
    let found = discover(&repo_dir).unwrap().unwrap();
    let workspace = Workspace::at(&home, id);
    fs::create_dir_all(&workspace.dir).unwrap();
    drop(tm_store::Store::open(&workspace.store_path()).unwrap());
    let (_, store) = workspace::workspace_for(&home, &found).unwrap();
    assert_eq!(store.workspace_id().unwrap(), Some(id));
    drop(store);
    fs::remove_file(workspace.store_path()).unwrap();
    let (_, copied) = Workspace::create(&dir.path().join("elsewhere"), Uuid::now_v7()).unwrap();
    copied
        .connection()
        .execute("VACUUM INTO ?1", [workspace.store_path().to_str().unwrap()])
        .unwrap();
    drop(copied);
    let Err(err) = workspace::workspace_for(&home, &found) else {
        panic!("a store from another Workspace opened");
    };
    assert!(
        matches!(
            store_error(&err),
            Some(StoreError::WorkspaceMismatch { .. })
        ),
        "{err}"
    );
}

#[test]
fn a_workspace_is_created_once_and_records_its_id() {
    let dir = TempDir::new();
    let home = dir.path().join("home");
    let id = Uuid::now_v7();
    let (workspace, store) = Workspace::create(&home, id).unwrap();
    assert_eq!(workspace.dir, home.join("workspaces").join(id.to_string()));
    assert_eq!(store.workspace_id().unwrap(), Some(id));
    drop(store);
    let Err(err) = Workspace::create(&home, id) else {
        panic!("a second create of one Workspace succeeded");
    };
    assert!(
        matches!(err, WorkspaceError::WorkspaceExists { id: existing, .. } if existing == id),
        "{err}"
    );
}

const CHILD_HOME: &str = "TM_STORE_TEST_WORKSPACE_HOME";
const CHILD_ID: &str = "TM_STORE_TEST_WORKSPACE_ID";

/// Under `umask 0277` the store cannot be written into the directory
/// `create` just made. The child leaves a marker so a filter that matches
/// no test cannot pass.
#[test]
fn a_failed_create_leaves_nothing_behind() {
    if let (Ok(home), Ok(id)) = (std::env::var(CHILD_HOME), std::env::var(CHILD_ID)) {
        let id: Uuid = id.parse().unwrap();
        assert!(Workspace::create(Path::new(&home), id).is_err());
        fs::write(Path::new(&home).join("ran"), "ran").unwrap();
        return;
    }
    let dir = TempDir::new();
    let home = dir.path().join("home");
    fs::create_dir_all(home.join("workspaces")).unwrap();
    let id = Uuid::now_v7();
    let exe = std::env::current_exe().unwrap();
    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg("umask 0277 && exec \"$0\" \"$@\"")
        .arg(&exe)
        .args([
            "--exact",
            "workspace::a_failed_create_leaves_nothing_behind",
        ])
        .args(["--test-threads", "1", "--quiet"])
        .env(CHILD_HOME, &home)
        .env(CHILD_ID, id.to_string())
        .status()
        .unwrap();
    assert!(status.success(), "the create in the child did not fail");
    assert!(home.join("ran").exists(), "the child test did not run");
    assert!(!Workspace::at(&home, id).dir.exists());
    Workspace::create(&home, id).unwrap();
}
