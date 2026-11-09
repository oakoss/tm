use ratatui::crossterm::event::KeyEventState;

use super::*;

#[test]
fn q_and_ctrl_c_quit_and_a_release_does_nothing() {
    let mut shell = Shell::new("TM");
    assert_eq!(shell.key(press(KeyCode::Char('q'))), Flow::Quit);
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(shell.key(ctrl_c), Flow::Quit);
    let release = KeyEvent::new_with_kind_and_state(
        KeyCode::Char('q'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
        KeyEventState::NONE,
    );
    assert_eq!(shell.key(release), Flow::Stay);
    shell.key(press(KeyCode::Char('/')));
    assert_eq!(shell.key(ctrl_c), Flow::Quit, "even while typing");
}

#[test]
fn question_mark_shows_the_keys_and_esc_hides_them() {
    let mut shell = Shell::new("TM");
    assert_eq!(shell.key(press(KeyCode::Char('?'))), Flow::Stay);
    insta::assert_snapshot!(drawn_sized(&mut shell, 60, 17));
    assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay);
    assert!(!drawn(&mut shell).contains("Keys"));
}

#[test]
fn a_busy_store_shows_until_a_look_gets_through() {
    let mut shell = Shell::new("TM");
    shell.seen(Seen::Changed("10:15:03".into()));
    shell.seen(Seen::Busy);
    assert!(drawn(&mut shell).contains("store busy, retrying"));
    shell.seen(Seen::Nothing);
    let after = drawn(&mut shell);
    assert!(!after.contains("busy"), "{after}");
    assert!(after.contains("changed elsewhere 10:15:03"), "{after}");
}

#[test]
fn a_narrow_footer_keeps_the_status_whole_beside_the_keys() {
    let mut shell = Shell::new("TM");
    shell.seen(Seen::Changed("10:15:03".into()));
    let footer = drawn_at(&mut shell, 34).lines().last().unwrap().to_owned();
    assert!(footer.contains("changed elsewhere 10:15:03"), "{footer}");
}

#[test]
fn no_size_panics() {
    let store = store();
    let mut shell = Shell::new("TM");
    shell.load(&store, &Fake);
    shell.seen(Seen::Changed("10:15:03".into()));
    let states = [
        (None, false),
        (None, true),
        (Some(KeyCode::Enter), true),
        (Some(KeyCode::Enter), false),
        (Some(KeyCode::Char('r')), false),
        (Some(KeyCode::Char('r')), true),
    ];
    for (open, help) in states {
        if let Some(open) = open {
            if shell.screens.depth() > 1 {
                shell.key(press(KeyCode::Esc));
            }
            shell.key(press(open));
            shell.load(&store, &Fake);
        }
        if help != shell.help {
            shell.key(press(KeyCode::Char('?')));
        }
        for width in 0..50 {
            for height in 0..12 {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| shell.render(frame)).unwrap();
                shell.key(press(KeyCode::Char('G')));
            }
        }
    }
}

#[test]
fn the_keys_shown_are_the_open_screens() {
    let store = store();
    let mut shell = opened(&store, &Fake);
    shell.key(press(KeyCode::Char('?')));
    let help = drawn_sized(&mut shell, 60, 16);
    assert!(
        help.contains("q Esc Bksp  back") && help.contains("scroll down and up"),
        "{help}"
    );
    assert!(!help.contains("type a filter"), "{help}");
    let footer = help.lines().last().unwrap_or_default();
    assert!(
        footer.starts_with("\"q back · j k scroll · ? keys "),
        "{footer}"
    );
}

#[test]
fn a_modified_esc_hides_the_keys_and_ctrl_c_quits_with_any_modifier() {
    let store = store();
    let mut shell = opened(&store, &Fake);
    shell.key(press(KeyCode::Char('?')));
    let shift_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::SHIFT);
    assert_eq!(shell.key(shift_esc), Flow::Stay);
    assert!(!shell.help && shell.screens.depth() == 2);
    let ctrl_alt_c = KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );
    assert_eq!(shell.key(ctrl_alt_c), Flow::Quit);
}

#[test]
fn esc_while_typing_drops_the_filter_whether_or_not_the_keys_show() {
    let mut shell = Shell::new("TM");
    shell.key(press(KeyCode::Char('?')));
    shell.key(press(KeyCode::Char('/')));
    shell.key(press(KeyCode::Char('x')));
    assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay);
    assert_eq!(list(&shell).prompt(), None);
    assert!(shell.help, "the keys stay until Esc again");
}

