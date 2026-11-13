//! The Notifications screen, the unread count every screen shows, and the
//! picker that resolves a comment's mentions after the editor saves.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use tm_core::mention::Mentioned;

use super::*;

/// [`Record`], with Notifications to list and mentions that name several.
struct Noticed {
    record: Record,
    notices: RefCell<Vec<Notice>>,
    ambiguous: Vec<Ambiguous>,
}

impl Noticed {
    fn new(store: &Store) -> Self {
        let notice = |n: u128, key: &str, read: bool| Notice {
            id: Uuid::from_u128(n),
            issue: store.issue_by_key(key).unwrap().unwrap(),
            key: key.into(),
            line: format!("{n} * {key} jace in a comment: @kim \u{1b}[2Jlook"),
            read,
        };
        Self {
            record: Record::new(),
            notices: RefCell::new(vec![
                notice(1, "TM-2", false),
                notice(2, "TM-1", false),
                notice(3, "TM-3", true),
            ]),
            ambiguous: Vec::new(),
        }
    }
}

impl Actions for Noticed {
    fn me(&self) -> Result<&str, &str> {
        self.record.me()
    }

    fn issue(&self, store: &Store, id: Uuid) -> Result<Issue, String> {
        self.record.issue(store, id)
    }

    fn ready(&self, store: &Store) -> Result<String, String> {
        self.record.ready(store)
    }

    fn write(&self, write: &Write) -> Result<Written, String> {
        self.record.write(write)
    }

    fn undo(&self, batch: Uuid) -> Result<Written, String> {
        self.record.undo(batch)
    }

    fn unread(&self, _: &Store) -> usize {
        self.notices
            .borrow()
            .iter()
            .filter(|notice| !notice.read)
            .count()
    }

    fn notifications(&self, _: &Store) -> Result<Vec<Notice>, String> {
        Ok(self.notices.borrow().clone())
    }

    fn mentions(&self, _: &Store, _: &str) -> Result<Vec<Ambiguous>, String> {
        Ok(self.ambiguous.clone())
    }
}

fn kim(at: &str) -> Mentioned {
    Mentioned::Person(format!("kim@{at}"))
}

#[test]
fn the_unread_count_shows_unasked_and_m_lists_them_to_open_or_read() {
    let store = store();
    let noticed = Noticed::new(&store);
    let mut shell = Shell::new("TM");
    shell.load(&store, &noticed);
    assert!(drawn_at(&mut shell, 80).contains("tm · TM · 2 unread (m) › Issues"));
    assert_eq!(shell.key(press(KeyCode::Char('m'))), Flow::Reload);
    shell.load(&store, &noticed);
    let screen = drawn_sized(&mut shell, 80, 10);
    assert!(screen.contains(" Notifications "), "{screen}");
    assert!(screen.contains("2 unread"), "{screen}");
    assert!(
        screen.contains("1 * TM-2 jace in a comment: @kim \\u{1b}[2Jlook"),
        "{screen}"
    );
    assert_eq!(shell.key(press(KeyCode::Char('r'))), Flow::Write);
    shell.write(&noticed);
    assert_eq!(
        *noticed.record.writes.borrow(),
        [Write::Read {
            id: Uuid::from_u128(1).to_string()
        }]
    );
    shell.load(&store, &noticed);
    assert_eq!(
        shell.key(press(KeyCode::Char('u'))),
        Flow::Write,
        "u undoes it here"
    );
    shell.write(&noticed);
    assert_eq!(noticed.record.undone.borrow().len(), 1);
    shell.key(press(KeyCode::Char('G')));
    assert_eq!(
        shell.key(press(KeyCode::Char('r'))),
        Flow::Stay,
        "already read"
    );
    assert_eq!(shell.key(press(KeyCode::Char('R'))), Flow::Write);
    shell.write(&noticed);
    assert_eq!(
        noticed.record.writes.borrow().last(),
        Some(&Write::Read { id: "all".into() })
    );
    shell.key(press(KeyCode::Char('g')));
    assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Reload);
    shell.load(&store, &noticed);
    assert!(
        drawn_at(&mut shell, 80).contains("› Notifications › TM-2"),
        "opens its Issue"
    );
}

