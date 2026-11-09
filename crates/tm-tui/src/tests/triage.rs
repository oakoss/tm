use super::*;

#[test]
fn t_opens_the_issues_in_triage_and_only_those() {
    let store = triage_store();
    let mut shell = triaging(&store, &Record::new());
    let screen = drawn_sized(&mut shell, 70, 10);
    assert!(
        screen.contains("Triage") && screen.contains("4 Issues"),
        "{screen}"
    );
    assert!(
        !screen.contains("first"),
        "an open Issue not in triage: {screen}"
    );
    assert!(
        screen.contains("s status") && screen.contains("u undo"),
        "{screen}"
    );
}

#[test]
fn the_queue_shows_an_issues_text_made_safe() {
    let mut store = triage_store();
    set(&mut store, "TM-5", "title", "tabbed\there \u{1b}[31mred");
    let mut shell = triaging(&store, &Record::new());
    let screen = drawn_sized(&mut shell, 70, 10);
    assert!(screen.contains("tabbed here \\u{1b}[31mred"), "{screen}");
    assert!(!screen.contains(['\t', '\u{1b}']), "{screen}");
}

#[test]
fn a_status_picked_moves_the_issue_as_read_and_goes_on_to_the_next() {
    let store = triage_store();
    let record = Record::new();
    let mut shell = triaging(&store, &record);
    let first = queue_key(&mut shell).unwrap();
    shell.key(press(KeyCode::Char('j')));
    let second = queue_key(&mut shell).unwrap();
    shell.key(press(KeyCode::Char('k')));
    assert_eq!(pick(&mut shell, 's', "todo"), Flow::Write);
    shell.write(&record);
    shell.load(&store, &record);
    let id = store.issue_by_key(&first).unwrap().unwrap();
    let version = store.issue_with_version(id).unwrap().unwrap().1;
    assert_eq!(
        *record.writes.borrow(),
        [Write::Move {
            key: first.clone(),
            status: "todo".into(),
            version,
        }]
    );
    assert_eq!(queue_key(&mut shell), Some(second), "the next Issue");
    let footer = drawn_sized(&mut shell, 70, 10);
    assert!(
        footer.contains(&format!("✓ {first}  written · u undo")),
        "{footer}"
    );
    shell.key(press(KeyCode::Char('j')));
    assert!(
        !drawn_sized(&mut shell, 70, 10).contains("✓"),
        "said until the next key"
    );
}

#[test]
fn marked_issues_take_the_action_and_at_me_comes_first() {
    let store = triage_store();
    let record = Record::new();
    let mut shell = triaging(&store, &record);
    shell.key(press(KeyCode::Char(' ')));
    shell.key(press(KeyCode::Char(' ')));
    assert!(drawn_sized(&mut shell, 70, 10).contains("2 marked"));
    shell.key(press(KeyCode::Char('A')));
    let picker = drawn_sized(&mut shell, 70, 12);
    let lines: Vec<Vec<char>> = picker.lines().map(|line| line.chars().collect()).collect();
    let (top, left) = lines
        .iter()
        .enumerate()
        .find_map(|(row, line)| {
            let text: String = line.iter().collect();
            text.find("┌ Assign")
                .map(|byte| (row, text[..byte].chars().count()))
        })
        .unwrap();
    let choices: Vec<String> = lines[top + 1..]
        .iter()
        .take_while(|line| line.get(left) == Some(&'│'))
        .map(|line| {
            let inside = line[left + 1..].iter().take_while(|c| **c != '│');
            inside.collect::<String>().trim().to_owned()
        })
        .collect();
    assert_eq!(choices, ["jace (@me)", "bob", "nobody"], "{picker}");
    assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Write);
    shell.write(&record);
    let writes = record.writes.borrow();
    assert_eq!(writes.len(), 2);
    assert!(writes.iter().all(|write| matches!(
        write,
        Write::Set { field, value: Some(value), .. } if field == "assignee" && value == "jace"
    )));
}

#[test]
fn a_name_no_one_holds_yet_is_assigned_as_typed_and_nobody_unsets() {
    let store = triage_store();
    let record = Record::new();
    let mut shell = triaging(&store, &record);
    assert_eq!(pick(&mut shell, 'A', "kim"), Flow::Write);
    shell.write(&record);
    shell.load(&store, &record);
    assert_eq!(pick(&mut shell, 'A', "nobody"), Flow::Write);
    shell.write(&record);
    let writes = record.writes.borrow();
    assert!(matches!(&writes[0], Write::Set { value: Some(value), .. } if value == "kim"));
    assert!(matches!(&writes[1], Write::Set { value: None, .. }));
}

