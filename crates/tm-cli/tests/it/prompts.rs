//! `tm`'s questions to a person, driven at a pseudo-terminal.

use std::path::Path;

use rexpect::process::WaitStatus;
use rexpect::session::PtySession;
use tm_store::Store;
use tm_store::workspace::{RepoFile, Workspace};
use uuid::Uuid;

use crate::support::{Scratch, fixture, initialized, jace, tm, tm_pty};

/// `tm` at a terminal on another machine: the same repo, `home` as `TM_HOME`.
fn elsewhere(scratch: &Scratch, home: &Path, repo: &Path, args: &[&str]) -> PtySession {
    let mut command = jace(scratch, repo, args);
    command.env("TM_HOME", home);
    rexpect::session::spawn_command(command, Some(10_000)).unwrap()
}

/// A mapped repo with one Issue, and its export saved under `name`.
fn exported(scratch: &Scratch, name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let (repo, _store, _) = initialized(scratch);
    let out = tm(
        scratch,
        &repo,
        &["create", "--type", "task", "--title", "kept"],
    );
    assert!(out.status.success(), "{out:?}");
    let out = tm(scratch, &repo, &["export"]);
    assert!(out.status.success(), "{out:?}");
    let file = scratch.0.join(name);
    std::fs::write(&file, &out.stdout).unwrap();
    (repo, file)
}

/// Asserts `tm` exited with `code`.
fn exited(tm: &PtySession, code: i32) {
    let status = tm.process().wait().unwrap();
    assert!(
        matches!(status, WaitStatus::Exited(_, c) if c == code),
        "{status:?}"
    );
}

/// Asserts no repo file and no Workspace exist.
fn nothing_written(scratch: &Scratch, repo: &Path) {
    assert!(!repo.join(".config/tm.toml").exists());
    let workspaces = scratch.home().join("workspaces");
    let made = std::fs::read_dir(&workspaces).map_or(0, Iterator::count);
    assert_eq!(made, 0, "no Workspace in {}", workspaces.display());
}

#[test]
fn an_agent_at_a_terminal_is_not_asked() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let mut command = jace(&scratch, &repo, &["init"]);
    command.env("TM_AGENT", "claude");
    let mut tm = rexpect::session::spawn_command(command, Some(10_000)).unwrap();
    let rest = tm.exp_eof().unwrap();
    assert!(rest.contains("not written: confirm with --yes"), "{rest}");
    assert!(!rest.contains("[y/N]"), "{rest}");
    exited(&tm, 1);
    nothing_written(&scratch, &repo);
}

#[test]
fn enter_takes_the_suggested_key_and_name() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let mut tm = tm_pty(&scratch, &repo, &["init"]);
    tm.exp_string("Project key [REP]: ").unwrap();
    tm.send_line("").unwrap();
    tm.exp_string("Project name [repo]: ").unwrap();
    tm.send_line("").unwrap();
    tm.exp_string("+project = \"REP\"").unwrap();
    tm.exp_string("Write .config/tm.toml? [y/N] ").unwrap();
    tm.send_line("y").unwrap();
    tm.exp_string("Add tm's lines to AGENTS.md? [y/N] ")
        .unwrap();
    tm.send_line("y").unwrap();
    let rest = tm.exp_eof().unwrap();
    exited(&tm, 0);
    assert!(
        rest.contains("Commit .config/tm.toml and AGENTS.md"),
        "{rest}"
    );
    let file =
        RepoFile::parse(&std::fs::read_to_string(repo.join(".config/tm.toml")).unwrap()).unwrap();
    assert_eq!(file.project, "REP");
    let agents = std::fs::read_to_string(repo.join("AGENTS.md")).unwrap();
    assert!(agents.starts_with("agents\n"), "{agents}");
    assert!(
        agents.contains("tm guide"),
        "a yes adds tm's lines: {agents}"
    );
}

#[test]
fn end_of_input_at_each_question_writes_nothing() {
    for answered in 0..3 {
        let scratch = Scratch::new();
        let repo = fixture(&scratch, "repo");
        let mut tm = tm_pty(&scratch, &repo, &["init"]);
        let questions = [
            (
                "Project key [REP]: ",
                "stopped at the prompt; nothing written",
            ),
            (
                "Project name [repo]: ",
                "stopped at the prompt; nothing written",
            ),
            (
                "Write .config/tm.toml? [y/N] ",
                ".config/tm.toml not written",
            ),
        ];
        for (question, _) in &questions[..answered] {
            tm.exp_string(question).unwrap();
            tm.send_line("").unwrap();
        }
        let (question, refusal) = questions[answered];
        tm.exp_string(question).unwrap();
        tm.send_control('d').unwrap();
        let rest = tm.exp_eof().unwrap();
        assert!(rest.contains(refusal), "at {question:?}: {rest}");
        exited(&tm, 1);
        nothing_written(&scratch, &repo);
    }
}

