use std::fmt::Write as _;

use super::*;

/// Lines that each wrap once inside a 20-column frame's borders.
struct Wide;

impl Actions for Wide {
    fn write(&self, write: &Write) -> Result<Written, String> {
        Err(format!("{} not written", write.key()))
    }

    fn undo(&self, _: Uuid) -> Result<Written, String> {
        Err("nothing undone".into())
    }

    fn me(&self) -> Result<&str, &str> {
        NOBODY
    }

    fn detail(&self, _: &Store, _: Uuid) -> Result<String, String> {
        Ok((1..=20).fold(String::new(), |mut text, n| {
            let _ = writeln!(text, "row{n:02} wraps in here");
            text
        }))
    }

    fn ready(&self, store: &Store) -> Result<String, String> {
        self.detail(store, Uuid::nil())
    }

    fn issue(&self, _: &Store, _: Uuid) -> Result<Issue, String> {
        Err("unused".into())
    }
}

#[test]
fn wrapped_text_scrolls_to_its_last_line() {
    let store = store();
    let mut shell = Shell::new("TM");
    shell.key(press(KeyCode::Char('r')));
    shell.load(&store, &Wide);
    drawn_sized(&mut shell, 20, 10);
    shell.key(press(KeyCode::Char('G')));
    let bottom = drawn_sized(&mut shell, 20, 10);
    assert!(bottom.contains("row20"), "{bottom}");
}

#[test]
fn r_opens_the_ready_queue_as_the_cli_prints_it_and_q_goes_back() {
    let store = store();
    let mut shell = Shell::new("TM");
    shell.load(&store, &Fake);
    shell.key(press(KeyCode::Char('j')));
    assert_eq!(shell.key(press(KeyCode::Char('r'))), Flow::Reload);
    shell.load(&store, &Fake);
    insta::assert_snapshot!(drawn(&mut shell));
    for back in [KeyCode::Char('q'), KeyCode::Esc, KeyCode::Backspace] {
        if shell.screens.depth() == 1 {
            shell.key(press(KeyCode::Char('r')));
            shell.load(&store, &Fake);
        }
        assert_eq!(shell.key(press(back)), Flow::Reload, "{back:?}");
        shell.load(&store, &Fake);
        assert!(drawn(&mut shell).contains("open Issues"), "{back:?}");
        assert_eq!(list(&shell).selected_key().as_deref(), Some("TM-2"));
    }
}

#[test]
fn r_with_an_issue_open_leaves_it_open() {
    let store = store();
    let mut shell = opened(&store, &Fake);
    assert_eq!(shell.key(press(KeyCode::Char('r'))), Flow::Stay);
    assert!(drawn(&mut shell).contains("┌ TM-2 "));
}

#[test]
fn the_ready_queue_reads_a_change_in_place() {
    let mut store = store();
    let mut shell = Shell::new("TM");
    shell.key(press(KeyCode::Char('r')));
    shell.load(&store, &Fake);
    assert!(drawn(&mut shell).contains("3 ready"));
    let jace = Actor::human("jace");
    let project = store.project_by_key("TM").unwrap().unwrap();
    store
        .create_issue(&jace, project, "task", "fourth")
        .unwrap();
    shell.load(&store, &Fake);
    assert!(drawn(&mut shell).contains("4 ready"));
}

#[test]
fn a_ready_queue_that_cannot_be_read_says_why_in_place() {
    let store = store();
    let mut shell = Shell::new("TM");
    shell.key(press(KeyCode::Char('r')));
    shell.load(&store, &Gone);
    let screen = drawn(&mut shell);
    assert!(
        screen.contains(r"the queue \u{1b}[2Jcannot be read"),
        "{screen}"
    );
}
