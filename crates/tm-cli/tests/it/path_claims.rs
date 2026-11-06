//! Path Claims warn: the edit hook tells a session about to edit a file
//! another session's Claim covers, and tm prime lists those on its changes.

use std::fs;
use std::path::Path;
use std::process::{Output, Stdio};

use serde_json::{Value, json};
use tm_core::Actor;

use crate::support::{Scratch, git, initialized, piped, text, tm_command};

/// `tm` as the Agent claude in `session`, for jace.
fn in_session(scratch: &Scratch, repo: &Path, session: &str, args: &[&str]) -> Output {
    session_command(scratch, repo, session, args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn session_command(
    scratch: &Scratch,
    repo: &Path,
    session: &str,
    args: &[&str],
) -> std::process::Command {
    let mut command = tm_command();
    command
        .args(args)
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "claude")
        .env("TM_SESSION", session)
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1");
    command
}

/// What the edit hook in `session` adds to the Agent's context before it
/// edits `file`; empty when it says nothing.
fn before_editing(scratch: &Scratch, repo: &Path, session: &str, file: &str) -> String {
    let input =
        json!({ "session_id": session, "tool_name": "Edit", "tool_input": { "file_path": file } });
    hook_says(scratch, repo, session, &input)
}

fn hook_says(scratch: &Scratch, repo: &Path, session: &str, input: &Value) -> String {
    let out = piped(
        session_command(scratch, repo, session, &["hook", "pre-edit", "claude"]),
        input.to_string(),
    );
    assert!(out.status.success(), "{out:?}");
    let said = text(&out.stdout);
    if said.is_empty() {
        return said;
    }
    let shown: Value = serde_json::from_str(&said).unwrap();
    assert_eq!(shown["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    shown["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn another_sessions_claim_warns_once_before_an_edit_and_in_prime() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["by path", "by branch"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    let repo = fs::canonicalize(&repo).unwrap();
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("src/lib.rs"), "a\n").unwrap();
    git(&scratch, &repo, &["add", "--all"]);
    git(&scratch, &repo, &["commit", "--quiet", "-m", "src"]);
    git(&scratch, &repo, &["checkout", "--quiet", "-b", "feat/docs"]);
    fs::write(repo.join("AGENTS.md"), "agents, more\n").unwrap();
    git(&scratch, &repo, &["commit", "--quiet", "-am", "docs"]);
    let took = in_session(
        &scratch,
        &repo,
        "s1",
        &["claim", "TM-1", "--paths", "src/**"],
    );
    assert!(took.status.success(), "{took:?}");
    let took = in_session(&scratch, &repo, "s1", &["claim", "TM-2"]);
    assert!(took.status.success(), "{took:?}");
    git(&scratch, &repo, &["checkout", "--quiet", "main"]);

    let at = |file: &str| repo.join(file).display().to_string();
    let said = before_editing(&scratch, &repo, "s2", &at("src/lib.rs"));
    assert_eq!(
        said,
        "tm: TM-1's Claim by claude (session s1) for jace covers src/lib.rs through src/**; \
         ask its holder or tm show TM-1 before editing it"
    );
    assert_eq!(
        before_editing(&scratch, &repo, "s2", &at("src/lib.rs")),
        "",
        "told once per session"
    );
    let said = before_editing(&scratch, &repo, "s2", &at("AGENTS.md"));
    assert!(
        said.contains(
            "TM-1's Claim by claude (session s1) for jace covers AGENTS.md, committed on feat/docs"
        ),
        "{said}"
    );
    assert!(
        !said.contains("TM-2"),
        "one session's branch is told once: {said}"
    );
    for quiet in [at("README.md"), "/etc/hosts".to_owned()] {
        assert_eq!(before_editing(&scratch, &repo, "s3", &quiet), "", "{quiet}");
    }
    let said = before_editing(&scratch, &repo, "s3", &at("src/new/mod.rs"));
    assert!(
        said.contains("covers src/new/mod.rs through src/**"),
        "a file not written yet: {said}"
    );
    assert_eq!(
        before_editing(&scratch, &repo, "s1", &at("src/lib.rs")),
        "",
        "a session's own Claim never warns it"
    );

    fs::write(repo.join("src/lib.rs"), "a, changed\n").unwrap();
    fs::write(repo.join("AGENTS.md"), "agents, changed\n").unwrap();
    let primed = in_session(&scratch, &repo, "s2", &["prime"]);
    let primed = text(&primed.stdout);
    assert!(
        primed.contains(
            "Path Claims on your changes (1):\n  TM-1  claude (session s1) for jace covers AGENTS.md, src/lib.rs\n"
        ),
        "{primed}"
    );
    let primed = in_session(&scratch, &repo, "s2", &["--json", "prime"]);
    let doc: Value = serde_json::from_slice(&primed.stdout).unwrap();
    assert_eq!(
        doc["data"]["path_claims"][0]["files"],
        json!([
            { "file": "AGENTS.md", "branch": "feat/docs" },
            { "file": "src/lib.rs", "path": "src/**" },
        ])
    );
    let own = in_session(&scratch, &repo, "s1", &["--json", "prime"]);
    let doc: Value = serde_json::from_slice(&own.stdout).unwrap();
    assert_eq!(
        doc["data"]["path_claims"],
        json!([]),
        "its own Claims are not listed"
    );
}

#[test]
fn the_edit_hook_is_silent_outside_a_workspace_and_without_input() {
    let scratch = Scratch::new();
    let outside = scratch.0.join("outside");
    fs::create_dir_all(&outside).unwrap();
    let input = json!({ "tool_input": { "file_path": "/etc/hosts" } }).to_string();
    for stdin in [input.as_str(), ""] {
        let out = piped(
            session_command(&scratch, &outside, "s1", &["hook", "pre-edit", "claude"]),
            stdin,
        );
        assert!(out.status.success(), "{out:?}");
        assert_eq!(text(&out.stdout), "", "{stdin:?}");
    }
}

#[test]
fn the_edit_hook_reads_no_email_for_an_agent() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "docs")
        .unwrap();
    drop(store);
    let repo = fs::canonicalize(&repo).unwrap();
    let took = in_session(
        &scratch,
        &repo,
        "s1",
        &["claim", "TM-1", "--paths", "README.md"],
    );
    assert!(took.status.success(), "{took:?}");
    let (bin, log) = scratch.logged_git();
    let file = repo.join("README.md").display().to_string();
    let input =
        json!({ "session_id": "s2", "tool_name": "Edit", "tool_input": { "file_path": file } });
    let mut command = session_command(&scratch, &repo, "s2", &["hook", "pre-edit", "claude"]);
    command.env_remove("TM_ACTOR").env("PATH", &bin);
    let out = piped(command, input.to_string());
    assert!(out.status.success(), "{out:?}");
    assert!(text(&out.stdout).contains("TM-1's Claim"), "{out:?}");
    let ran = fs::read_to_string(&log).unwrap_or_default();
    assert!(!ran.contains("user.email"), "{ran}");
}

/// TM-1 claimed by s1 on `src/lib.rs`; TM-2 by s2 and TM-3 by s4 on
/// `feat/x`, which commits `café.rs` and `nb/a.ipynb`.
fn three_claims(scratch: &Scratch) -> std::path::PathBuf {
    let (repo, mut store, project) = initialized(scratch);
    for title in ["exact", "branch", "same branch", "vendored", "later"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    let repo = fs::canonicalize(&repo).unwrap();
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("src/lib.rs"), "a\n").unwrap();
    git(scratch, &repo, &["add", "--all"]);
    git(scratch, &repo, &["commit", "--quiet", "-m", "src"]);
    git(scratch, &repo, &["checkout", "--quiet", "-b", "feat/x"]);
    fs::create_dir_all(repo.join("nb")).unwrap();
    fs::write(repo.join("café.rs"), "c\n").unwrap();
    fs::write(repo.join("nb/a.ipynb"), "{}\n").unwrap();
    git(scratch, &repo, &["add", "--all"]);
    git(scratch, &repo, &["commit", "--quiet", "-m", "x"]);
    for (session, issue) in [("s2", "TM-2"), ("s4", "TM-3")] {
        let took = in_session(scratch, &repo, session, &["claim", issue]);
        assert!(took.status.success(), "{took:?}");
    }
    git(scratch, &repo, &["checkout", "--quiet", "main"]);
    let took = in_session(
        scratch,
        &repo,
        "s1",
        &["claim", "TM-1", "--paths", "src/lib.rs"],
    );
    assert!(took.status.success(), "{took:?}");
    repo
}

#[test]
fn sub_agents_handed_claims_other_worktrees_and_any_file_name_warn() {
    let scratch = Scratch::new();
    let repo = three_claims(&scratch);
    let at = |file: &str| repo.join(file).display().to_string();

    let sub_agent = json!({
        "session_id": "s1",
        "agent_id": "abcd1234-0000",
        "agent_type": "general-purpose",
        "tool_name": "Edit",
        "tool_input": { "file_path": at("src/lib.rs") },
    });
    assert_eq!(
        hook_says(&scratch, &repo, "s1", &sub_agent),
        "tm: TM-1's Claim by claude (session s1) for jace covers src/lib.rs; \
         ask its holder or tm show TM-1 before editing it",
        "a sub-agent is warned of its parent's Claim, with no glob to name"
    );
    assert_eq!(before_editing(&scratch, &repo, "s1", &at("src/lib.rs")), "");

    let relative = json!({
        "session_id": "s3",
        "cwd": repo.join("src"),
        "tool_name": "Write",
        "tool_input": { "file_path": "lib.rs" },
    });
    assert!(
        hook_says(&scratch, &repo, "s3", &relative).contains("TM-1's Claim"),
        "a relative path is read from the hook's cwd"
    );
    let wt = scratch.0.join("wt");
    git(
        &scratch,
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "side",
            wt.to_str().unwrap(),
        ],
    );
    let said = before_editing(
        &scratch,
        &repo,
        "s5",
        &wt.join("src/lib.rs").display().to_string(),
    );
    assert!(said.contains("TM-1's Claim"), "another worktree: {said}");

    let said = before_editing(&scratch, &repo, "s6", &at("café.rs"));
    assert!(
        said.contains(
            "TM-2's Claim by claude (session s2) for jace covers café.rs, committed on feat/x"
        ) && said.contains("TM-3's Claim by claude (session s4) for jace covers café.rs"),
        "each session holding the branch, by the file's own name: {said}"
    );
    let notebook = json!({
        "session_id": "s7",
        "tool_name": "NotebookEdit",
        "tool_input": { "notebook_path": at("nb/a.ipynb") },
    });
    assert!(hook_says(&scratch, &repo, "s7", &notebook).contains("covers nb/a.ipynb"));

    let handed = in_session(
        &scratch,
        &repo,
        "s1",
        &["handoff", "TM-1", "--to", "codex", "over to you"],
    );
    assert!(handed.status.success(), "{handed:?}");
    let said = before_editing(&scratch, &repo, "s1", &at("src/lib.rs"));
    assert!(
        said.contains("TM-1's Claim by codex"),
        "a handed Claim warns until it is taken up: {said}"
    );
}

