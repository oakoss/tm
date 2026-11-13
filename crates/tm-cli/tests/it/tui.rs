//! `tm tui` opens its screens only at a terminal, shows writes made
//! elsewhere, and leaves the terminal as it found it (ADR 0018).

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use rexpect::process::{Signal, WaitStatus};
use rexpect::session::PtySession;

use crate::support::{Scratch, initialized, jace, text, tm};

/// Leaving the alternate screen, the last step of restoring the terminal.
const LEFT: &str = "\u{1b}[?1049l";

/// `tm` with `args` at an 80 by 24 terminal: rexpect's opens at 0 by 0,
/// where nothing is drawn. It first prints `pid N`, since rexpect keeps
/// the pid to itself and `exec` keeps it the same.
fn sized(scratch: &Scratch, repo: &Path, args: &str) -> PtySession {
    sized_with(scratch, repo, args, &[])
}

fn sized_with(scratch: &Scratch, repo: &Path, args: &str, env: &[(&str, &str)]) -> PtySession {
    let mut tm = jace(scratch, repo, &[]);
    tm.envs(env.iter().copied());
    let mut command = Command::new("sh");
    command.current_dir(repo).args([
        "-c",
        &format!(
            "stty cols 80 rows 24 && echo \"pid $$\" && exec '{}' {args}",
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
fn a_reader_refused_the_store_is_warned_it_reads_the_snapshot() {
    let scratch = Scratch::new();
    let (repo, store, _project) = initialized(&scratch);
    drop(store);
    let made = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "a"],
    );
    assert!(made.status.success(), "{made:?}");
    let path = store_path(&scratch);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let mut tui = sized(&scratch, &repo, "tui");
    let shown = tui.exp_string("published");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    shown.unwrap();
    press(&mut tui, "q");
    tui.exp_eof().unwrap();
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
fn enter_shows_the_issue_reads_a_write_in_place_and_opens_its_parent() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    for made in [
        tm(
            &scratch,
            &repo,
            &["create", "--type", "epic", "--title", "whole"],
        ),
        tm(
            &scratch,
            &repo,
            &[
                "create", "--type", "task", "--title", "alpha", "--parent", "TM-1",
            ],
        ),
    ] {
        assert!(made.status.success(), "{made:?}");
    }
    let mut tui = sized(&scratch, &repo, "tui");
    tui.exp_string("alpha").unwrap();
    press(&mut tui, "\r");
    tui.exp_string("History").unwrap();
    tui.exp_string("parent").unwrap();
    // Letters no earlier text drew, so the redraw writes them in one run.
    let said = tm(&scratch, &repo, &["comment", "TM-2", "**QZXQZX**"]);
    assert!(said.status.success(), "{said:?}");
    tui.exp_string("QZXQZX").unwrap();
    press(&mut tui, "\r");
    tui.exp_string("child").unwrap();
    // Back to TM-2, back to the list, then quit.
    press(&mut tui, "qqq");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}

#[test]
fn c_writes_what_the_editor_saved_as_tm_comment_does_and_takes_the_terminal_back() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let made = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "alpha"],
    );
    assert!(made.status.success(), "{made:?}");
    let recorded = scratch.home().join("draft-path");
    let editor = format!(
        "printf %s \"$1\" > '{}'; printf '**QZXQZX** from the editor' >",
        recorded.display()
    );
    let editor = editor.as_str();
    let mut tui = sized_with(
        &scratch,
        &repo,
        "tui",
        &[("VISUAL", editor), ("EDITOR", "false")],
    );
    tui.exp_string("alpha").unwrap();
    press(&mut tui, "\r");
    tui.exp_string("History").unwrap();
    press(&mut tui, "c");
    tui.exp_string(LEFT).unwrap();
    tui.exp_string("\u{1b}[?1049h").unwrap();
    // The title, unchanged, drawn again, as the editor drew over the screen.
    tui.exp_string("alpha").unwrap();
    tui.exp_string("QZXQZX").unwrap();
    let shown = tm(
        &scratch,
        &repo,
        &["--json", "show", "TM-1", "--comments", "all"],
    );
    let doc: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    let comments = &doc["data"]["comments"];
    assert_eq!(
        comments[0]["body"], "**QZXQZX** from the editor",
        "{comments}"
    );
    assert_eq!(comments[0]["actor"], "jace", "{comments}");
    let draft = fs::read_to_string(&recorded).unwrap();
    assert!(!Path::new(&draft).exists(), "{draft} outlived its comment");
    press(&mut tui, "qq");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}

