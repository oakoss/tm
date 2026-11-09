use super::*;

fn on_board(store: &Store, actions: &dyn Actions) -> Shell {
    let mut shell = Shell::new("TM");
    shell.load(store, actions);
    assert_eq!(shell.key(press(KeyCode::Char('b'))), Flow::Reload);
    shell.load(store, actions);
    shell
}

/// The cursor's column Status and Issue Key.
fn cursor(shell: &mut Shell) -> (String, Option<String>) {
    match shell.shown() {
        Screen::Board(board) => board.cursor(),
        _ => unreachable!("the Board is shown"),
    }
}

#[test]
fn b_opens_a_column_per_open_status_and_a_adds_done_and_cancelled() {
    let store = store();
    let record = Record::new();
    let mut shell = on_board(&store, &record);
    let screen = drawn_sized(&mut shell, 120, 10);
    insta::assert_snapshot!(screen);
    for column in ["Triage 0", "Todo 3", "In Progress 0", "In Review 0"] {
        assert!(screen.contains(column), "{column}: {screen}");
    }
    assert!(!screen.contains("Done"), "{screen}");
    assert_eq!(cursor(&mut shell).0, "todo", "opens on the first Issue");
    assert_eq!(shell.key(press(KeyCode::Char('a'))), Flow::Reload);
    shell.load(&store, &record);
    let screen = drawn_sized(&mut shell, 160, 10);
    assert!(
        screen.contains("Done 1") && screen.contains("Cancelled 0"),
        "{screen}"
    );
}

#[test]
fn capital_l_and_h_move_the_issue_beside_and_the_cursor_follows_it() {
    let mut store = store();
    let record = Record::new();
    let mut shell = on_board(&store, &record);
    let (_, key) = cursor(&mut shell);
    let key = key.unwrap();
    assert_eq!(shell.key(press(KeyCode::Char('L'))), Flow::Write);
    shell.write(&record);
    assert_eq!(
        *record.writes.borrow(),
        [Write::Move {
            key: key.clone(),
            status: "in_progress".into(),
            version: version(&store, &key),
        }]
    );
    set(&mut store, &key, "status", "in_progress");
    shell.load(&store, &record);
    assert_eq!(
        cursor(&mut shell),
        ("in_progress".into(), Some(key.clone()))
    );
    assert_eq!(shell.key(press(KeyCode::Char('H'))), Flow::Write);
    shell.write(&record);
    let writes = record.writes.borrow();
    assert!(
        matches!(&writes[1], Write::Move { status, version: v, .. }
            if status == "todo" && *v == version(&store, &key)),
        "{writes:?}"
    );
}

#[test]
fn a_move_past_either_end_of_the_workflow_writes_nothing() {
    let mut store = store();
    let record = Record::new();
    let mut shell = on_board(&store, &record);
    let key = cursor(&mut shell).1.unwrap();
    set(&mut store, &key, "status", "triage");
    shell.load(&store, &record);
    assert_eq!(cursor(&mut shell), ("triage".into(), Some(key.clone())));
    assert_eq!(shell.key(press(KeyCode::Char('H'))), Flow::Stay);
    set(&mut store, &key, "status", "cancelled");
    shell.key(press(KeyCode::Char('a')));
    shell.load(&store, &record);
    assert_eq!(cursor(&mut shell), ("cancelled".into(), Some(key)));
    assert_eq!(shell.key(press(KeyCode::Char('L'))), Flow::Stay);
    assert!(record.writes.borrow().is_empty());
}

#[test]
fn a_refused_move_leaves_the_cursor_on_its_issue() {
    let store = store();
    let record = Record {
        refuse: true,
        ..Record::new()
    };
    let mut shell = on_board(&store, &record);
    shell.key(press(KeyCode::Char('j')));
    let before = cursor(&mut shell);
    shell.key(press(KeyCode::Char('L')));
    shell.write(&record);
    shell.load(&store, &record);
    assert_eq!(cursor(&mut shell), before);
    assert!(drawn_sized(&mut shell, 120, 10).contains("✗ "));
}

#[test]
fn s_moves_the_issue_to_a_status_its_workflow_has() {
    let store = store();
    let record = Record::new();
    let mut shell = on_board(&store, &record);
    let key = cursor(&mut shell).1.unwrap();
    assert_eq!(pick(&mut shell, 's', "review"), Flow::Write);
    shell.write(&record);
    assert_eq!(
        *record.writes.borrow(),
        [Write::Move {
            key: key.clone(),
            status: "in_review".into(),
            version: version(&store, &key),
        }]
    );
}

#[test]
fn columns_past_the_terminal_scroll_into_view_and_say_how_many_are_hidden() {
    let store = store();
    let record = Record::new();
    let mut shell = on_board(&store, &record);
    let screen = drawn_sized(&mut shell, 60, 10);
    assert!(
        screen.contains("Todo 3") && screen.contains(" 2› "),
        "{screen}"
    );
    for _ in 0..3 {
        shell.key(press(KeyCode::Char('l')));
    }
    assert_eq!(cursor(&mut shell), ("in_review".into(), None));
    let screen = drawn_sized(&mut shell, 60, 10);
    assert!(screen.contains("‹2 In Progress 0"), "{screen}");
    assert!(
        screen.contains("In Review 0") && !screen.contains("Todo"),
        "{screen}"
    );
    assert_eq!(shell.key(press(KeyCode::Char('L'))), Flow::Stay, "no Issue");
}

