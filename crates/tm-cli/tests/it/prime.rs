//! `tm prime`: the context a `SessionStart` hook gives an Agent session.

use std::fs;
use std::path::Path;
use std::process::Output;
use std::time::Instant;

use serde_json::{Value, json};
use tm_core::{Actor, Change, ProjectId};
use tm_store::workspace::{PathProject, RepoFile};
use tm_store::{Draft, Store};
use uuid::Uuid;

use crate::support::{Scratch, initialized, text, tm, tm_command};

/// Prime for a 200-Issue Workspace, every title at its cut, fits in 4.5 KB,
/// about 1.1k tokens: TM-10's 4 KB, plus room for the habit lines.
const BUDGET_BYTES: usize = 4608;

/// `tm` as the Agent `claude` writing for `jace`.
fn as_claude(scratch: &Scratch, repo: &Path, args: &[&str]) -> Output {
    as_agent(scratch, repo, "claude", &[], args)
}

/// `tm` as the Agent `agent` writing for `jace`, with `env` set.
fn as_agent(
    scratch: &Scratch,
    repo: &Path,
    agent: &str,
    env: &[(&str, &str)],
    args: &[&str],
) -> Output {
    let mut command = tm_command();
    command
        .args(args)
        .current_dir(repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", agent)
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn set(store: &mut Store, project: ProjectId, issue: Uuid, field: &str, value: Value) {
    store
        .append(Draft {
            actor: Actor::human("jace"),
            project,
            subject: issue,
            change: Change::SetField {
                field: field.into(),
                label: String::new(),
                value,
                value_label: None,
            },
        })
        .unwrap();
}

/// The indented lines under `heading` in prime's text.
fn under<'a>(shown: &'a str, heading: &str) -> Vec<&'a str> {
    shown
        .lines()
        .skip_while(|line| *line != heading)
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .collect()
}

/// Short-lists naming claude at their fullest as prime shows them: more
/// lists than it shows, names at their longest, and the members above and
/// after the first that can start passed over for every reason at once.
fn fill_lists(store: &mut Store, project: ProjectId) {
    let jace = Actor::human("jace");
    let codex = Actor::agent("codex", "jace");
    let ready: Vec<Uuid> = store.ready().unwrap().iter().map(|r| r.issue).collect();
    let blocker = *ready.last().unwrap();
    for (n, members) in (1..).zip(ready.chunks(10).take(4)) {
        let name = format!("{n}-{}", "x".repeat(62));
        let actors = ["claude".to_owned()];
        let (list, _) = store
            .create_shortlist(&jace, project, &name, &actors, None)
            .unwrap();
        store
            .shortlist_add(&jace, list, members, tm_store::Place::End)
            .unwrap();
        for marked in [members[0], members[1], members[3], members[4]] {
            store.take(&codex, marked, None).unwrap();
            store.link(&jace, "blocks", blocker, marked).unwrap();
            set(store, project, marked, "type", json!("epic"));
            set(store, project, marked, "archived", json!(true));
        }
    }
}

/// List names, marks and the counts above are cut, and lists stop at their
/// share of the budget: two of [`fill_lists`]'s four fit.
fn lists_are_cut(shown: &str) {
    assert!(!shown.contains(&"x".repeat(62)), "{shown}");
    assert!(shown.contains("…  [blocked +3]\n"), "{shown}");
    let above = "2 above it passed over: 2 archived, 2 blocked, 2 clai…\n";
    assert!(shown.contains(above), "{shown}");
    let more = "  and 2 more lists: tm shortlist\n";
    assert!(shown.contains(more), "{shown}");
}