#[test]
fn prime_lists_committed_and_untracked_changes_and_says_what_it_cannot_check() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    for title in ["paths", "branch"] {
        store
            .create_issue(&Actor::human("jace"), project, "task", title)
            .unwrap();
    }
    drop(store);
    let repo = fs::canonicalize(&repo).unwrap();
    let took = in_session(
        &scratch,
        &repo,
        "s1",
        &["claim", "TM-1", "--paths", "src/**"],
    );
    assert!(took.status.success(), "{took:?}");
    git(&scratch, &repo, &["checkout", "--quiet", "-b", "feat/y"]);
    fs::create_dir_all(repo.join("src")).unwrap();
    for file in ["src/a.rs", "src/b.rs"] {
        fs::write(repo.join(file), "x\n").unwrap();
    }
    git(&scratch, &repo, &["add", "--all"]);
    git(&scratch, &repo, &["commit", "--quiet", "-m", "y"]);
    for file in ["src/c.rs", "src/d.rs"] {
        fs::write(repo.join(file), "x\n").unwrap();
    }

    let primed = text(&in_session(&scratch, &repo, "s2", &["prime"]).stdout);
    assert!(
        primed.contains(
            "  TM-1  claude (session s1) for jace covers src/a.rs, src/b.rs, src/c.rs, and 1 more\n"
        ),
        "{primed}"
    );
    let primed = in_session(&scratch, &repo, "s2", &["--json", "prime"]);
    let doc: Value = serde_json::from_slice(&primed.stdout).unwrap();
    let files: Vec<&str> = doc["data"]["path_claims"][0]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["file"].as_str().unwrap())
        .collect();
    assert_eq!(files, ["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs"]);

    let took = in_session(&scratch, &repo, "s1", &["claim", "TM-2"]);
    assert!(took.status.success(), "{took:?}");
    git(&scratch, &repo, &["branch", "-m", "main", "trunk"]);
    let said = before_editing(
        &scratch,
        &repo,
        "s2",
        &repo.join("README.md").display().to_string(),
    );
    assert!(
        said.starts_with("tm: cannot tell what is committed off the default branch: ")
            && said.contains("no origin/HEAD and no main or master branch")
            && !said.contains('\n'),
        "{said}"
    );
    assert_eq!(
        before_editing(
            &scratch,
            &repo,
            "s2",
            &repo.join("notes.md").display().to_string()
        ),
        "",
        "told once"
    );
    let primed = text(&in_session(&scratch, &repo, "s2", &["prime"]).stdout);
    assert!(
        primed
            .matches("warning: Path Claims: cannot tell what is committed off the default branch: ")
            .count()
            == 1
            && !primed.contains("cannot list your committed changes"),
        "one warning for one cause: {primed}"
    );

    let released = in_session(&scratch, &repo, "s1", &["release", "TM-2"]);
    assert!(released.status.success(), "{released:?}");
    let at = |file: &str| repo.join(file).display().to_string();
    assert!(before_editing(&scratch, &repo, "s4", &at("README.md")).starts_with("tm: cannot tell"));
    for file in ["src/a.rs", "README.md", "src/a.rs", "README.md"] {
        let said = before_editing(&scratch, &repo, "s4", &at(file));
        assert!(
            !said.contains("cannot tell"),
            "a file a path covers checks no branch, so does not forget it: {file}: {said}"
        );
    }
}

