//! The Agent session each operation records, and what each did to an Issue.

use serde_json::json;
use tm_core::actor::SessionId;
use tm_core::filter::Context;
use tm_core::{Actor, Change};
use tm_store::{Draft, Store, Workplace};
use uuid::Uuid;

use crate::support::{ManualClock, PROJECT, TempDir, jace, new_issue, open, set_field};

const HOUR_MS: u64 = 3_600_000;

fn agent(name: &str, session: &str) -> Actor {
    Actor::agent(name, "jace").in_session(SessionId::parse(session).unwrap())
}

fn write(store: &mut Store, actor: &Actor, issue: Uuid, change: Change) {
    store
        .append(Draft {
            actor: actor.clone(),
            project: PROJECT,
            subject: issue,
            change,
        })
        .unwrap();
}

fn comment(body: &str) -> Change {
    Change::Comment { body: body.into() }
}

fn placed(store: Store, dir: &str, branch: Option<&str>) -> Store {
    store.working_in(Workplace {
        dir: Some(dir.into()),
        branch: branch.map(str::to_owned),
        ..Workplace::default()
    })
}

#[test]
fn each_session_reads_as_what_it_did_and_where_it_resumes() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let store = open(&dir.db(), &clock);
    let mut store = placed(store, "/work/tm", Some("feat/x"));
    let issue = new_issue(&mut store);
    let claude = agent("claude", "s1");
    store.take(&claude, issue, Some(HOUR_MS)).unwrap();
    write(&mut store, &claude, issue, comment("found it"));
    write(&mut store, &claude, issue, comment("fixed it"));
    write(
        &mut store,
        &claude,
        issue,
        set_field("status", json!("in_review")),
    );
    clock.advance(60_000);
    let codex = agent("codex", "c1");
    write(&mut store, &codex, issue, comment("looked"));
    write(&mut store, &jace(), issue, comment("a person's"));
    clock.advance(60_000);
    store.heartbeat(&claude, Some(issue)).unwrap();

    let sessions = store.sessions(issue).unwrap();
    let ids: Vec<&str> = sessions.iter().map(|work| work.session.as_str()).collect();
    assert_eq!(ids, ["s1", "c1"], "the renewal makes s1 the latest");
    let s1 = &sessions[0];
    assert_eq!(
        (s1.agent.as_str(), s1.claimed, s1.comments),
        ("claude", true, 2)
    );
    assert_eq!(s1.moved_to, ["in_review"]);
    assert_eq!(s1.other, 0, "the renewal is not counted as work");
    assert_eq!(s1.branch.as_deref(), Some("feat/x"));
    assert_eq!(
        s1.place.as_ref().map(|place| place.dir.as_str()),
        Some("/work/tm")
    );
    assert!(s1.moved_last);
    assert_eq!(s1.last_ms - s1.first_ms, 120_000);
    let c1 = &sessions[1];
    assert_eq!(
        (c1.agent.as_str(), c1.comments, c1.claimed),
        ("codex", 1, false)
    );
    assert!(!c1.moved_last);
}

#[test]
fn a_session_written_elsewhere_has_no_directory_here() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    write(
        &mut store,
        &agent("claude", "s1"),
        issue,
        comment("from no place"),
    );
    let sessions = store.sessions(issue).unwrap();
    assert_eq!(sessions[0].place, None);
    assert_eq!(sessions[0].branch, None);
}

#[test]
fn the_session_survives_export_restore_and_rebuild_but_its_directory_stays_here() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = placed(open(&dir.db(), &clock), "/work/tm", None);
    store.set_workspace_id(Uuid::from_u128(77)).unwrap();
    let issue = new_issue(&mut store);
    write(&mut store, &agent("claude", "s1"), issue, comment("noted"));
    let before = store.sessions(issue).unwrap();

    store.rebuild().unwrap();
    assert_eq!(
        store.sessions(issue).unwrap(),
        before,
        "a rebuild keeps the directory"
    );

    let mut file = Vec::new();
    store.export(&mut file, 1).unwrap();
    assert!(!String::from_utf8_lossy(&file).contains("/work/tm"));
    let copy_dir = TempDir::new();
    let mut copy = Store::open_with_clock(&copy_dir.db(), Box::new(clock.clone())).unwrap();
    copy.set_workspace_id(Uuid::from_u128(77)).unwrap();
    copy.restore(&mut file.as_slice()).unwrap();
    let restored = copy.sessions(issue).unwrap();
    assert_eq!(restored[0].session, "s1");
    assert_eq!(restored[0].comments, 1);
    assert_eq!(restored[0].place, None, "the directory is this machine's");
}

