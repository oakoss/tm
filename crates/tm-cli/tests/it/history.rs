//! `tm history`: an Issue's writes, oldest first.

use std::path::Path;

use serde_json::Value;
use tm_core::actor::SessionId;
use tm_core::op::ClaimAction;
use tm_core::{Actor, Change};
use tm_store::Draft;

use crate::support::{Scratch, initialized, text, tm, tm_piped};

/// The text with each write's local time, whatever its zone, cut from its
/// header line; the time is the only part of a header before two spaces.
fn untimed(shown: &str) -> String {
    shown
        .lines()
        .map(|line| match line.split_once("  ") {
            Some((_, rest)) if line.starts_with("20") => format!("<time>  {rest}\n"),
            _ => format!("{line}\n"),
        })
        .collect()
}

fn history(scratch: &Scratch, repo: &Path, args: &[&str]) -> String {
    let out = tm(scratch, repo, &[&["history"], args].concat());
    assert!(out.status.success(), "{out:?}");
    untimed(&text(&out.stdout))
}

#[test]
fn history_shows_each_write_oldest_first() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    let (issue, _) = store.create_issue(&jace, project, "task", "first").unwrap();
    let second = store.create_issue(&jace, project, "task", "second");
    second.unwrap();
    let claude = Actor::agent("claude", "jace");
    let claim = store.take(&claude, issue, Some(3_600_000)).unwrap();
    let renew = Change::Claim {
        action: ClaimAction::Renew {
            claim: claim.id,
            ttl_ms: 3_600_000,
        },
    };
    let draft = Draft {
        actor: claude.clone(),
        project,
        subject: issue,
        change: renew,
    };
    store.append(draft).unwrap();
    drop(store);
    for args in [
        &["edit", "TM-1", "--set", "assignee=jace", "--label", "ui"][..],
        &["link", "TM-2", "blocks", "TM-1"],
        &["move", "TM-1", "in_progress"],
    ] {
        let out = tm(&scratch, &repo, args);
        assert!(out.status.success(), "{out:?}");
    }
    let out = tm_piped(&scratch, &repo, &["comment", "TM-1", "-"], "on it\nmore\n");
    assert!(out.status.success(), "{out:?}");
    let undone = tm(&scratch, &repo, &["undo"]);
    assert!(undone.status.success(), "{undone:?}");

    let shown = history(&scratch, &repo, &["TM-1"]);
    let batches: Vec<&str> = shown
        .lines()
        .filter_map(|line| line.split_once("(batch ").map(|(_, rest)| &rest[..12]))
        .collect();
    let [create, take, edit, link, moved, comment, undo] = batches[..] else {
        panic!("{shown}");
    };
    let expected = format!(
        "TM-1  first\n\
         <time>  jace  (batch {create})\n  created\n  Type: Task\n  Title: first\n  Status: Todo\n\
         <time>  claude for jace  (batch {take})\n  claimed: claude for 1h\n\
         <time>  jace  (batch {edit})\n  Assignee: (none) → jace\n  Labels: + ui\n\
         <time>  jace  (batch {link})\n  linked: is blocked by TM-2\n\
         <time>  jace  (batch {moved})\n  Status: Todo → In Progress\n\
         <time>  jace  (batch {comment}, undone by {undo})\n  commented: on it…\n\
         <time>  jace  (batch {undo}, undoes {comment})\n  retracted a comment\n"
    );
    assert_eq!(shown, expected);

    let all = history(&scratch, &repo, &["TM-1", "--all"]);
    assert!(all.contains("\n  Claim renewed for 1h\n"), "{all}");
    let latest = history(&scratch, &repo, &["TM-1", "--limit", "2"]);
    let head = "TM-1  first\n<time>  jace  (batch";
    assert!(latest.starts_with(head), "{latest}");
    assert!(latest.contains("commented: on it…"), "{latest}");
    assert!(!latest.contains("In Progress"), "{latest}");
    assert!(latest.ends_with("(2 of 7 writes shown)\n"), "{latest}");

    let doc = tm(&scratch, &repo, &["--json", "history", "TM-2"]);
    let doc: Value = serde_json::from_slice(&doc.stdout).unwrap();
    let lines: Vec<&str> = doc["data"]["writes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|write| write["changes"].as_array().unwrap())
        .map(|change| change["line"].as_str().unwrap())
        .collect();
    assert_eq!(lines.last(), Some(&"linked: blocks TM-1"), "{doc}");
    assert_eq!(doc["data"]["total"], 2);

    let doc = tm(&scratch, &repo, &["--json", "history", "TM-1"]);
    let doc: Value = serde_json::from_slice(&doc.stdout).unwrap();
    let writes = doc["data"]["writes"].as_array().unwrap();
    let (commented, undo) = (&writes[5], &writes[6]);
    assert_eq!(commented["undone_by"], undo["batch"]);
    assert_eq!(undo["undid"], commented["batch"]);
    assert_eq!(commented["undid"], Value::Null);
    // `before` holds the value as stored, beside the line's label.
    let moved = &writes[4]["changes"][0];
    assert_eq!(moved["line"], "Status: Todo → In Progress");
    assert_eq!(moved["before"], "todo");
    let retracted = &writes[6]["changes"][0];
    assert_eq!(retracted["before"], Value::Null, "not a field");
}