/// Process `pid`'s state, as `ps` reports it: empty once it is gone.
fn state(pid: &str) -> String {
    let ps = Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    text(&ps.stdout).trim().to_owned()
}

fn stopped(pid: &str) -> bool {
    state(pid).starts_with('T')
}

/// Neither gone nor a zombie its shell has yet to reap.
fn running(pid: &str) -> bool {
    let state = state(pid);
    !state.is_empty() && !state.starts_with('Z')
}

/// `tm tui` started from an interactive bash, a shell with job control, at
/// an 80 by 24 terminal, and its pid.
fn in_a_shell(scratch: &Scratch, repo: &Path, env: &[(&str, &str)]) -> (PtySession, String) {
    let mut tm = jace(scratch, repo, &[]);
    tm.envs(env.iter().copied());
    let mut command = Command::new("sh");
    command.current_dir(repo).args([
        "-c",
        "stty cols 80 rows 24 && exec env PS1='ready$ ' bash --norc --noprofile -i",
    ]);
    for (key, value) in tm.get_envs() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    command.env("BASH_SILENCE_DEPRECATION_WARNING", "1");
    let mut shell = rexpect::session::spawn_command(command, Some(10_000)).unwrap();
    shell.exp_string("ready$ ").unwrap();
    let program = tm.get_program().display().to_string();
    // `exec` keeps the pid the shell echoes.
    let line = format!("sh -c 'echo pid $$; exec \"$0\" tui' '{program}'");
    shell.send_line(&line).unwrap();
    let (_, said) = shell.exp_regex(r"pid \d+").unwrap();
    (shell, said.trim_start_matches("pid ").to_owned())
}

/// Waits for bash to report the job stopped and prompt again.
fn shell_has_it_stopped(shell: &mut PtySession) {
    shell.exp_string("Stopped").unwrap();
    shell.exp_string("ready$ ").unwrap();
}

/// The status `tm tui` ended with in `shell`, which then exits.
fn status_then_exit(shell: &mut PtySession, wait: &str) -> String {
    shell.send_line(&format!("{wait}echo status $?")).unwrap();
    let (_, said) = shell.exp_regex(r"status \d+").unwrap();
    shell.send_line("exit").unwrap();
    shell.exp_eof().unwrap();
    said
}

#[test]
fn ctrl_z_or_a_stop_signal_hands_the_terminal_back_and_fg_redraws_it() {
    for stop in ["ctrl-z", "SIGTSTP", "ctrl-z then bg"] {
        let scratch = Scratch::new();
        let (repo, _store, _project) = initialized(&scratch);
        let made = tm(
            &scratch,
            &repo,
            &["create", "--type", "task", "--title", "alpha"],
        );
        assert!(made.status.success(), "{made:?}");
        let (mut shell, pid) = in_a_shell(&scratch, &repo, &[]);
        shell.exp_string("alpha").unwrap();
        if stop == "SIGTSTP" {
            kill(&pid, "-TSTP");
        } else {
            press(&mut shell, "\u{1a}");
        }
        // The terminal is the shell's again before the TUI stops.
        shell.exp_string(LEFT).unwrap();
        settles(|| stopped(&pid));
        shell_has_it_stopped(&mut shell);
        if stop == "ctrl-z then bg" {
            // In the background it waits, stopped, for `fg`.
            shell.send_line("bg").unwrap();
            shell.exp_string("ready$ ").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            settles(|| stopped(&pid));
        }
        shell.send_line("fg").unwrap();
        if let Err(err) = shell.exp_string("\u{1b}[?1049h") {
            panic!("{stop}: fg did not take the terminal: {err}");
        }
        // The unchanged list drawn again, as the shell drew over it.
        shell.exp_string("alpha").unwrap();
        // Longer than a look for writes elsewhere, which would stop it again
        // on a stop left pending.
        std::thread::sleep(std::time::Duration::from_millis(600));
        assert!(!stopped(&pid), "{stop}: stopped again");
        press(&mut shell, "q");
        if let Err(err) = shell.exp_string("ready$ ") {
            panic!("{stop}: q did not quit: {err}");
        }
        assert_eq!(status_then_exit(&mut shell, ""), "status 0", "{stop}");
    }
}