#[test]
fn a_session_filter_finds_the_issues_it_wrote() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let (one, two, three, four) = (
        new_issue(&mut store),
        new_issue(&mut store),
        new_issue(&mut store),
        new_issue(&mut store),
    );
    let long = "01a190a7-6cab-7f33-ac3c-161870da743b";
    write(&mut store, &agent("claude", long), one, comment("a"));
    write(&mut store, &agent("codex", "c_1"), two, comment("b"));
    write(&mut store, &jace(), three, comment("c"));
    write(&mut store, &agent("codex", "cx1"), four, comment("d"));
    let find = |filter: &str| store.find(filter, Context::default()).unwrap();
    assert_eq!(find(&format!("session:{long}")), [one]);
    assert_eq!(find("session:…70da743b"), [one], "the end tm history shows");
    assert_eq!(find("session:c_1"), [two]);
    assert_eq!(find("session:…_1"), [two], "an underscore is no wildcard");
    let mut written = find("has:session");
    written.sort();
    let mut both = vec![one, two, four];
    both.sort();
    assert_eq!(written, both);
    assert_eq!(find("no:session"), [three]);
    assert!(store.find("session:a b", Context::default()).is_err());
}

/// A store whose log has no `session` column gains it on open and keeps
/// every operation and hash it held.
#[test]
fn a_log_from_before_sessions_gains_the_column_on_open() {
    let dir = TempDir::new();
    let clock = ManualClock::at(1_000);
    let mut store = open(&dir.db(), &clock);
    let issue = new_issue(&mut store);
    write(
        &mut store,
        &Actor::agent("claude", "jace"),
        issue,
        comment("before"),
    );
    let before = crate::support::dump(&store);
    store
        .connection()
        .execute_batch(
            "DROP TRIGGER ops_immutable;
             DROP INDEX ops_by_session;
             ALTER TABLE ops DROP COLUMN session;
             CREATE TRIGGER ops_immutable
             BEFORE UPDATE OF op_id, op_hash, v, kind, replica_id, actor, on_behalf_of, source,
                              project, issue_id, field, elem, hlc, base_seq, utc_ms, offset_min,
                              payload ON ops
             BEGIN SELECT RAISE(ABORT, 'stored operations are immutable'); END;",
        )
        .unwrap();
    drop(store);

    let mut store = open(&dir.db(), &clock);
    assert_eq!(crate::support::dump(&store), before);
    write(&mut store, &agent("claude", "s2"), issue, comment("after"));
    assert_eq!(store.sessions(issue).unwrap()[0].session, "s2");
    let changed = store
        .connection()
        .execute("UPDATE ops SET session = 'x' WHERE session = 's2'", []);
    assert!(changed.is_err(), "the trigger covers the new column");
    // Replay checks every stored hash against its operation.
    assert_eq!(store.rebuild().unwrap(), 0);
}

#[test]
fn an_import_records_no_session() {
    let dir = TempDir::new();
    let mut store = open(&dir.db(), &ManualClock::at(1_000));
    let issue = new_issue(&mut store);
    let draft = Draft {
        actor: agent("claude", "s1"),
        project: PROJECT,
        subject: issue,
        change: comment("imported"),
    };
    let provenance = tm_store::Provenance {
        source: "beads:tm-1".into(),
        at: tm_store::WallTime {
            utc_ms: 500,
            offset_min: 0,
        },
    };
    let ops = store.append_imported(vec![(draft, provenance)]).unwrap();
    assert_eq!(ops[0].session, None);
    assert_eq!(store.sessions(issue).unwrap(), []);
}
