//! `tm tui` opens its screens only at a terminal, shows writes made
//! elsewhere, and leaves the terminal as it found it (ADR 0018).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rexpect::process::{Signal, WaitStatus};
use rexpect::session::PtySession;

use crate::support::{Scratch, initialized, jace, text, tm};

/// Leaving the alternate screen, the last step of restoring the terminal.
const LEFT: &str = "\u{1b}[?1049l";

/// `tm` with `args` at an 80 by 24 terminal: rexpect's opens at 0 by 0,
/// where nothing is drawn.
fn sized(scratch: &Scratch, repo: &Path, args: &str) -> PtySession {
    let tm = jace(scratch, repo, &[]);
    let mut command = Command::new("sh");
    command.current_dir(repo).args([
        "-c",
        &format!(
            "stty cols 80 rows 24 && exec '{}' {args}",
            tm.get_program().display()
        ),
    ]);
    for (key, value) in tm.get_envs() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    rexpect::session::spawn_command(command, Some(10_000)).unwrap()
}

fn press(tui: &mut PtySession, key: &str) {
    tui.send(key).unwrap();
    tui.flush().unwrap();
}

fn exit_code(tui: &PtySession) -> i32 {
    match tui.process().wait().unwrap() {
        WaitStatus::Exited(_, code) => code,
        other => panic!("{other:?}"),
    }
}

fn store_path(scratch: &Scratch) -> PathBuf {
    let mut workspaces = fs::read_dir(scratch.home().join("workspaces")).unwrap();
    workspaces.next().unwrap().unwrap().path().join("store.db")
}

#[test]
fn without_a_terminal_or_under_json_it_refuses_and_says_why() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let out = tm(&scratch, &repo, &["tui"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(text(&out.stderr).contains("needs a terminal"), "{out:?}");
    let mut tui = sized(&scratch, &repo, "--json tui");
    let said = tui.exp_eof().unwrap();
    assert!(said.contains("not_a_terminal"), "{said}");
    assert!(!said.contains("\u{1b}[?1049h"), "never drew: {said}");
    assert_eq!(exit_code(&tui), 1);
}

#[test]
fn at_a_terminal_it_names_the_project_shows_its_keys_and_quits_on_q() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let mut tui = sized(&scratch, &repo, "tui");
    tui.exp_string("TM").unwrap();
    tui.exp_string("Nothing").unwrap();
    press(&mut tui, "?");
    tui.exp_string("Keys").unwrap();
    press(&mut tui, "q");
    let rest = tui.exp_eof().unwrap();
    assert!(rest.contains(LEFT), "left the alternate screen");
    assert_eq!(exit_code(&tui), 0);
}

#[test]
fn a_write_elsewhere_shows_in_the_footer() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let mut tui = sized(&scratch, &repo, "tui");
    tui.exp_string("Nothing").unwrap();
    let made = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "a"],
    );
    assert!(made.status.success(), "{made:?}");
    tui.exp_string("elsewhere").unwrap();
    press(&mut tui, "q");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}

#[test]
fn a_store_removed_while_open_restores_the_terminal_then_says_so() {
    let scratch = Scratch::new();
    let (repo, store, _project) = initialized(&scratch);
    drop(store);
    let mut tui = sized(&scratch, &repo, "tui");
    tui.exp_string("Nothing").unwrap();
    let path = store_path(&scratch);
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let rest = tui.exp_eof().unwrap();
    let left = rest.find(LEFT).expect("left the alternate screen");
    let said = rest.find("moved or deleted").expect("said why");
    assert!(left < said, "restored before saying so: {rest}");
    assert_eq!(exit_code(&tui), 1);
}

#[test]
fn a_repo_file_naming_a_missing_project_is_refused() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let file = repo.join(".config/tm.toml");
    let named = fs::read_to_string(&file)
        .unwrap()
        .replace("\"TM\"", "\"ZZ\"");
    fs::write(&file, named).unwrap();
    let mut tui = sized(&scratch, &repo, "tui");
    let said = tui.exp_eof().unwrap();
    assert!(said.contains("Project ZZ"), "{said}");
    assert_eq!(exit_code(&tui), 1);
}

#[test]
fn a_signal_restores_the_terminal_and_exits_as_the_signal_would() {
    for (signal, number) in [(Signal::SIGTERM, 15), (Signal::SIGINT, 2)] {
        let scratch = Scratch::new();
        let (repo, _store, _project) = initialized(&scratch);
        let mut tui = sized(&scratch, &repo, "tui");
        tui.exp_string("Nothing").unwrap();
        tui.process_mut().signal(signal).unwrap();
        let rest = tui.exp_eof().unwrap();
        assert!(rest.contains(LEFT), "{signal:?} left the alternate screen");
        assert_eq!(exit_code(&tui), 128 + number, "{signal:?}");
    }
}

#[test]
fn a_hangup_ends_it_as_the_signal_does() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let mut tui = sized(&scratch, &repo, "tui");
    tui.exp_string("Nothing").unwrap();
    tui.process_mut().signal(Signal::SIGHUP).unwrap();
    tui.exp_eof().unwrap();
    let status = tui.process().wait().unwrap();
    assert!(
        matches!(status, WaitStatus::Signaled(_, Signal::SIGHUP, _)),
        "{status:?}"
    );
}