#[test]
fn enter_opens_the_issue_under_the_cursor() {
    let store = store();
    let record = Record::new();
    let mut shell = on_board(&store, &record);
    shell.key(press(KeyCode::Char('G')));
    let key = cursor(&mut shell).1.unwrap();
    assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Reload);
    shell.load(&store, &record);
    assert!(drawn_sized(&mut shell, 120, 10).contains(&format!("Board › {key}")));
    assert_eq!(shell.key(press(KeyCode::Char('q'))), Flow::Reload);
    shell.load(&store, &record);
    assert_eq!(cursor(&mut shell).1, Some(key), "back where it was");
}

#[test]
fn scrolled_columns_come_back_as_the_cursor_or_the_terminal_widens() {
    let store = store();
    let record = Record::new();
    let mut shell = on_board(&store, &record);
    for _ in 0..3 {
        shell.key(press(KeyCode::Char('l')));
    }
    drawn_sized(&mut shell, 60, 10);
    let wide = drawn_sized(&mut shell, 120, 10);
    assert!(wide.contains("Triage 0") && !wide.contains('‹'), "{wide}");
    drawn_sized(&mut shell, 60, 10);
    for _ in 0..3 {
        shell.key(press(KeyCode::Char('h')));
    }
    let back = drawn_sized(&mut shell, 60, 10);
    assert!(back.contains("Todo 3") && !back.contains('‹'), "{back}");
}

#[test]
fn a_gone_issue_leaves_the_cursor_in_its_column_and_a_hidden_column_sends_it_to_work() {
    let mut store = store();
    let record = Record::new();
    let mut shell = on_board(&store, &record);
    shell.key(press(KeyCode::Char('G')));
    let gone = cursor(&mut shell).1.unwrap();
    set(&mut store, &gone, "status", "done");
    shell.load(&store, &record);
    let (column, now) = cursor(&mut shell);
    assert_eq!(column, "todo");
    assert!(now.is_some_and(|key| key != gone));
    shell.key(press(KeyCode::Char('a')));
    shell.load(&store, &record);
    for _ in 0..5 {
        shell.key(press(KeyCode::Char('l')));
    }
    assert_eq!(cursor(&mut shell).0, "cancelled");
    shell.key(press(KeyCode::Char('a')));
    shell.load(&store, &record);
    assert_eq!(
        cursor(&mut shell).0,
        "todo",
        "the first column with an Issue"
    );
}

#[test]
fn the_cursor_follows_its_issue_when_an_issue_above_it_leaves() {
    let mut store = store();
    let record = Record::new();
    let mut shell = on_board(&store, &record);
    let first = cursor(&mut shell).1.unwrap();
    shell.key(press(KeyCode::Char('j')));
    let kept = cursor(&mut shell).1;
    set(&mut store, &first, "status", "done");
    shell.load(&store, &record);
    assert_eq!(cursor(&mut shell).1, kept);
}

#[test]
fn a_column_keeps_its_row_while_the_cursor_is_elsewhere() {
    let store = store();
    let record = Record::new();
    let mut shell = on_board(&store, &record);
    shell.key(press(KeyCode::Char('j')));
    let left_on = cursor(&mut shell).1;
    shell.key(press(KeyCode::Char('l')));
    shell.load(&store, &record);
    shell.key(press(KeyCode::Char('h')));
    assert_eq!(cursor(&mut shell).1, left_on);
}

#[test]
fn moves_follow_the_issues_own_workflow_not_the_columns() {
    let mut scheme = tm_core::scheme::Scheme::built_in();
    let default = scheme.workflows[0].clone();
    let lean = default
        .statuses
        .iter()
        .filter(|status| ["todo", "in_review", "done"].contains(&status.id.as_str()))
        .cloned()
        .collect();
    scheme.workflows.push(tm_core::scheme::Workflow {
        name: "lean".into(),
        start: "todo".into(),
        statuses: lean,
        ..default
    });
    let bug = scheme
        .types
        .iter_mut()
        .find(|issue| issue.id == "bug")
        .unwrap();
    bug.workflow = "lean".into();
    let mut store = store().with_scheme(scheme);
    let jace = Actor::human("jace");
    let project = store.project_by_key("TM").unwrap().unwrap();
    let (id, _) = store.create_issue(&jace, project, "bug", "lean").unwrap();
    let key = store
        .issue_with_version(id)
        .unwrap()
        .unwrap()
        .0
        .key
        .unwrap();
    let record = Record::new();
    let mut shell = on_board(&store, &record);
    for _ in 0..4 {
        if cursor(&mut shell).1.as_deref() == Some(key.as_str()) {
            break;
        }
        shell.key(press(KeyCode::Char('j')));
    }
    assert_eq!(cursor(&mut shell).1, Some(key));
    assert_eq!(
        shell.key(press(KeyCode::Char('H'))),
        Flow::Stay,
        "no triage"
    );
    assert_eq!(pick(&mut shell, 's', "progress"), Flow::Stay, "not offered");
    assert_eq!(shell.key(press(KeyCode::Char('L'))), Flow::Write);
    shell.write(&record);
    assert!(
        matches!(&record.writes.borrow()[..], [Write::Move { status, .. }] if status == "in_review"),
        "{:?}",
        record.writes.borrow()
    );
}