#[test]
fn a_repo_file_that_appears_while_tm_asks_is_kept() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let mut tm = tm_pty(&scratch, &repo, &["init", "--key", "TM", "--name", "repo"]);
    tm.exp_string("Write .config/tm.toml? [y/N] ").unwrap();
    let theirs = RepoFile {
        workspace: Uuid::now_v7(),
        project: "OTHER".into(),
        paths: Vec::new(),
        claim_expiry: None,
    }
    .render();
    std::fs::create_dir_all(repo.join(".config")).unwrap();
    std::fs::write(repo.join(".config/tm.toml"), &theirs).unwrap();
    tm.send_line("y").unwrap();
    tm.exp_string("Add tm's lines to AGENTS.md? [y/N] ")
        .unwrap();
    tm.send_line("y").unwrap();
    let rest = tm.exp_eof().unwrap();
    assert!(rest.contains("appeared while waiting"), "{rest}");
    exited(&tm, 1);
    assert_eq!(
        std::fs::read_to_string(repo.join(".config/tm.toml")).unwrap(),
        theirs
    );
    let workspaces = scratch.home().join("workspaces");
    assert_eq!(std::fs::read_dir(&workspaces).map_or(0, Iterator::count), 0);
    assert_eq!(
        std::fs::read_to_string(repo.join("AGENTS.md")).unwrap(),
        "agents\n",
        "the yes to tm's lines is dropped with the refusal"
    );
}

#[test]
fn a_refused_key_is_asked_again_and_the_typed_one_is_written() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let mut tm = tm_pty(&scratch, &repo, &["init"]);

    tm.exp_string("Project key [REP]: ").unwrap();
    tm.send_line("1x").unwrap();
    tm.exp_string("\"1X\" is not a Project key").unwrap();
    tm.exp_string("Project key [REP]: ").unwrap();
    tm.send_line("ops").unwrap();
    tm.exp_string("Project name [repo]: ").unwrap();
    tm.send_line("").unwrap();
    tm.exp_string("+project = \"OPS\"").unwrap();
    tm.exp_string("Write .config/tm.toml? [y/N] ").unwrap();
    tm.send_line("y").unwrap();
    tm.exp_string("Add tm's lines to AGENTS.md? [y/N] ")
        .unwrap();
    tm.send_line("n").unwrap();
    let rest = tm.exp_eof().unwrap();
    assert!(rest.contains("Commit .config/tm.toml\r\n"), "{rest}");
    let status = tm.process().wait().unwrap();
    assert!(matches!(status, WaitStatus::Exited(_, 0)), "{status:?}");

    let file =
        RepoFile::parse(&std::fs::read_to_string(repo.join(".config/tm.toml")).unwrap()).unwrap();
    assert_eq!(file.project, "OPS");
    let workspace = Workspace::at(&scratch.home(), file.workspace);
    let store = Store::open_existing(&workspace.store_path(), file.workspace).unwrap();
    let name: String = store
        .connection()
        .query_row("SELECT name FROM projects WHERE key = 'OPS'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(name, "repo", "Enter takes the suggested name");
    let agents = std::fs::read_to_string(repo.join("AGENTS.md")).unwrap();
    assert_eq!(agents, "agents\n", "a no leaves AGENTS.md as it was");
}

#[test]
fn a_fresh_clone_restores_its_workspace_at_the_prompt() {
    const ASKED: &str = "Path to an export of it (Enter to skip): ";
    let scratch = Scratch::new();
    let (repo, file) = exported(&scratch, "backup.jsonl");
    let other = Scratch::new();
    let (_, foreign) = exported(&other, "foreign.jsonl");
    let home = scratch.0.join("other-home");

    let mut tm = elsewhere(&scratch, &home, &repo, &["init"]);
    tm.exp_string("which this machine lacks").unwrap();
    tm.exp_string(ASKED).unwrap();
    tm.send_line("-").unwrap();
    tm.exp_string("- is not a file; give the export's path")
        .unwrap();
    tm.exp_string(ASKED).unwrap();
    tm.send_line(foreign.to_str().unwrap()).unwrap();
    tm.exp_string("is an export of Workspace").unwrap();
    tm.exp_string(ASKED).unwrap();
    assert!(
        !home.join("workspaces").exists(),
        "refusals restore nothing"
    );
    tm.send_line(file.to_str().unwrap()).unwrap();
    tm.exp_string("Add tm's lines to AGENTS.md? [y/N] ")
        .unwrap();
    tm.send_line("n").unwrap();
    let rest = tm.exp_eof().unwrap();
    exited(&tm, 0);
    assert!(rest.contains("already maps"), "{rest}");

    let mut command = jace(&scratch, &repo, &["list"]);
    command.env("TM_HOME", &home);
    let listed = command.stdin(std::process::Stdio::null()).output().unwrap();
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains("kept"),
        "{listed:?}"
    );
}