#[test]
fn no_key_is_named_twice_on_a_screen() {
    let named = |table: Vec<(String, &'static str)>| -> Vec<String> {
        table
            .into_iter()
            .flat_map(|(keys, _)| keys.split(' ').map(str::to_owned).collect::<Vec<_>>())
            .collect()
    };
    let global = named(keys::help(&GLOBAL));
    for (screen, table) in [
        ("list", keys::help(&crate::list::KEYS)),
        ("detail", keys::help(&crate::detail::KEYS)),
    ] {
        let keys = named(table);
        let mut seen = std::collections::BTreeSet::new();
        for key in &keys {
            assert!(seen.insert(key), "{screen} names {key} twice");
            // Esc is the screen's own unless the keys show.
            assert!(
                key == "Esc" || !global.contains(key),
                "{screen} shadows {key}"
            );
        }
    }
}

#[test]
fn esc_hides_the_keys_before_it_leaves_an_issue() {
    let store = store();
    let mut shell = opened(&store, &Fake);
    shell.key(press(KeyCode::Char('?')));
    assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay);
    assert!(!shell.help && shell.screens.depth() > 1);
}

#[test]
fn u_undoes_this_sessions_writes_newest_first() {
    let store = triage_store();
    let record = Record::new();
    let mut shell = triaging(&store, &record);
    assert_eq!(shell.key(press(KeyCode::Char('u'))), Flow::Write);
    shell.write(&record);
    assert!(drawn_sized(&mut shell, 70, 10).contains("nothing written here to undo"));
    pick(&mut shell, 'p', "high");
    shell.write(&record);
    shell.load(&store, &record);
    pick(&mut shell, 'p', "low");
    shell.write(&record);
    shell.load(&store, &record);
    shell.key(press(KeyCode::Char('u')));
    shell.write(&record);
    assert_eq!(record.undone.borrow().len(), 1);
    assert!(drawn_sized(&mut shell, 70, 10).contains("✓ undid the write"));
}

#[test]
fn a_partly_refused_action_keeps_the_refused_marked_and_u_undoes_what_landed() {
    let store = triage_store();
    let mut record = Record::new();
    let mut shell = triaging(&store, &record);
    let first = queue_key(&mut shell).unwrap();
    shell.key(press(KeyCode::Char(' ')));
    shell.key(press(KeyCode::Char(' ')));
    record.refuse_key = Some(first);
    pick(&mut shell, 'p', "high");
    shell.write(&record);
    shell.load(&store, &record);
    let screen = drawn_sized(&mut shell, 110, 10);
    assert!(screen.contains("(1 written; u undoes them)"), "{screen}");
    assert!(screen.contains("1 marked"), "the refused one: {screen}");
    shell.key(press(KeyCode::Char('u')));
    shell.write(&record);
    assert_eq!(*record.undone.borrow(), *record.issued.borrow());
}

#[test]
fn a_write_that_changes_nothing_offers_no_undo() {
    let store = triage_store();
    let mut record = Record::new();
    let mut shell = triaging(&store, &record);
    record.unchanged_key = queue_key(&mut shell);
    pick(&mut shell, 'p', "none");
    shell.write(&record);
    let screen = drawn_sized(&mut shell, 110, 10);
    assert!(
        screen.contains("nothing changed") && !screen.contains("✓"),
        "{screen}"
    );
    shell.key(press(KeyCode::Char('u')));
    shell.write(&record);
    assert!(drawn_sized(&mut shell, 110, 10).contains("nothing written here to undo"));
}

#[test]
fn u_takes_the_newest_action_first_and_keeps_one_refused_for_another_try() {
    let store = triage_store();
    let record = Record::new();
    let mut shell = triaging(&store, &record);
    for level in ["high", "low"] {
        pick(&mut shell, 'p', level);
        shell.write(&record);
        shell.load(&store, &record);
    }
    let issued = record.issued.borrow().clone();
    record.refuse_undo.set(true);
    shell.key(press(KeyCode::Char('u')));
    shell.write(&record);
    assert!(drawn_sized(&mut shell, 110, 10).contains("✗ not undone"));
    for _ in 0..2 {
        shell.key(press(KeyCode::Char('u')));
        shell.write(&record);
    }
    assert_eq!(*record.undone.borrow(), [issued[1], issued[0]]);
}

#[test]
fn a_warning_from_the_cli_shows_with_the_write() {
    let store = triage_store();
    let record = Record::new();
    let mut shell = triaging(&store, &record);
    pick(&mut shell, 'p', "high");
    shell.write(&record);
    let screen = drawn_sized(&mut shell, 140, 10);
    assert!(
        screen.contains("warning: the Snapshot was not updated"),
        "{screen}"
    );
}

#[test]
fn a_refusal_with_a_warning_keeps_its_start_on_a_narrow_terminal() {
    let store = triage_store();
    let mut record = Record::new();
    let mut shell = triaging(&store, &record);
    shell.key(press(KeyCode::Char(' ')));
    shell.key(press(KeyCode::Char(' ')));
    record.refuse_key = Some("TM-8".into());
    pick(&mut shell, 'p', "high");
    shell.write(&record);
    let screen = drawn_sized(&mut shell, 80, 12);
    let said = screen.lines().rev().nth(1).unwrap_or_default();
    assert!(said.starts_with("\"✗ TM-8: "), "{screen}");
    assert!(said.ends_with("…\""), "cut at its end: {screen}");
    let footer = screen.lines().last().unwrap_or_default();
    assert!(
        footer.starts_with("\"q back"),
        "the keys keep their row: {screen}"
    );
}