#[test]
fn history_names_takeovers_releases_links_and_long_text() {
    let scratch = Scratch::new();
    let (repo, mut store, project) = initialized(&scratch);
    let jace = Actor::human("jace");
    store.create_issue(&jace, project, "epic", "E").unwrap();
    let description = format!("{}\nsecond line", "a".repeat(70));
    let create = "create --type task --title c --parent TM-1 --set description=-";
    let args: Vec<&str> = create.split(' ').collect();
    let out = tm_piped(&scratch, &repo, &args, &description);
    assert!(out.status.success(), "{out:?}");
    let child = store.issue_by_key("TM-2").unwrap().unwrap();
    let s1 = Actor::agent("claude", "jace").in_session(SessionId::parse("s1").unwrap());
    store.take(&s1, child, Some(1)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let codex = Actor::agent("codex", "jace");
    store.take(&codex, child, None).unwrap();
    let comment = store
        .append(Draft {
            actor: jace.clone(),
            project,
            subject: child,
            change: Change::Comment { body: "ok".into() },
        })
        .unwrap();
    for retracted in [true, false] {
        let retract = Change::Retract {
            comment: comment.op_id,
            retracted,
        };
        let draft = Draft {
            actor: jace.clone(),
            project,
            subject: child,
            change: retract,
        };
        store.append(draft).unwrap();
    }
    let archive = Change::SetField {
        field: "archived".into(),
        label: "Archived".into(),
        value: Value::Bool(true),
        value_label: None,
    };
    let draft = Draft {
        actor: jace.clone(),
        project,
        subject: child,
        change: archive,
    };
    store.append(draft).unwrap();
    for args in [
        &["link", "TM-2", "relates_to", "TM-1"][..],
        &["unlink", "TM-2", "relates_to", "TM-1"],
        &["move", "TM-2", "done"],
    ] {
        let out = tm(&scratch, &repo, args);
        assert!(out.status.success(), "{out:?}");
    }

    let shown = history(&scratch, &repo, &["TM-2"]);
    // Batches are pinned in the test above; here only the lines matter.
    let unbatched: String = shown
        .lines()
        .map(|line| match line.split_once("  (batch ") {
            Some((who, _)) => format!("{who}\n"),
            None => format!("{line}\n"),
        })
        .collect();
    let expected = "TM-2  c\n\
        <time>  jace\n  created\n  Type: Task\n  Status: Todo\n  Title: c\n  Parent: TM-1\n  \
        Description: 2 lines\n\
        <time>  claude for jace in session s1\n  claimed: claude for under a minute\n\
        <time>  codex for jace\n  took over the Claim: codex, with no expiry\n\
        <time>  jace\n  commented: ok\n\
        <time>  jace\n  retracted a comment\n\
        <time>  jace\n  showed a retracted comment again\n\
        <time>  jace\n  Archived: false → true\n\
        <time>  jace\n  linked: relates to TM-1\n\
        <time>  jace\n  unlinked: relates to TM-1\n\
        <time>  jace\n  Status: Todo → Done\n  Claim released\n";
    assert_eq!(unbatched, expected);

    let all = history(&scratch, &repo, &["TM-2", "--limit", "99"]);
    assert_eq!(all, shown, "a limit past the total shows every write");
    let none = history(&scratch, &repo, &["TM-2", "--limit", "0"]);
    assert_eq!(none, "TM-2  c\n(0 of 10 writes shown)\n");

    let doc = tm(&scratch, &repo, &["--json", "history", "TM-2"]);
    let doc: Value = serde_json::from_slice(&doc.stdout).unwrap();
    let writes = doc["data"]["writes"].as_array().unwrap();
    assert_eq!(writes[1]["on_behalf_of"], "jace");
    assert_eq!(writes[1]["changes"][0]["lost"], false);
}