#[test]
fn a_restore_that_lacks_the_named_project_still_reports_what_it_restored() {
    let scratch = Scratch::new();
    let (repo, file) = exported(&scratch, "backup.jsonl");
    let home = scratch.0.join("other-home");
    let path = repo.join(".config/tm.toml");
    let mut mapped = RepoFile::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
    mapped.project = "ZZZ".into();
    std::fs::write(&path, mapped.render()).unwrap();

    let mut tm = elsewhere(&scratch, &home, &repo, &["init"]);
    tm.exp_string("Path to an export of it (Enter to skip): ")
        .unwrap();
    tm.send_line(file.to_str().unwrap()).unwrap();
    let rest = tm.exp_eof().unwrap();
    exited(&tm, 1);
    assert!(rest.contains("names Project ZZZ"), "{rest}");
    assert!(rest.contains("restored 5 operations"), "{rest}");
    assert!(rest.contains("the same next Keys"), "{rest}");
    let restored = Workspace::at(&home, mapped.workspace);
    assert!(
        Store::open_existing(&restored.store_path(), mapped.workspace).is_ok(),
        "the restore is kept"
    );
}

#[test]
fn a_bd_export_chosen_at_the_prompt_is_imported_and_published() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    std::fs::create_dir(repo.join(".beads")).unwrap();
    let beads = format!(
        "{}/../tm-store/tests/fixtures/beads.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );

    let mut tm = tm_pty(&scratch, &repo, &["init", "--key", "TM", "--name", "repo"]);
    tm.exp_string("Path to a bd export to import (Enter to skip): ")
        .unwrap();
    tm.send_line(&beads).unwrap();
    tm.exp_string("Write .config/tm.toml? [y/N] ").unwrap();
    tm.send_line("y").unwrap();
    tm.exp_string("Add tm's lines to AGENTS.md? [y/N] ")
        .unwrap();
    tm.send_line("n").unwrap();
    let rest = tm.exp_eof().unwrap();
    exited(&tm, 0);
    assert!(!rest.contains("To bring beads' Issues"), "{rest}");

    let file =
        RepoFile::parse(&std::fs::read_to_string(repo.join(".config/tm.toml")).unwrap()).unwrap();
    let workspace = Workspace::at(&scratch.home(), file.workspace);
    let held = |store: &Store| -> i64 {
        store
            .connection()
            .query_row("SELECT count(*) FROM issues", [], |row| row.get(0))
            .unwrap()
    };
    let store = Store::open_existing(&workspace.store_path(), file.workspace).unwrap();
    let imported = held(&store);
    assert!(imported > 0, "{rest}");
    let snapshot = Store::open_snapshot(&workspace.snapshot_path(), file.workspace).unwrap();
    assert_eq!(held(&snapshot), imported, "published after the import");
}

#[test]
fn setup_writes_the_hooks_on_a_yes_and_nothing_on_a_no() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let local = repo.join(".claude/settings.local.json");
    let shared = repo.join(".claude/settings.json");

    let mut tm = tm_pty(&scratch, &repo, &["setup", "claude"]);
    tm.exp_string("Where should tm's hooks go?").unwrap();
    tm.exp_string("Choose 1 or 2 [1]: ").unwrap();
    tm.send_line("").unwrap();
    tm.exp_string("+++ b/.claude/settings.local.json").unwrap();
    tm.exp_string("Write .claude/settings.local.json? [y/N] ")
        .unwrap();
    tm.send_line("n").unwrap();
    let rest = tm.exp_eof().unwrap();
    assert!(
        rest.contains(".claude/settings.local.json not written"),
        "{rest}"
    );
    assert!(!local.exists());

    let mut tm = tm_pty(&scratch, &repo, &["setup", "claude"]);
    tm.exp_string("Choose 1 or 2 [1]: ").unwrap();
    tm.send_line("3").unwrap();
    tm.exp_string("Choose 1 or 2 [1]: ").unwrap();
    tm.send_line("1").unwrap();
    tm.exp_string("Write .claude/settings.local.json? [y/N] ")
        .unwrap();
    tm.send_line("y").unwrap();
    let rest = tm.exp_eof().unwrap();
    assert!(rest.contains("it is not committed\r\n"), "{rest}");
    assert!(
        std::fs::read_to_string(&local)
            .unwrap()
            .contains("tm prime")
    );

    let mut tm = tm_pty(&scratch, &repo, &["setup", "claude"]);
    let rest = tm.exp_eof().unwrap();
    assert!(
        rest.contains("tm's hooks are already in .claude/settings.local.json")
            && !rest.contains("Where should"),
        "{rest}"
    );

    // With the hooks in a file, setup acts there without asking where.
    let mut tm = tm_pty(&scratch, &repo, &["setup", "claude", "--undo"]);
    tm.exp_string("Remove .claude/settings.local.json? [y/N] ")
        .unwrap();
    tm.send_line("y").unwrap();
    tm.exp_eof().unwrap();
    assert!(!local.exists());

    let mut tm = tm_pty(&scratch, &repo, &["setup", "claude"]);
    tm.exp_string("Choose 1 or 2 [1]: ").unwrap();
    tm.send_line("2").unwrap();
    tm.exp_string("Write .claude/settings.json? [y/N] ")
        .unwrap();
    tm.send_line("y").unwrap();
    let rest = tm.exp_eof().unwrap();
    assert!(rest.contains("Commit .claude/settings.json\r\n"), "{rest}");
    assert!(shared.exists() && !local.exists());
}