#[test]
fn a_file_is_placed_by_the_checkout_it_lies_in_and_a_new_claim_is_told() {
    let scratch = Scratch::new();
    let repo = three_claims(&scratch);
    let at = |file: &str| repo.join(file).display().to_string();
    let took = in_session(
        &scratch,
        &repo,
        "s8",
        &["claim", "TM-4", "--paths", "vendor/**"],
    );
    assert!(took.status.success(), "{took:?}");

    let nested = repo.join(".claude/worktrees/n");
    git(
        &scratch,
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "nested",
            nested.to_str().unwrap(),
        ],
    );
    let said = before_editing(
        &scratch,
        &repo,
        "s5",
        &nested.join("src/lib.rs").display().to_string(),
    );
    assert!(
        said.contains("TM-1's Claim"),
        "a worktree inside the checkout: {said}"
    );
    let vendored = repo.join("vendor/lib");
    fs::create_dir_all(&vendored).unwrap();
    git(&scratch, &vendored, &["init", "--quiet"]);
    assert_eq!(
        before_editing(
            &scratch,
            &repo,
            "s9",
            &vendored.join("x.rs").display().to_string()
        ),
        "",
        "a repository inside the checkout is not part of it"
    );
    assert!(before_editing(&scratch, &repo, "s9", &at("vendor/plain.rs")).contains("TM-4's Claim"));
    let other = scratch.0.join("other");
    fs::create_dir_all(other.join("src")).unwrap();
    git(&scratch, &other, &["init", "--quiet"]);
    assert_eq!(
        before_editing(
            &scratch,
            &repo,
            "s9",
            &other.join("src/lib.rs").display().to_string()
        ),
        "",
        "another repository with the same path"
    );

    assert!(before_editing(&scratch, &repo, "s12", &at("src/lib.rs")).contains("TM-1's Claim"));
    let took = in_session(
        &scratch,
        &repo,
        "s13",
        &["claim", "TM-5", "--paths", "src/**"],
    );
    assert!(took.status.success(), "{took:?}");
    let said = before_editing(&scratch, &repo, "s12", &at("src/lib.rs"));
    assert!(
        said.contains("TM-5's Claim") && !said.contains("TM-1's Claim"),
        "a Claim taking a file later is told, the one told before is not: {said}"
    );

    let feat = repo.join(".git/refs/heads/feat/x");
    let good = fs::read_to_string(&feat).unwrap();
    fs::write(&feat, "garbage\n").unwrap();
    let said = before_editing(&scratch, &repo, "s10", &at("README.md"));
    assert!(
        said.starts_with("tm: cannot tell which files feat/x commits: ") && said.contains("broken"),
        "{said}"
    );
    for file in ["/etc/hosts".to_owned(), at("README.md")] {
        assert_eq!(
            before_editing(&scratch, &repo, "s10", &file),
            "",
            "an edit that checks no branch does not forget it: {file}"
        );
    }
    fs::write(&feat, &good).unwrap();
    assert_eq!(before_editing(&scratch, &repo, "s10", &at("README.md")), "");
    fs::write(&feat, "garbage\n").unwrap();
    assert!(
        before_editing(&scratch, &repo, "s10", &at("README.md")).starts_with("tm: cannot tell"),
        "told again once the check came back and failed again"
    );
    fs::write(&feat, format!("{}\n", "1".repeat(40))).unwrap();
    let said = before_editing(&scratch, &repo, "s14", &at("README.md"));
    assert!(
        said.starts_with("tm: cannot tell which files feat/x commits: "),
        "a branch to a missing commit: {said}"
    );
    fs::remove_file(&feat).unwrap();
    assert_eq!(
        before_editing(&scratch, &repo, "s11", &at("README.md")),
        "",
        "a branch this clone does not have covers nothing"
    );
}

#[test]
fn an_edit_hook_error_is_told_once_whatever_the_heartbeat_does() {
    let scratch = Scratch::new();
    let repo = three_claims(&scratch);
    let unreadable =
        json!({ "session_id": "s2", "tool_name": "Edit", "tool_input": { "path": "x" } });
    assert_eq!(
        hook_says(&scratch, &repo, "s2", &unreadable),
        "tm: cannot find the edited file in the hook's input (tool_input holds path), \
         so Path Claims go unchecked"
    );
    let beat = piped(
        session_command(&scratch, &repo, "s2", &["hook", "heartbeat", "claude"]),
        json!({ "session_id": "s2" }).to_string(),
    );
    assert!(beat.status.success(), "{beat:?}");
    assert_eq!(
        hook_says(&scratch, &repo, "s2", &unreadable),
        "",
        "a heartbeat's success does not forget the edit hook's error"
    );
    before_editing(
        &scratch,
        &repo,
        "s2",
        &repo.join("README.md").display().to_string(),
    );
    assert!(
        hook_says(&scratch, &repo, "s2", &unreadable).starts_with("tm: cannot find"),
        "the edit hook's own success does"
    );
}