#[test]
fn prime_for_200_issues_fits_the_budget_and_keeps_readys_order() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let claude = Actor::agent("claude", "jace");
    let jace = Actor::human("jace");
    for n in 1..=200 {
        // Longer than any title prime prints whole: ASCII with a character it
        // escapes, or four-byte emoji, fewer characters than the cap's bytes.
        let title = if n % 2 == 0 {
            format!("Issue {n} {}", "😀".repeat(40))
        } else {
            format!("Issue {n} \u{202e}{}", "long title ".repeat(27))
        };
        let (issue, _) = store.create_issue(&jace, project, "task", &title).unwrap();
        if n % 16 == 0 {
            store.take(&claude, issue, None).unwrap();
        }
        if n % 10 == 5 {
            set(&mut store, project, issue, "status", json!("in_progress"));
        }
    }
    fill_lists(&mut store, project);
    drop(store);

    let started = Instant::now();
    let out = as_claude(&scratch, &repo, &["prime"]);
    let took = started.elapsed();
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    assert!(
        shown.len() <= BUDGET_BYTES,
        "{} bytes over the budget:\n{shown}",
        shown.len()
    );
    eprintln!(
        "tm prime over 200 Issues: {} bytes in {took:?}",
        shown.len()
    );

    let ready = text(&as_claude(&scratch, &repo, &["ready"]).stdout);
    assert!(
        ready.lines().all(|line| !line.contains('…')),
        "only prime cuts titles: {ready}"
    );
    let key = |line: &str| line.split_whitespace().next().unwrap_or("").to_owned();
    // Both lead with the session's lists; with them prime shows 3 ready, in
    // ready's order, passing over those tm ready marks as on a list.
    let queue = |text: &str| -> Vec<String> {
        let lines = text.lines().skip_while(|line| !line.starts_with("Ready ("));
        let open = lines.skip(1).filter(|line| !line.contains("  [on "));
        open.take(3).map(key).collect()
    };
    assert_eq!(queue(&shown), queue(&ready));
    assert!(shown.contains("\nShort-lists ("), "{shown}");
    assert!(ready.starts_with("Short-lists ("), "{ready}");
    let title = |key: &str| {
        let at = |line: &&str| line.starts_with(&format!("  {key}"));
        let line = shown.lines().find(at).unwrap_or("");
        line.trim_start()
            .splitn(4, "  ")
            .nth(3)
            .unwrap_or("")
            .to_owned()
    };
    // The first ready Issues, one titled in escapes and one in emoji, are
    // cut on a character boundary.
    let titles: Vec<String> = queue(&shown)
        .iter()
        .map(|key| title(&format!("{key} ")))
        .collect();
    for title in &titles {
        assert!(title.len() <= 64 && title.ends_with('…'), "{title}");
    }
    let kinds = titles.concat();
    let both = kinds.contains(r"\u{202e}") && kinds.contains('😀');
    assert!(both, "{titles:?}");
    lists_are_cut(&shown);
    assert!(shown.contains("Ready (152):\n"), "{shown}");
    assert!(shown.contains(PASSED), "{shown}");
    assert!(shown.contains("  and 125 more: tm ready\n"), "{shown}");
    a_session_without_lists_fits_too(&scratch, &repo);
    let active: Vec<String> = under(&shown, "In progress (20):")
        .into_iter()
        .map(key)
        .collect();
    assert_eq!(
        active,
        [
            "TM-195", "TM-185", "TM-175", "TM-165", "TM-155", "TM-145", "TM-135", "TM-125",
            "TM-115", "TM-105", "and"
        ],
        "most recently started first: {shown}"
    );
    assert!(
        shown.contains("  and 10 more: tm list category:active\n"),
        "{shown}"
    );
    let claims = under(&shown, "Your Claims:");
    assert_eq!(claims.len(), 11, "{shown}");
    assert_eq!(claims[10], "  and 2 more: tm --json prime lists them all");
    assert!(
        shown.starts_with("tm: Project TM, writing as claude for jace\n"),
        "{shown}"
    );
    json_lists_every_claim_and_the_first_active(&scratch, &repo);
}

/// The ready Issues on [`fill_lists`]'s lists, which prime counts.
const PASSED: &str = "  24 on Short-lists, passed over: tm ready marks them\n";

/// Prime over the 200-Issue Workspace for codex, whom no list names: 15
/// ready Issues in place of the lists, within the budget.
fn a_session_without_lists_fits_too(scratch: &Scratch, repo: &Path) {
    let theirs = text(&as_agent(scratch, repo, "codex", &[], &["prime"]).stdout);
    assert!(!theirs.contains("Short-lists ("), "{theirs}");
    assert!(theirs.contains(PASSED), "{theirs}");
    assert!(theirs.contains("  and 113 more: tm ready\n"), "{theirs}");
    assert!(
        theirs.len() <= BUDGET_BYTES,
        "{} bytes over the budget:\n{theirs}",
        theirs.len()
    );
}