#[test]
fn end_of_input_where_setup_asks_for_a_file_writes_nothing() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let mut tm = tm_pty(&scratch, &repo, &["setup", "claude"]);
    tm.exp_string("Choose 1 or 2 [1]: ").unwrap();
    tm.send_control('d').unwrap();
    let rest = tm.exp_eof().unwrap();
    assert!(
        rest.contains("stopped at the prompt; nothing written"),
        "{rest}"
    );
    exited(&tm, 1);
    assert!(!repo.join(".claude").exists());
}

#[test]
fn an_undo_across_both_files_writes_neither_when_one_changed_while_it_asked() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let made = crate::support::tm(&scratch, &repo, &["setup", "claude", "--shared", "--yes"]);
    assert!(made.status.success(), "{made:?}");
    let shared = repo.join(".claude/settings.json");
    let local = repo.join(".claude/settings.local.json");
    std::fs::copy(&shared, &local).unwrap();

    let mut tm = tm_pty(&scratch, &repo, &["setup", "claude", "--undo"]);
    tm.exp_string(
        "Remove tm's hooks from .claude/settings.json and .claude/settings.local.json? [y/N] ",
    )
    .unwrap();
    std::fs::write(&local, "{\"model\": \"edited\"}\n").unwrap();
    tm.send_line("y").unwrap();
    let rest = tm.exp_eof().unwrap();
    assert!(
        rest.contains(".claude/settings.local.json changed while tm asked; nothing written"),
        "{rest}"
    );
    exited(&tm, 1);
    assert!(shared.exists());
}

#[test]
fn setup_writes_nothing_when_the_file_changed_while_it_asked() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let settings = repo.join(".claude/settings.json");
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    std::fs::write(&settings, "{}\n").unwrap();

    let mut tm = tm_pty(&scratch, &repo, &["setup", "claude", "--shared"]);
    tm.exp_string("Write .claude/settings.json? [y/N] ")
        .unwrap();
    std::fs::write(&settings, "{\"model\": \"edited\"}\n").unwrap();
    tm.send_line("y").unwrap();
    let rest = tm.exp_eof().unwrap();
    assert!(
        rest.contains(".claude/settings.json changed while tm asked; nothing written"),
        "{rest}"
    );
    assert_eq!(
        std::fs::read_to_string(&settings).unwrap(),
        "{\"model\": \"edited\"}\n"
    );

    // A link to the same text, swapped in while tm asked, is still a link.
    let shared = scratch.0.join("shared.json");
    std::fs::write(&shared, "{}\n").unwrap();
    std::fs::write(&settings, "{}\n").unwrap();
    let mut tm = tm_pty(&scratch, &repo, &["setup", "claude", "--shared"]);
    tm.exp_string("Write .claude/settings.json? [y/N] ")
        .unwrap();
    std::fs::remove_file(&settings).unwrap();
    std::os::unix::fs::symlink(&shared, &settings).unwrap();
    tm.send_line("y").unwrap();
    let rest = tm.exp_eof().unwrap();
    assert!(
        rest.contains("is a symlink, so tm leaves it alone"),
        "{rest}"
    );
    let link = std::fs::symlink_metadata(&settings).unwrap();
    assert!(link.file_type().is_symlink());
    assert_eq!(std::fs::read_to_string(&shared).unwrap(), "{}\n");
}
