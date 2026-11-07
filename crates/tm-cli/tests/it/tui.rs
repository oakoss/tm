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
    assert!(text(&out.stderr).contains("fix: tm list\n"), "{out:?}");
    for args in ["--json tui", "tui --json"] {
        let mut tui = sized(&scratch, &repo, args);
        let said = tui.exp_eof().unwrap();
        assert!(said.contains(r#""error":"usage""#), "{args}: {said}");
        assert!(said.contains("no JSON form"), "{args}: {said}");
        assert!(said.contains(r#""fix":"tm list --json""#), "{args}: {said}");
        assert!(!said.contains("\u{1b}[?1049h"), "never drew: {said}");
        assert_eq!(exit_code(&tui), 2);
    }
    let out = jace(&scratch, &repo, &["tui"])
        .env("TM_JSON", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(text(&out.stdout).contains("TM_JSON=1"), "{out:?}");
}

#[test]
fn its_help_does_not_offer_json() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let out = tm(&scratch, &repo, &["tui", "--help"]);
    assert!(out.status.success(), "{out:?}");
    assert!(!text(&out.stdout).contains("--json"), "{out:?}");
}

#[test]
fn at_a_terminal_it_names_the_project_shows_its_keys_and_quits_on_q() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let mut tui = sized(&scratch, &repo, "tui");
    tui.exp_string("TM").unwrap();
    tui.exp_string("match").unwrap();
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
    tui.exp_string("match").unwrap();
    let made = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "fresh"],
    );
    assert!(made.status.success(), "{made:?}");
    tui.exp_string("fresh").unwrap();
    tui.exp_string("elsewhere").unwrap();
    press(&mut tui, "q");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}

#[test]
fn the_list_shows_open_issues_and_a_typed_filter_narrows_it() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    for title in ["alpha", "beta"] {
        let made = tm(
            &scratch,
            &repo,
            &["create", "--type", "task", "--title", title],
        );
        assert!(made.status.success(), "{made:?}");
    }
    let mut tui = sized(&scratch, &repo, "tui");
    tui.exp_string("beta").unwrap();
    tui.exp_string("alpha").unwrap();
    press(&mut tui, "/title:nothing\r");
    tui.exp_string("match").unwrap();
    press(&mut tui, "q");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}

#[test]
fn enter_shows_the_issue_as_tm_show_and_history_do_and_reads_a_write_in_place() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let made = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "alpha"],
    );
    assert!(made.status.success(), "{made:?}");
    let mut tui = sized(&scratch, &repo, "tui");
    tui.exp_string("alpha").unwrap();
    press(&mut tui, "\r");
    tui.exp_string("History").unwrap();
    let said = tm(&scratch, &repo, &["comment", "TM-1", "plotted"]);
    assert!(said.status.success(), "{said:?}");
    tui.exp_string("plotted").unwrap();
    press(&mut tui, "\x1b");
    tui.exp_string("filter").unwrap();
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
    tui.exp_string("match").unwrap();
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
        tui.exp_string("match").unwrap();
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
    tui.exp_string("match").unwrap();
    tui.process_mut().signal(Signal::SIGHUP).unwrap();
    tui.exp_eof().unwrap();
    let status = tui.process().wait().unwrap();
    assert!(
        matches!(status, WaitStatus::Signaled(_, Signal::SIGHUP, _)),
        "{status:?}"
    );
}

#[test]
fn a_reader_refused_the_store_reads_each_snapshot_published() {
    use std::os::unix::fs::PermissionsExt as _;

    let scratch = Scratch::new();
    let (repo, store, _project) = initialized(&scratch);
    drop(store);
    let made = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "before"],
    );
    assert!(made.status.success(), "{made:?}");
    let path = store_path(&scratch);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let mut tui = sized(&scratch, &repo, "tui");
    tui.exp_string("Snapshot").unwrap();
    tui.exp_string("before").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let made = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "after"],
    );
    assert!(made.status.success(), "{made:?}");
    tui.exp_string("after").unwrap();
    press(&mut tui, "q");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}

#[test]
fn at_me_is_the_person_at_the_terminal() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    for args in [
        &["create", "--type", "task", "--title", "mine"][..],
        &["edit", "TM-1", "--set", "assignee=jace"],
    ] {
        let out = tm(&scratch, &repo, args);
        assert!(out.status.success(), "{out:?}");
    }
    let mut tui = sized(&scratch, &repo, "tui");
    tui.exp_string("mine").unwrap();
    // Only when @me is jace does excluding jace's Issues leave none.
    press(&mut tui, "/-assignee:@me\r");
    tui.exp_string("match").unwrap();
    press(&mut tui, "q");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}

#[test]
fn a_store_replaced_while_open_is_read_from_its_new_file() {
    let scratch = Scratch::new();
    let (repo, store, _project) = initialized(&scratch);
    drop(store);
    let mut tui = sized(&scratch, &repo, "tui");
    tui.exp_string("match").unwrap();
    let path = store_path(&scratch);
    let copy = scratch.0.join("copy.db");
    fs::copy(&path, &copy).unwrap();
    let made = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "lost"],
    );
    assert!(made.status.success(), "{made:?}");
    tui.exp_string("lost").unwrap();
    let mut replacement = tm_store::Store::open(&copy).unwrap();
    let jace = tm_core::Actor::human("jace");
    let project = replacement.project_by_key("TM").unwrap().unwrap();
    replacement
        .create_issue(&jace, project, "task", "replacing")
        .unwrap();
    drop(replacement);
    fs::rename(&copy, &path).unwrap();
    tui.exp_string("replacing").unwrap();
    press(&mut tui, "q");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}