fn kill(pid: &str, signal: &str) {
    let sent = Command::new("kill").args([signal, pid]).status().unwrap();
    assert!(sent.success());
}

#[test]
fn kill_while_suspended_ends_it_without_taking_the_terminal_back() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let (mut shell, pid) = in_a_shell(&scratch, &repo, &[]);
    shell.exp_string("match").unwrap();
    press(&mut shell, "\u{1a}");
    shell.exp_string(LEFT).unwrap();
    settles(|| stopped(&pid));
    shell_has_it_stopped(&mut shell);
    // SIGTERM then SIGCONT, to a job in the background.
    shell.send_line("kill %1").unwrap();
    settles(|| !running(&pid));
    // By pid, as bash 5 may already have reported the job done.
    let status = status_then_exit(&mut shell, &format!("wait {pid}; "));
    assert_eq!(status, "status 143");
}

#[test]
fn a_sigcont_after_an_uncatchable_sigstop_takes_the_terminal_back() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let made = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "alpha"],
    );
    assert!(made.status.success(), "{made:?}");
    let mut tui = sized(&scratch, &repo, "tui");
    let (_, said) = tui.exp_regex(r"pid \d+").unwrap();
    let pid = said.trim_start_matches("pid ").to_owned();
    tui.exp_string("alpha").unwrap();
    kill(&pid, "-STOP");
    settles(|| stopped(&pid));
    kill(&pid, "-CONT");
    tui.exp_string("\u{1b}[?1049h").unwrap();
    tui.exp_string("alpha").unwrap();
    press(&mut tui, "q");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}

#[test]
fn ctrl_z_with_no_shell_to_come_back_to_does_nothing() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    // rexpect starts it as the leader of its own session, as `exec tm tui`
    // or `tmux new-window 'tm tui'` would, so nothing could send it SIGCONT.
    let mut tui = sized(&scratch, &repo, "tui");
    let (_, said) = tui.exp_regex(r"pid \d+").unwrap();
    let pid = said.trim_start_matches("pid ").to_owned();
    tui.exp_string("match").unwrap();
    press(&mut tui, "\u{1a}");
    kill(&pid, "-TSTP");
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert!(!stopped(&pid));
    press(&mut tui, "q");
    let rest = tui.exp_eof().unwrap();
    assert!(!rest.contains("\u{1b}[?1049h"), "never left: {rest:?}");
    assert_eq!(exit_code(&tui), 0);
}

#[test]
fn ctrl_z_in_the_editor_stops_the_tui_with_it_and_fg_goes_on_writing() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let made = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "alpha"],
    );
    assert!(made.status.success(), "{made:?}");
    // SIGTSTP to the whole group, as Ctrl-Z sends it.
    let editor = "kill -TSTP 0; printf QZXQZX >";
    let (mut shell, pid) = in_a_shell(&scratch, &repo, &[("VISUAL", editor)]);
    shell.exp_string("alpha").unwrap();
    press(&mut shell, "\r");
    shell.exp_string("History").unwrap();
    press(&mut shell, "c");
    settles(|| stopped(&pid));
    shell_has_it_stopped(&mut shell);
    shell.send_line("fg").unwrap();
    shell.exp_string("commented:").unwrap();
    press(&mut shell, "qq");
    shell.exp_string("ready$ ").unwrap();
    let status = status_then_exit(&mut shell, "");
    assert_eq!(status, "status 0", "went on, not stopped again");
}