/// `--json` prime over the 200-Issue Workspace: every Claim, and the first
/// ten in-progress Issues with their total.
fn json_lists_every_claim_and_the_first_active(scratch: &Scratch, repo: &Path) {
    let out = as_claude(scratch, repo, &["--json", "prime"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["claims"].as_array().map(Vec::len), Some(12));
    let active = &doc["data"]["in_progress"];
    let keys: Vec<&Value> = active["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| &issue["key"])
        .collect();
    assert_eq!(
        (&active["total"], keys.len(), keys[0]),
        (&json!(20), 10, &json!("TM-195"))
    );
}

#[test]
fn in_progress_lists_every_active_status_by_last_update() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    let [started, reviewed, shelved, finished] = ["started", "reviewed", "shelved", "finished"]
        .map(|title| store.create_issue(&jace, project, "task", title).unwrap().0);
    for (issue, field, value) in [
        (started, "status", json!("in_progress")),
        (reviewed, "status", json!("in_review")),
        (shelved, "status", json!("in_progress")),
        (shelved, "archived", json!(true)),
        (finished, "status", json!("done")),
        (started, "priority", json!("high")),
    ] {
        set(&mut store, project, issue, field, value);
    }
    drop(store);

    let shown = text(&as_claude(&scratch, &repo, &["prime"]).stdout);
    assert!(
        shown.contains(
            "In progress (2):\n  TM-1  in_progress  high  started\n  TM-2  in_review  none  reviewed\n\
             Ready (0):\n  nothing is ready\n"
        ),
        "the last updated first, not the last created: {shown}"
    );
}

#[test]
fn a_session_writing_as_a_human_is_warned_in_the_text_an_agent_reads() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let out = tm(&scratch, &repo, &["prime"]);
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    assert!(
        shown.starts_with(
            "warning: this session writes as jace; an Agent sets TM_AGENT to its name\n"
        ),
        "{shown}"
    );
    assert!(shown.contains("Your Claims:\n  none\n"), "{shown}");

    let shown = text(&as_claude(&scratch, &repo, &["prime"]).stdout);
    assert!(!shown.contains("warning:"), "{shown}");
}

#[test]
fn prime_without_an_identity_still_gives_the_scheme_and_ready_queue() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    store
        .create_issue(&Actor::human("jace"), project, "task", "first")
        .unwrap();
    drop(store);
    let out = tm_command()
        .args(["prime"])
        .current_dir(&repo)
        .env_remove("TM_ACTOR")
        .env_remove("TM_AGENT")
        .env("TM_HOME", scratch.home())
        .env("GIT_CONFIG_GLOBAL", scratch.git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    assert!(shown.starts_with("warning: no identity: "), "{shown}");
    assert!(shown.contains("writing as nobody\n"), "{shown}");
    assert!(
        shown.contains("In progress (0):\n  none\nReady (1):\n  TM-1  todo  none  first\n"),
        "{shown}"
    );
    assert!(
        shown.ends_with(
            "Habits (tm guide has the rest):\n  tm start KEY when work starts\n  \
             after a merge: git fetch, then tm git scan moves what it closed to done\n  \
             your writes renew your Claims; tm setup claude adds hooks that heartbeat between them\n  \
             the description holds the current plan; tm edit changes it\n  \
             tm comment KEY for progress, what was learned and changes of plan\n"
        ),
        "{shown}"
    );
}

#[test]
fn a_claims_time_left_prints_and_a_lapsed_ones_time_is_local() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let (issue, _) = store
        .create_issue(&Actor::human("jace"), project, "task", "held")
        .unwrap();
    let (gone, _) = store
        .create_issue(&Actor::human("jace"), project, "task", "lapsed")
        .unwrap();
    let held = store
        .take(&Actor::agent("claude", "jace"), issue, Some(3_600_000))
        .unwrap();
    let claim = store
        .take(&Actor::agent("claude", "jace"), gone, Some(1))
        .unwrap();
    let (forever, _) = store
        .create_issue(&Actor::human("jace"), project, "task", "kept")
        .unwrap();
    let kept = store
        .take(&Actor::agent("claude", "jace"), forever, None)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    drop(store);
    let out = tm_command()
        .args(["prime"])
        .current_dir(&repo)
        .env("TM_HOME", scratch.home())
        .env("TM_ACTOR", "jace")
        .env("TM_AGENT", "claude")
        .env("TZ", "America/Chicago")
        .output()
        .unwrap();
    let expires = jiff::Timestamp::from_millisecond(claim.expires_ms.unwrap())
        .unwrap()
        .in_tz("America/Chicago")
        .unwrap()
        .strftime("%Y-%m-%d %H:%M %Z")
        .to_string();
    let shown = text(&out.stdout);
    for line in [
        format!(
            "  TM-1  todo  none  held  (Claim {}, expires in 1h)\n",
            held.id
        ),
        format!(
            "  TM-2  todo  none  lapsed  (Claim {}, lapsed {expires})\n",
            claim.id
        ),
        format!("  TM-3  todo  none  kept  (Claim {}, no expiry)\n", kept.id),
    ] {
        assert!(shown.contains(&line), "{line:?} in {shown}");
    }
}