#[test]
fn every_one_read_shows_no_count_and_r_writes_nothing() {
    let store = store();
    let noticed = Noticed::new(&store);
    for notice in noticed.notices.borrow_mut().iter_mut() {
        notice.read = true;
    }
    let mut shell = Shell::new("TM");
    shell.load(&store, &noticed);
    assert!(!drawn_at(&mut shell, 80).contains("unread"));
    shell.key(press(KeyCode::Char('m')));
    shell.load(&store, &noticed);
    assert_eq!(shell.key(press(KeyCode::Char('R'))), Flow::Stay);
}

/// TM-2 open, its comment composed in `draft`.
fn composing(store: &Store, noticed: &Noticed, draft: &Path) -> Shell {
    let mut shell = Shell::new("TM");
    shell.load(store, noticed);
    while list(&shell).selected_key().as_deref() != Some("TM-2") {
        shell.key(press(KeyCode::Down));
    }
    shell.key(press(KeyCode::Enter));
    shell.load(store, noticed);
    assert_eq!(shell.key(press(KeyCode::Char('c'))), Flow::Compose);
    let composed = Composed {
        text: "ask @kim".into(),
        draft: draft.to_path_buf(),
    };
    shell.compose(Ok(composed), store, noticed);
    shell
}

fn two_kims(store: &Store) -> Noticed {
    Noticed {
        ambiguous: vec![Ambiguous {
            typed: "kim".into(),
            candidates: vec![
                (kim("example.com"), "Kim\tLee <kim@example.com>".into()),
                (kim("other.org"), "kim@other.org".into()),
            ],
        }],
        ..Noticed::new(store)
    }
}

#[test]
fn a_mention_naming_several_is_picked_before_the_comment_is_written() {
    let store = store();
    let noticed = two_kims(&store);
    let draft = PathBuf::from("/drafts/mention.md");
    let mut shell = composing(&store, &noticed, &draft);
    shell.write(&noticed);
    assert!(noticed.record.writes.borrow().is_empty(), "asked first");
    let screen = drawn_sized(&mut shell, 80, 12);
    assert!(screen.contains("@kim names more than one"), "{screen}");
    assert!(screen.contains("none: leave it as text"), "{screen}");
    assert!(
        screen.contains("Kim Lee <kim@example.com>"),
        "a tab drawn as a space: {screen}"
    );
    assert_eq!(
        shell.kept_drafts(),
        [("comment", draft.clone())],
        "kept while asked"
    );
    shell.load(&store, &noticed);
    assert!(
        drawn_sized(&mut shell, 80, 12).contains("@kim names more than one"),
        "a read keeps it"
    );
    shell.key(press(KeyCode::Down));
    assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Write);
    shell.write(&noticed);
    assert_eq!(
        *noticed.record.writes.borrow(),
        [Write::Comment {
            key: "TM-2".into(),
            body: "ask @kim".into(),
            picks: vec![Some(kim("other.org"))],
        }]
    );
}

#[test]
fn leaving_a_mention_as_text_or_escaping_the_picker() {
    let store = store();
    let noticed = two_kims(&store);
    let draft = PathBuf::from("/drafts/mention.md");
    let mut shell = composing(&store, &noticed, &draft);
    shell.key(press(KeyCode::Char('G')));
    for _ in 0..3 {
        shell.key(press(KeyCode::Down));
    }
    assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Write);
    shell.write(&noticed);
    assert_eq!(
        noticed.record.writes.borrow().last(),
        Some(&Write::Comment {
            key: "TM-2".into(),
            body: "ask @kim".into(),
            picks: vec![None],
        }),
        "the last choice leaves it as text"
    );
    shell.key(press(KeyCode::Char('c')));
    let composed = Composed {
        text: "ask @kim".into(),
        draft: draft.clone(),
    };
    shell.compose(Ok(composed), &store, &noticed);
    assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay);
    assert_eq!(
        noticed.record.writes.borrow().len(),
        1,
        "Esc writes nothing"
    );
    assert_eq!(shell.kept_drafts(), [("comment", draft)]);
    assert!(drawn_sized(&mut shell, 80, 12).contains("✗ comment kept"));
}