#[test]
fn a_comment_not_written_is_kept_and_its_path_printed_once_the_terminal_is_back() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let made = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "alpha"],
    );
    assert!(made.status.success(), "{made:?}");
    let editor = "printf 'careful words' > \"$1\"; false";
    let mut tui = sized_with(&scratch, &repo, "tui", &[("VISUAL", editor)]);
    tui.exp_string("alpha").unwrap();
    press(&mut tui, "\r");
    tui.exp_string("History").unwrap();
    press(&mut tui, "c");
    tui.exp_string("\u{1b}[?1049h").unwrap();
    tui.exp_string("printed").unwrap();
    press(&mut tui, "qq");
    tui.exp_string(LEFT).unwrap();
    let (_, said) = tui
        .exp_regex(r"tm: a comment not written is kept in \S+\.md")
        .unwrap();
    let draft = said.rsplit(' ').next().unwrap();
    assert_eq!(fs::read_to_string(draft).unwrap(), "careful words");
    fs::remove_file(draft).unwrap();
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}

#[test]
fn a_ctrl_c_in_the_editor_is_the_editors_and_a_dash_is_a_comment_not_stdin() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let made = tm(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "alpha"],
    );
    assert!(made.status.success(), "{made:?}");
    // SIGINT to the whole group twice, as Ctrl-C sends it, which the
    // editor shrugs off before saving.
    let editor = "trap '' INT; kill -INT 0; sleep 0.2; kill -INT 0; printf - >";
    let mut tui = sized_with(&scratch, &repo, "tui", &[("VISUAL", editor)]);
    tui.exp_string("alpha").unwrap();
    press(&mut tui, "\r");
    tui.exp_string("History").unwrap();
    press(&mut tui, "c");
    tui.exp_string("commented:").unwrap();
    let shown = tm(
        &scratch,
        &repo,
        &["--json", "show", "TM-1", "--comments", "all"],
    );
    let doc: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(doc["data"]["comments"][0]["body"], "-", "{doc}");
    // Back from the editor, a SIGINT is the TUI's again, ending it with the
    // terminal restored.
    tui.process_mut().signal(Signal::SIGINT).unwrap();
    let rest = tui.exp_eof().unwrap();
    assert!(rest.contains(LEFT), "left the alternate screen");
    assert_eq!(exit_code(&tui), 130);
}

#[test]
fn r_shows_the_ready_queue_as_tm_ready_does_and_reads_a_write_in_place() {
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
    tui.exp_string("alpha").unwrap();
    press(&mut tui, "r");
    tui.exp_string("Ready").unwrap();
    tui.exp_string("beta").unwrap();
    let linked = tm(&scratch, &repo, &["link", "TM-1", "blocks", "TM-2"]);
    assert!(linked.status.success(), "{linked:?}");
    tui.exp_string("(unblocks").unwrap();
    press(&mut tui, "q");
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

fn status(scratch: &Scratch, repo: &Path, key: &str) -> String {
    let shown = tm(scratch, repo, &["--json", "show", key]);
    let doc: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    doc["data"]["fields"]["status"].as_str().unwrap().to_owned()
}

#[test]
fn triage_moves_an_issue_as_tm_move_does_and_u_undoes_it() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let made = tm(
        &scratch,
        &repo,
        &[
            "create",
            "--type",
            "task",
            "--title",
            "alpha",
            "--set",
            "status=triage",
        ],
    );
    assert!(made.status.success(), "{made:?}");
    let mut tui = sized(&scratch, &repo, "tui");
    tui.exp_string("alpha").unwrap();
    press(&mut tui, "t");
    tui.exp_string("Triage").unwrap();
    press(&mut tui, "s");
    tui.exp_string("Enter choose").unwrap();
    press(&mut tui, "todo\r");
    tui.exp_string("Nothing to triage").unwrap();
    assert_eq!(status(&scratch, &repo, "TM-1"), "todo");
    press(&mut tui, "u");
    tui.exp_string("alpha").unwrap();
    assert_eq!(status(&scratch, &repo, "TM-1"), "triage");
    let history = tm(&scratch, &repo, &["history", "TM-1"]);
    assert!(text(&history.stdout).contains("jace"), "{history:?}");
    press(&mut tui, "q");
    press(&mut tui, "q");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}