#[test]
fn a_directory_mapped_to_a_missing_project_is_a_warning() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let path = repo.join(".config/tm.toml");
    let mut file = RepoFile::parse(&fs::read_to_string(&path).unwrap()).unwrap();
    file.paths.push(PathProject {
        prefix: "gone".into(),
        project: "GONE".into(),
    });
    fs::write(&path, file.render()).unwrap();
    fs::create_dir_all(repo.join("gone")).unwrap();

    let out = as_claude(&scratch, &repo.join("gone"), &["prime"]);
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    assert!(
        shown.starts_with("warning: ") && shown.contains("Project GONE, which this Workspace"),
        "{shown}"
    );
    assert!(
        shown.contains("tm: Project -, writing as claude for jace\n"),
        "{shown}"
    );
}

#[test]
fn json_prime_keeps_warnings_out_of_the_data() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let out = tm(&scratch, &repo, &["--json", "prime"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        doc["warnings"][0],
        "this session writes as jace; an Agent sets TM_AGENT to its name"
    );
    assert_eq!(doc["data"]["project"], "TM");
    assert_eq!(doc["data"]["ready"]["total"], 0);
    let shown = text(&tm(&scratch, &repo, &["prime"]).stdout);
    let habits: Vec<String> = under(&shown, "Habits (tm guide has the rest):")
        .iter()
        .map(|line| line.trim().to_owned())
        .collect();
    assert_eq!(doc["data"]["habits"], json!(habits));
}

/// A binding left set after its list is gone is a warning in tm ready too,
/// as tm next refuses it.
#[test]
fn ready_warns_of_a_binding_when_the_project_has_no_lists() {
    let scratch = Scratch::new();
    let (repo, _store, _) = initialized(&scratch);
    let nope = [("TM_SHORTLIST", "nope")];
    let out = as_agent(&scratch, &repo, "claude", &nope, &["--json", "ready"]);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let warned = doc["warnings"][0].as_str().unwrap_or_default();
    assert!(warned.starts_with("TM_SHORTLIST names nope"), "{doc}");

    // So is one in a directory mapped to a Project the Workspace lacks.
    let file = repo.join(".config/tm.toml");
    let mapped = fs::read_to_string(&file).unwrap();
    fs::write(&file, mapped.replace("\"TM\"", "\"NOPE\"")).unwrap();
    let out = as_agent(&scratch, &repo, "claude", &nope, &["--json", "ready"]);
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let warned = doc["warnings"][0].as_str().unwrap_or_default();
    assert!(warned.starts_with("no Short-lists: "), "{doc}");
    let quiet = as_agent(&scratch, &repo, "claude", &[], &["--json", "ready"]);
    let doc: Value = serde_json::from_slice(&quiet.stdout).unwrap();
    assert_eq!(doc["warnings"], json!([]), "{doc}");
}

/// The list tm next takes from shows even past the lists prime shows, its
/// Issue alone marked next; another list's first that can start is then.
#[test]
fn prime_marks_and_shows_the_list_tm_next_takes_from() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    let actors = ["claude".to_owned()];
    let mut lists = Vec::new();
    for name in ["a", "b", "c", "d", "e"] {
        let (issue, _) = store.create_issue(&jace, project, "task", name).unwrap();
        let (list, _) = store
            .create_shortlist(&jace, project, name, &actors, None)
            .unwrap();
        store
            .shortlist_add(&jace, list, &[issue], tm_store::Place::End)
            .unwrap();
        lists.push(issue);
    }
    let started = json!("in_progress");
    for issue in &lists[..3] {
        set(&mut store, project, *issue, "status", started.clone());
    }
    drop(store);

    let shown = text(&as_claude(&scratch, &repo, &["prime"]).stdout);
    assert!(shown.contains("\n  d (1 Issue)\n    next  TM-4"), "{shown}");
    assert!(!shown.contains("  e ("), "{shown}");
    // Three lists show in all, the one tm next takes from among them.
    let more = "  and 2 more lists: tm shortlist\n";
    assert!(shown.contains(more), "{shown}");
}