#[test]
fn an_undo_says_the_clis_warning() {
    let store = triage_store();
    let record = Record::new();
    let mut shell = triaging(&store, &record);
    pick(&mut shell, 'p', "high");
    shell.write(&record);
    shell.key(press(KeyCode::Char('u')));
    shell.write(&record);
    let screen = drawn_sized(&mut shell, 140, 10);
    assert!(
        screen.contains("✓ undid the write · warning: a backup was not made"),
        "{screen}"
    );
}

#[test]
fn a_cut_counts_terminal_columns_and_keeps_its_ellipsis() {
    assert_eq!(cut("abc", 5), "abc");
    let wide = cut("名前名前名前", 5);
    assert!(columns(&wide) <= 5 && wide.ends_with('…'), "{wide}");
    assert_eq!(wide, "名前…");
}

#[test]
fn the_crumbs_drop_the_oldest_screens_first() {
    let titles = ["Issues".to_owned(), "Triage".to_owned(), "TM-5".to_owned()];
    assert_eq!(
        crumbs("TM", &titles, 80),
        "tm · TM › Issues › Triage › TM-5"
    );
    assert_eq!(crumbs("TM", &titles, 28), "tm · TM › … › Triage › TM-5");
    assert_eq!(crumbs("TM", &titles, 20), "tm · TM › … › TM-5");
    assert_eq!(crumbs("TM", &titles, 8), "… › TM-5");
    let one = ["Issues".to_owned()];
    assert_eq!(crumbs("TM", &one, 12), "Issues");
    for width in 0..30 {
        for listed in [&titles[..], &one[..]] {
            let line = crumbs("TM", listed, width);
            assert!(columns(&line) <= width.max(1), "{width}: {line}");
        }
    }
}

#[test]
fn a_mixed_action_names_what_landed_and_counts_what_did_not_change() {
    let store = triage_store();
    let mut record = Record::new();
    let mut shell = triaging(&store, &record);
    let first = queue_key(&mut shell).unwrap();
    shell.key(press(KeyCode::Char(' ')));
    let second = queue_key(&mut shell).unwrap();
    shell.key(press(KeyCode::Char(' ')));
    record.unchanged_key = Some(first);
    pick(&mut shell, 'p', "high");
    shell.write(&record);
    let screen = drawn_sized(&mut shell, 140, 10);
    let said = format!("✓ {second}  written · 1 unchanged · u undo");
    assert!(screen.contains(&said), "{screen}");
}

#[test]
fn u_after_an_action_that_wrote_nothing_says_it_reaches_back() {
    let store = triage_store();
    let mut record = Record::new();
    let mut shell = triaging(&store, &record);
    pick(&mut shell, 'p', "high");
    shell.write(&record);
    shell.load(&store, &record);
    record.unchanged_key = queue_key(&mut shell);
    pick(&mut shell, 'p', "high");
    shell.write(&record);
    shell.key(press(KeyCode::Char('u')));
    shell.write(&record);
    let screen = drawn_sized(&mut shell, 140, 10);
    assert!(
        screen.contains("✓ the earlier action: undid the write"),
        "{screen}"
    );
}

#[test]
fn every_refusal_and_warning_is_counted() {
    let store = triage_store();
    let record = Record {
        refuse: true,
        ..Record::new()
    };
    let mut shell = triaging(&store, &record);
    shell.key(press(KeyCode::Char(' ')));
    shell.key(press(KeyCode::Char(' ')));
    pick(&mut shell, 'p', "high");
    shell.write(&record);
    let screen = drawn_sized(&mut shell, 140, 10);
    assert!(screen.contains("(+1 more refused)"), "{screen}");
    let items = ["a", "b", "a", "c"].map(str::to_owned);
    assert_eq!(
        first_of(&items, "more warnings"),
        Some("a (+2 more warnings)".to_owned()),
        "a repeat counts once"
    );
}

#[test]
fn a_refused_undo_of_an_earlier_action_says_so() {
    let store = triage_store();
    let mut record = Record::new();
    let mut shell = triaging(&store, &record);
    pick(&mut shell, 'p', "high");
    shell.write(&record);
    shell.load(&store, &record);
    record.unchanged_key = queue_key(&mut shell);
    pick(&mut shell, 'p', "high");
    shell.write(&record);
    record.refuse_undo.set(true);
    shell.key(press(KeyCode::Char('u')));
    shell.write(&record);
    let screen = drawn_sized(&mut shell, 140, 10);
    assert!(
        screen.contains("✗ the earlier action: not undone"),
        "{screen}"
    );
}