/// Waits up to ten seconds for `done`, as the TUI writes between keys.
fn settles(done: impl Fn() -> bool) {
    for _ in 0..100 {
        if done() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("never settled");
}

fn field(scratch: &Scratch, repo: &Path, key: &str, name: &str) -> serde_json::Value {
    let shown = tm(scratch, repo, &["--json", "show", key]);
    let doc: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    doc["data"]["fields"][name].clone()
}

#[test]
fn triage_refuses_a_write_over_a_change_made_elsewhere_and_unsets_with_nobody() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let made = tm(
        &scratch,
        &repo,
        &[
            "create",
            "--type",
            "task",
            "--title",
            "alpha",
            "--set",
            "status=triage",
        ],
    );
    assert!(made.status.success(), "{made:?}");
    let mut tui = sized(&scratch, &repo, "tui");
    tui.exp_string("alpha").unwrap();
    press(&mut tui, "t");
    tui.exp_string("Triage").unwrap();
    press(&mut tui, "p");
    tui.exp_string("Enter choose").unwrap();
    let edited = tm(&scratch, &repo, &["edit", "TM-1", "--set", "priority=low"]);
    assert!(edited.status.success(), "{edited:?}");
    press(&mut tui, "high\r");
    tui.exp_string("so nothing").unwrap();
    assert_eq!(field(&scratch, &repo, "TM-1", "priority"), "low");
    press(&mut tui, "A");
    tui.exp_string("nobody").unwrap();
    press(&mut tui, "kim\r");
    settles(|| field(&scratch, &repo, "TM-1", "assignee") == "kim");
    press(&mut tui, "A");
    tui.exp_string("nobody").unwrap();
    press(&mut tui, "nobody\r");
    tui.exp_string("assignee").unwrap();
    assert_eq!(
        field(&scratch, &repo, "TM-1", "assignee"),
        serde_json::Value::Null
    );
    press(&mut tui, "q");
    press(&mut tui, "q");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}

#[test]
fn the_board_moves_an_issue_a_column_as_tm_move_does_and_u_undoes_it() {
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
    press(&mut tui, "b");
    tui.exp_string("Todo 1").unwrap();
    press(&mut tui, "L");
    settles(|| status(&scratch, &repo, "TM-1") == "in_progress");
    tui.exp_string("In Progress 1").unwrap();
    press(&mut tui, "u");
    settles(|| status(&scratch, &repo, "TM-1") == "todo");
    press(&mut tui, "q");
    press(&mut tui, "q");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}

/// The repo's `.config/tm.toml` with one View, a comment above it.
fn with_a_view(repo: &Path) -> PathBuf {
    let path = repo.join(".config/tm.toml");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("\n# kept as written\n[[views]]\nname = \"Mine\"\nfilter = \"title:alpha\"\n");
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn e_edits_a_view_in_the_editor_and_y_on_its_diff_writes_the_repo_file() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let path = with_a_view(&repo);
    let recorded = scratch.home().join("draft-path");
    let editor = format!(
        "printf %s \"$1\" > '{}'; printf '[[views]]\\nname = \"Ours\"\\nfilter = \"title:alpha\"\\n' >",
        recorded.display()
    );
    let mut tui = sized_with(&scratch, &repo, "tui", &[("VISUAL", editor.as_str())]);
    tui.exp_string("Issues").unwrap();
    press(&mut tui, "v");
    tui.exp_string("Mine").unwrap();
    press(&mut tui, "e");
    // Words are drawn apart, each where it changed.
    tui.exp_string("change?").unwrap();
    tui.exp_string("Ours").unwrap();
    let before = fs::read_to_string(&path).unwrap();
    assert!(
        before.contains("name = \"Mine\""),
        "nothing written before y"
    );
    press(&mut tui, "y");
    settles(|| fs::read_to_string(&path).is_ok_and(|text| text.contains("Ours")));
    let after = fs::read_to_string(&path).unwrap();
    assert!(
        after.contains("# kept as written\n[[views]]\nname = \"Ours\""),
        "{after}"
    );
    let draft = fs::read_to_string(&recorded).unwrap();
    assert_eq!(
        Path::new(&draft).extension(),
        Some("toml".as_ref()),
        "the editor opened a View's draft: {draft}"
    );
    // The draft is removed after the file is written, not with it.
    settles(|| !Path::new(&draft).exists());
    press(&mut tui, "q");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}