/// ADR 0016: prime and ready lead with the session's lists, each from the
/// Issue tm next would take, members it passes over marked.
#[test]
fn prime_and_ready_lead_with_the_sessions_lists() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    let ids: Vec<Uuid> = ["a", "b", "c", "d", "e"]
        .into_iter()
        .map(|title| store.create_issue(&jace, project, "task", title).unwrap().0)
        .collect();
    let actors = ["claude".to_owned()];
    let (docs, later) = (&ids[..4], &ids[4..]);
    for (name, members) in [("docs", docs), ("empty", &[][..]), ("later", later)] {
        let (list, _) = store
            .create_shortlist(&jace, project, name, &actors, None)
            .unwrap();
        if !members.is_empty() {
            store
                .shortlist_add(&jace, list, members, tm_store::Place::End)
                .unwrap();
        }
    }
    // Members that began or became blocked after joining stay, marked.
    set(&mut store, project, ids[0], "status", json!("in_progress"));
    store.link(&jace, "blocks", ids[4], ids[2]).unwrap();
    drop(store);

    let shown = text(&as_claude(&scratch, &repo, &["prime"]).stdout);
    let heading = "Short-lists (tm next takes the first that can start):";
    let lists = under(&shown, heading);
    let expected = [
        "  docs (4 Issues)",
        "    1 above it passed over: 1 started",
        "    next  TM-2  todo  none  b",
        "          TM-3  todo  none  c  [blocked]",
        "          TM-4  todo  none  d",
        "  empty: no Issues",
        "  later (1 Issue)",
        "    then  TM-5  todo  none  e",
    ];
    assert_eq!(lists, expected, "{shown}");
    let doc: Value =
        serde_json::from_slice(&as_claude(&scratch, &repo, &["--json", "prime"]).stdout).unwrap();
    let docs = &doc["data"]["shortlists"][0];
    assert_eq!(docs["first"]["key"], "TM-2");
    assert_eq!(docs["next"], true);
    let ready = text(&as_claude(&scratch, &repo, &["ready"]).stdout);
    assert!(ready.starts_with("Short-lists ("), "{ready}");
    // A session no list names sees which ready Issues tm next passes over:
    // tm ready marks them, and prime counts them instead of showing them.
    let theirs = |args: &[&str]| text(&as_agent(&scratch, &repo, "codex", &[], args).stdout);
    let ready = theirs(&["ready"]);
    assert!(
        ready.contains("TM-2  todo  none  b  [on docs]\n"),
        "{ready}"
    );
    assert!(
        ready.contains("TM-5  todo  none  e  (unblocks TM-3)  [on later]\n"),
        "{ready}"
    );
    let shown = theirs(&["prime"]);
    let section = "Ready (3):\n  3 on Short-lists, passed over: tm ready marks them\nHabits";
    assert!(shown.contains(section), "{shown}");
    let doc: Value = serde_json::from_str(&theirs(&["--json", "prime"])).unwrap();
    assert_eq!(doc["data"]["ready"]["on_shortlists"], 3, "{doc}");
    assert_eq!(doc["data"]["ready"]["issues"], json!([]), "{doc}");
    let doc: Value =
        serde_json::from_slice(&as_claude(&scratch, &repo, &["--json", "ready"]).stdout).unwrap();
    assert_eq!(doc["data"]["shortlists"][0]["first"]["key"], "TM-2");
    // A session that cannot say who it is is told why it sees no lists.
    let bad = [("TM_SESSION", "bad session!")];
    let out = as_agent(&scratch, &repo, "claude", &bad, &["--json", "ready"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let warned = doc["warnings"][0].as_str().unwrap_or_default();
    let why = "no identity, so no Short-lists";
    assert!(warned.starts_with(why), "{doc}");

    // A binding naming no list is a warning: prime never fails a session.
    let nope = [("TM_SHORTLIST", "nope")];
    let bound = as_agent(&scratch, &repo, "claude", &nope, &["prime"]);
    assert!(bound.status.success(), "{bound:?}");
    let said = text(&bound.stdout);
    let warned = "warning: TM_SHORTLIST names nope";
    assert!(said.starts_with(warned), "{said}");
}
