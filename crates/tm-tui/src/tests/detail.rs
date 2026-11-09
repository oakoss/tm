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
}

#[test]
fn wrapped_text_scrolls_to_its_last_line() {
    let store = store();
    let mut shell = opened(&store, &Wide);
    drawn_sized(&mut shell, 20, 10);
    shell.key(press(KeyCode::Char('G')));
    let bottom = drawn_sized(&mut shell, 20, 10);
    assert!(bottom.contains("row20"), "{bottom}");
}

#[test]
fn enter_opens_the_selected_issue_as_the_cli_shows_it() {
    let store = store();
    let mut shell = opened(&store, &Fake);
    insta::assert_snapshot!(drawn(&mut shell));
}

#[test]
fn q_esc_and_backspace_go_back_to_the_list_where_it_was() {
    let store = store();
    for back in [KeyCode::Char('q'), KeyCode::Esc, KeyCode::Backspace] {
        let mut shell = opened(&store, &Fake);
        assert_eq!(shell.key(press(back)), Flow::Reload, "{back:?}");
        shell.load(&store, &Fake);
        assert!(drawn(&mut shell).contains("open Issues"), "{back:?}");
        assert_eq!(list(&shell).selected_key().as_deref(), Some("TM-2"));
    }
    let mut shell = opened(&store, &Fake);
    shell.key(press(KeyCode::Esc));
    assert_eq!(shell.key(press(KeyCode::Char('q'))), Flow::Quit);
}

#[test]
fn an_issue_scrolls_no_further_than_its_last_line_and_keeps_its_place() {
    let store = store();
    let mut shell = opened(&store, &Fake);
    drawn(&mut shell);
    for _ in 0..100 {
        shell.key(press(KeyCode::Char('j')));
    }
    let bottom = drawn(&mut shell);
    assert!(
        bottom.contains("line 30") && !bottom.contains("line 22"),
        "{bottom}"
    );
    shell.key(press(KeyCode::Char('g')));
    assert!(drawn(&mut shell).contains("safe"));
    shell.key(press(KeyCode::Char('G')));
    assert_eq!(drawn(&mut shell), bottom);
    shell.key(press(KeyCode::Char('k')));
    shell.load(&store, &Fake);
    let reread = drawn(&mut shell);
    assert!(
        reread.contains("line 29") && !reread.contains("line 30"),
        "{reread}"
    );
}

#[test]
fn an_issue_that_cannot_be_shown_says_why_in_place() {
    let store = store();
    let mut shell = opened(&store, &Gone);
    let screen = drawn(&mut shell);
    assert!(screen.contains(r"Issue TM-2 \u{1b}[2Jdoes not"), "{screen}");
    assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Reload);
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
