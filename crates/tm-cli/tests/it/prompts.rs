//! `tm`'s questions to a person, driven at a pseudo-terminal.

use rexpect::process::WaitStatus;
use tm_store::Store;
use tm_store::workspace::{RepoFile, Workspace};

use crate::support::{Scratch, fixture, tm_pty};

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
fn setup_writes_the_hooks_on_a_yes_and_nothing_on_a_no() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let settings = repo.join(".claude/settings.json");

    let mut tm = tm_pty(&scratch, &repo, &["setup", "claude"]);
    tm.exp_string("+++ b/.claude/settings.json").unwrap();
    tm.exp_string("Write .claude/settings.json? [y/N] ")
        .unwrap();
    tm.send_line("n").unwrap();
    let rest = tm.exp_eof().unwrap();
    assert!(rest.contains(".claude/settings.json not written"), "{rest}");
    assert!(!settings.exists());

    let mut tm = tm_pty(&scratch, &repo, &["setup", "claude"]);
    tm.exp_string("Write .claude/settings.json? [y/N] ")
        .unwrap();
    tm.send_line("y").unwrap();
    let rest = tm.exp_eof().unwrap();
    assert!(rest.contains("Commit .claude/settings.json\r\n"), "{rest}");
    assert!(
        std::fs::read_to_string(&settings)
            .unwrap()
            .contains("tm prime")
    );

    let mut tm = tm_pty(&scratch, &repo, &["setup", "claude", "--undo"]);
    tm.exp_string("Remove .claude/settings.json? [y/N] ")
        .unwrap();
    tm.send_line("y").unwrap();
    tm.exp_eof().unwrap();
    assert!(!settings.exists());
}

#[test]
fn setup_writes_nothing_when_the_file_changed_while_it_asked() {
    let scratch = Scratch::new();
    let repo = fixture(&scratch, "repo");
    let settings = repo.join(".claude/settings.json");
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    std::fs::write(&settings, "{}\n").unwrap();

    let mut tm = tm_pty(&scratch, &repo, &["setup", "claude"]);
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
    let mut tm = tm_pty(&scratch, &repo, &["setup", "claude"]);
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
