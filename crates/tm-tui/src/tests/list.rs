use super::*;

#[test]
fn the_list_shows_the_open_issues_with_their_text_made_safe() {
    let store = store();
    let mut shell = Shell::new("TM");
    shell.load(&store, &Fake);
    let screen = drawn(&mut shell);
    insta::assert_snapshot!(screen);
    assert!(!screen.contains("finished"), "done is not open work");
    assert!(!screen.contains('\u{1b}'), "{screen}");
}

#[test]
fn a_typed_filter_and_every_status_change_what_is_read() {
    let store = store();
    let mut shell = Shell::new("TM");
    shell.load(&store, &Fake);
    assert_eq!(shell.key(press(KeyCode::Char('/'))), Flow::Stay);
    assert_eq!(
        shell.key(press(KeyCode::Char('q'))),
        Flow::Stay,
        "q is text"
    );
    shell.key(press(KeyCode::Backspace));
    assert_eq!(typed(&mut shell, "title:first"), Flow::Reload);
    shell.load(&store, &Fake);
    let screen = drawn(&mut shell);
    assert!(
        screen.contains("first") && !screen.contains("second"),
        "{screen}"
    );
    assert!(screen.contains("1 Issue "), "{screen}");
    shell.key(press(KeyCode::Char('/')));
    for _ in 0.."title:first".len() {
        shell.key(press(KeyCode::Backspace));
    }
    shell.key(press(KeyCode::Enter));
    assert_eq!(shell.key(press(KeyCode::Char('a'))), Flow::Reload);
    shell.load(&store, &Fake);
    assert!(drawn(&mut shell).contains("finished"));
}

#[test]
fn esc_drops_a_typed_filter_and_a_bad_one_says_why() {
    let store = store();
    let mut shell = Shell::new("TM");
    shell.load(&store, &Fake);
    shell.key(press(KeyCode::Char('/')));
    shell.key(press(KeyCode::Char('x')));
    assert!(drawn(&mut shell).contains("/x"));
    assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay);
    shell.load(&store, &Fake);
    let dropped = drawn(&mut shell);
    assert!(
        !dropped.contains("/x") && dropped.contains("open Issues"),
        "{dropped}"
    );
    assert!(dropped.contains("3 Issues"), "{dropped}");
    assert_eq!(typed_after_slash(&mut shell, "nonsense:1"), Flow::Reload);
    shell.load(&store, &Fake);
    assert!(drawn(&mut shell).contains("unknown field"));
}

#[test]
fn at_me_reads_the_person_or_says_why_no_one_can() {
    let store = store();
    let mut shell = Shell::new("TM");
    typed_after_slash(&mut shell, "assignee:@me");
    shell.load(&store, &Jace);
    let mine = drawn(&mut shell);
    assert!(
        mine.contains("third") && mine.contains("1 Issue "),
        "{mine}"
    );
    shell.load(&store, &Fake);
    let nobody = drawn_at(&mut shell, 100);
    assert!(
        nobody.contains("@me needs to know who you are: set TM_ACTOR"),
        "{nobody}"
    );
}

#[test]
fn a_read_that_shrinks_the_list_keeps_a_row_selected() {
    let store = store();
    let mut shell = Shell::new("TM");
    shell.load(&store, &Fake);
    shell.key(press(KeyCode::Char('G')));
    assert_eq!(list(&shell).selected_key().as_deref(), Some("TM-1"));
    typed_after_slash(&mut shell, "title:third");
    shell.load(&store, &Fake);
    assert_eq!(list(&shell).selected_key().as_deref(), Some("TM-3"));
}

#[test]
fn while_typing_question_mark_is_text_and_a_control_key_is_not() {
    let mut shell = Shell::new("TM");
    shell.key(press(KeyCode::Char('/')));
    assert_eq!(shell.key(press(KeyCode::Char('?'))), Flow::Stay);
    let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert_eq!(shell.key(ctrl_w), Flow::Stay);
    assert_eq!(list(&shell).prompt().as_deref(), Some("/?"));
    assert!(!drawn_sized(&mut shell, 60, 16).contains("Keys"));
}

#[test]
fn the_selection_moves_and_stays_on_its_issue_across_a_read() {
    let mut store = store();
    let mut shell = Shell::new("TM");
    shell.load(&store, &Fake);
    let selected = |shell: &Shell| list(shell).selected_key();
    assert_eq!(selected(&shell).as_deref(), Some("TM-3"), "the first row");
    shell.key(press(KeyCode::Char('G')));
    shell.key(press(KeyCode::Up));
    assert_eq!(selected(&shell).as_deref(), Some("TM-2"));
    let jace = Actor::human("jace");
    let project = store.project_by_key("TM").unwrap().unwrap();
    store
        .create_issue(&jace, project, "task", "zeroth")
        .unwrap();
    shell.load(&store, &Fake);
    assert_eq!(
        selected(&shell).as_deref(),
        Some("TM-2"),
        "kept across a read"
    );
    shell.key(press(KeyCode::Char('g')));
    assert_eq!(selected(&shell).as_deref(), Some("TM-5"));
    shell.key(press(KeyCode::PageDown));
    assert_eq!(
        selected(&shell).as_deref(),
        Some("TM-1"),
        "clamped to the last"
    );
}

#[test]
fn r_while_typing_a_filter_is_text() {
    let mut shell = Shell::new("TM");
    shell.key(press(KeyCode::Char('/')));
    assert_eq!(shell.key(press(KeyCode::Char('r'))), Flow::Stay);
    assert_eq!(shell.screens.depth(), 1);
    assert_eq!(list(&shell).prompt().as_deref(), Some("/r"));
}

#[test]
fn enter_with_no_issue_listed_stays() {
    let store = store();
    let mut shell = Shell::new("TM");
    typed_after_slash(&mut shell, "title:nothing");
    shell.load(&store, &Fake);
    assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Stay);
    assert_eq!(shell.screens.depth(), 1);
}