#[test]
fn a_refused_write_says_why_and_the_cursor_stays() {
    let store = triage_store();
    let record = Record {
        refuse: true,
        ..Record::new()
    };
    let mut shell = triaging(&store, &record);
    let first = queue_key(&mut shell);
    pick(&mut shell, 'p', "high");
    shell.write(&record);
    shell.load(&store, &record);
    assert_eq!(queue_key(&mut shell), first);
    let screen = drawn_sized(&mut shell, 80, 10);
    assert!(screen.contains("✗ "), "{screen}");
    assert!(record.writes.borrow().is_empty());
}

#[test]
fn esc_cancels_a_picker_and_its_keys_are_text() {
    let store = triage_store();
    let record = Record::new();
    let mut shell = triaging(&store, &record);
    shell.key(press(KeyCode::Char('s')));
    assert_eq!(shell.key(press(KeyCode::Char('?'))), Flow::Stay);
    assert!(!shell.help, "? is text in a picker");
    assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay);
    assert_eq!(
        shell.key(press(KeyCode::Char('q'))),
        Flow::Reload,
        "q goes back once the picker is gone"
    );
    assert!(record.writes.borrow().is_empty());
}

fn written_keys(record: &Record) -> Vec<String> {
    let writes = record.writes.borrow();
    writes.iter().map(|write| write.key().to_owned()).collect()
}

#[test]
fn a_picker_writes_the_issue_it_opened_on_whatever_a_read_moves() {
    let mut store = triage_store();
    let record = Record::new();
    let mut shell = triaging(&store, &record);
    let first = queue_key(&mut shell).unwrap();
    shell.key(press(KeyCode::Char('p')));
    set(&mut store, &first, "status", "todo");
    shell.load(&store, &record);
    for c in "high".chars() {
        shell.key(press(KeyCode::Char(c)));
    }
    assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Write);
    shell.write(&record);
    assert_eq!(written_keys(&record), [first]);
}

#[test]
fn after_a_write_from_the_middle_the_cursor_goes_down_not_up() {
    let store = triage_store();
    let record = Record::new();
    let mut shell = triaging(&store, &record);
    shell.key(press(KeyCode::Char('j')));
    shell.key(press(KeyCode::Char('j')));
    shell.key(press(KeyCode::Char('j')));
    let after = queue_key(&mut shell);
    shell.key(press(KeyCode::Char('k')));
    pick(&mut shell, 'p', "high");
    shell.write(&record);
    shell.load(&store, &record);
    assert_eq!(queue_key(&mut shell), after);
}

#[test]
fn the_selected_issue_shows_beside_the_queue_and_the_crumbs_name_the_screens() {
    let store = triage_store();
    let record = Record::new();
    let mut shell = triaging(&store, &record);
    let screen = drawn_sized(&mut shell, 110, 10);
    assert!(screen.contains("tm · TM › Issues › Triage"), "{screen}");
    assert!(
        screen.contains("shown") && screen.contains("line 1"),
        "{screen}"
    );
    shell.key(press(KeyCode::Char('j')));
    shell.load(&store, &record);
    let selected = queue_key(&mut shell).unwrap();
    assert!(drawn_sized(&mut shell, 110, 10).contains(&format!("┌ {selected} ")));
}

#[test]
fn a_detail_that_cannot_be_read_says_so_beside_the_queue() {
    struct Unreadable;

    impl Actions for Unreadable {
        fn me(&self) -> Result<&str, &str> {
            NOBODY
        }

        fn detail(&self, _: &Store, _: Uuid) -> Result<String, String> {
            Err("database is locked".into())
        }

        fn ready(&self, store: &Store) -> Result<String, String> {
            Fake.ready(store)
        }

        fn write(&self, write: &Write) -> Result<Written, String> {
            Fake.write(write)
        }

        fn undo(&self, batch: Uuid) -> Result<Written, String> {
            Fake.undo(batch)
        }
    }

    let store = triage_store();
    let mut shell = triaging(&store, &Unreadable);
    let screen = drawn_sized(&mut shell, 140, 10);
    assert!(screen.contains("✗ cannot show TM-"), "{screen}");
    assert!(screen.contains("database is locked"), "{screen}");
}