#[test]
fn an_agent_is_shown_the_views_but_no_editor_opens_for_one() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let path = with_a_view(&repo);
    let before = fs::read_to_string(&path).unwrap();
    let opened = scratch.home().join("opened");
    let editor = format!("touch '{}'; true", opened.display());
    let mut tui = sized_with(
        &scratch,
        &repo,
        "tui",
        &[("VISUAL", editor.as_str()), ("TM_AGENT", "claude")],
    );
    tui.exp_string("Issues").unwrap();
    press(&mut tui, "v");
    tui.exp_string("Mine").unwrap();
    press(&mut tui, "e");
    tui.exp_string("person").unwrap();
    // Esc and q sent together would read as Alt-q.
    press(&mut tui, "v");
    press(&mut tui, "q");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
    assert!(!opened.exists(), "no editor opened");
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn a_comment_naming_several_actors_asks_which_and_writes_the_one_picked() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    for kim in ["kim@example.com", "kim@other.org"] {
        let mut made = jace(
            &scratch,
            &repo,
            &["create", "--type", "task", "--title", kim],
        );
        made.env("TM_ACTOR", kim);
        assert!(made.output().unwrap().status.success());
    }
    let editor = "printf 'ping @kim' >";
    let mut tui = sized_with(
        &scratch,
        &repo,
        "tui",
        &[("VISUAL", editor), ("EDITOR", "false")],
    );
    tui.exp_string("kim@other.org").unwrap();
    press(&mut tui, "\r");
    tui.exp_string("History").unwrap();
    press(&mut tui, "c");
    // Each word is drawn at its own cell.
    tui.exp_string("@kim").unwrap();
    tui.exp_string("names").unwrap();
    tui.exp_string("leave").unwrap();
    press(&mut tui, "j\r");
    tui.exp_string("commented").unwrap();
    // The list opens on its newest Issue.
    let shown = tm(
        &scratch,
        &repo,
        &["--json", "show", "TM-2", "--comments", "all"],
    );
    let doc: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(
        doc["data"]["comments"][0]["body"], "ping <@kim@other.org>",
        "{doc}"
    );
    press(&mut tui, "qq");
    let rest = tui.exp_eof().unwrap();
    assert!(
        !rest.contains("Which one?"),
        "the CLI's prompt stays held: {rest}"
    );
    assert_eq!(exit_code(&tui), 0);
}

#[test]
fn m_lists_the_mentions_of_you_and_r_marks_one_read() {
    let scratch = Scratch::new();
    let (repo, _store, _project) = initialized(&scratch);
    let mut made = jace(
        &scratch,
        &repo,
        &["create", "--type", "task", "--title", "alpha"],
    );
    made.env("TM_ACTOR", "kim@example.com");
    assert!(made.output().unwrap().status.success());
    let said = tm(
        &scratch,
        &repo,
        &["comment", "TM-1", "@kim@example.com QZXQZX"],
    );
    assert!(said.status.success(), "{said:?}");
    let mut tui = sized_with(&scratch, &repo, "tui", &[("TM_ACTOR", "kim@example.com")]);
    tui.exp_string("unread").unwrap();
    tui.exp_string("(m)").unwrap();
    press(&mut tui, "m");
    tui.exp_string("Notifications").unwrap();
    tui.exp_string("@kim@example.com").unwrap();
    press(&mut tui, "r");
    tui.exp_string("u undo").unwrap();
    let mut listed = jace(&scratch, &repo, &["--json", "notifications"]);
    listed.env("TM_ACTOR", "kim@example.com");
    let doc: serde_json::Value = serde_json::from_slice(&listed.output().unwrap().stdout).unwrap();
    assert_eq!(doc["data"]["notifications"], serde_json::json!([]), "{doc}");
    press(&mut tui, "qq");
    tui.exp_eof().unwrap();
    assert_eq!(exit_code(&tui), 0);
}
