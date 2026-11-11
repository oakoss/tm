use super::*;

/// `key` opened from the list.
fn open(store: &Store, key: &str) -> Shell {
    open_with(store, key, &Fake)
}

fn open_with(store: &Store, key: &str, actions: &dyn Actions) -> Shell {
    let mut shell = Shell::new("TM");
    shell.load(store, actions);
    while list(&shell).selected_key().as_deref() != Some(key) {
        shell.key(press(KeyCode::Down));
    }
    assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Reload);
    shell.load(store, actions);
    shell
}

#[test]
fn s_p_and_a_write_the_issue_shown_at_the_version_read() {
    let store = store();
    let record = Record::new();
    let mut shell = open_with(&store, "TM-2", &record);
    let read = version(&store, "TM-2");
    assert_eq!(pick(&mut shell, 's', "done"), Flow::Write);
    shell.write(&record);
    assert_eq!(pick(&mut shell, 'p', "high"), Flow::Write);
    shell.write(&record);
    assert_eq!(pick(&mut shell, 'A', "nobody"), Flow::Write);
    shell.write(&record);
    assert_eq!(
        *record.writes.borrow(),
        [
            Write::Move {
                key: "TM-2".into(),
                status: "done".into(),
                version: read,
            },
            Write::Set {
                key: "TM-2".into(),
                field: "priority".into(),
                value: Some("high".into()),
                version: read,
            },
            Write::Set {
                key: "TM-2".into(),
                field: "assignee".into(),
                value: None,
                version: read,
            },
        ]
    );
    let footer = drawn_sized(&mut shell, 120, 20);
    assert!(footer.contains("✓ TM-2  written · u undo"), "{footer}");
    assert_eq!(shell.key(press(KeyCode::Char('u'))), Flow::Write);
    shell.write(&record);
    assert_eq!(record.undone.borrow().len(), 1);
}

fn composed(text: &str) -> Composed {
    Composed {
        text: text.to_owned(),
        draft: "/drafts/one.md".into(),
    }
}

#[test]
fn c_writes_the_text_composed_as_a_comment_and_u_undoes_it() {
    let store = store();
    let record = Record::new();
    let mut shell = open_with(&store, "TM-2", &record);
    assert_eq!(shell.key(press(KeyCode::Char('c'))), Flow::Compose);
    let draft = std::env::temp_dir().join(format!("tm-comment-test-{}.md", Uuid::now_v7()));
    std::fs::write(&draft, "saved").unwrap();
    shell.compose(Ok(Composed {
        text: "# Plan\n\n- \u{1b}[2Jfirst\n".into(),
        draft: draft.clone(),
    }));
    shell.write(&record);
    assert!(!draft.exists(), "the draft of a comment written goes");
    assert_eq!(
        *record.writes.borrow(),
        [Write::Comment {
            key: "TM-2".into(),
            body: "# Plan\n\n- \u{1b}[2Jfirst\n".into(),
        }]
    );
    assert_eq!(shell.kept_drafts(), &[] as &[PathBuf]);
    assert!(drawn_sized(&mut shell, 120, 20).contains("✓ TM-2  written · u undo"));
    assert_eq!(shell.key(press(KeyCode::Char('u'))), Flow::Write);
    shell.write(&record);
    assert_eq!(record.undone.borrow().len(), 1);
}

#[test]
fn a_comment_not_written_keeps_its_draft_and_says_why() {
    let store = store();
    let record = Record {
        refuse: true,
        ..Record::new()
    };
    let mut shell = open_with(&store, "TM-2", &record);
    shell.key(press(KeyCode::Char('c')));
    shell.compose(Ok(composed("a long careful comment")));
    shell.write(&record);
    let screen = drawn_sized(&mut shell, 80, 20);
    assert!(
        screen.contains("✗ comment kept, its path printed on exit · TM-2: TM-2 changed"),
        "the reason shows at 80 columns: {screen}"
    );
    shell.key(press(KeyCode::Char('c')));
    shell.compose(Err(Unwritten {
        why: "vi exited with exit status: 1".into(),
        kept: Some("/drafts/two.md".into()),
    }));
    let screen = drawn_sized(&mut shell, 80, 20);
    assert!(
        screen.contains("✗ comment kept, its path printed on exit · vi exited"),
        "{screen}"
    );
    assert_eq!(
        shell.kept_drafts(),
        [PathBuf::from("/drafts/one.md"), "/drafts/two.md".into()]
    );
}

#[test]
fn an_editor_giving_no_comment_writes_nothing_and_says_why() {
    let store = store();
    let record = Record::new();
    let mut shell = open_with(&store, "TM-2", &record);
    shell.key(press(KeyCode::Char('c')));
    shell.compose(Err(Unwritten {
        why: "vi exited with \u{1b}[2Jexit status: 1\nmore".to_owned(),
        kept: None,
    }));
    shell.write(&record);
    assert_eq!(shell.kept_drafts(), &[] as &[PathBuf]);
    let screen = drawn_sized(&mut shell, 120, 20);
    assert!(
        screen.contains(r"✗ no comment written: vi exited with \u{1b}[2Jexit status: 1"),
        "{screen}"
    );
    assert!(!screen.contains("more"), "{screen}");
    assert!(record.writes.borrow().is_empty());
}

#[test]
fn the_status_picker_offers_what_tm_show_says_it_moves_to() {
    let store = store();
    let mut shell = open(&store, "TM-2");
    shell.key(press(KeyCode::Char('s')));
    let picker = drawn_sized(&mut shell, 120, 30);
    assert!(
        picker.contains("┌ Status") && picker.contains("Done"),
        "{picker}"
    );
    assert!(
        !picker.contains("│Todo"),
        "not the Status it is in: {picker}"
    );
    assert!(drawn_sized(&mut shell, 120, 30).contains("Status type to narrow"));
}

#[test]
fn a_picker_takes_question_mark_and_esc_as_its_own_keys() {
    let store = store();
    let record = Record::new();
    let mut issue = open_with(&store, "TM-2", &record);
    let mut board = Shell::new("TM");
    board.load(&store, &record);
    assert_eq!(board.key(press(KeyCode::Char('b'))), Flow::Reload);
    board.load(&store, &record);
    for (name, shell) in [("Issue", &mut issue), ("Board", &mut board)] {
        shell.key(press(KeyCode::Char('s')));
        assert_eq!(shell.key(press(KeyCode::Char('?'))), Flow::Stay, "{name}");
        assert!(!shell.help, "? is text in the {name} picker");
        assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay, "{name}");
        assert_eq!(
            shell.key(press(KeyCode::Char('q'))),
            Flow::Reload,
            "q goes back from the {name} once the picker is gone"
        );
    }
    assert!(record.writes.borrow().is_empty());
}

#[test]
fn a_picker_opened_over_the_keys_takes_question_mark_and_esc_too() {
    let store = store();
    let record = Record::new();
    let mut shell = open_with(&store, "TM-2", &record);
    shell.key(press(KeyCode::Char('?')));
    assert!(shell.help);
    shell.key(press(KeyCode::Char('s')));
    shell.key(press(KeyCode::Char('?')));
    assert!(shell.help, "? is text in the picker, not a toggle");
    assert!(drawn_sized(&mut shell, 120, 30).contains("Status /?"));
    shell.key(press(KeyCode::Esc));
    assert!(shell.help, "Esc cancels the picker first");
    assert!(!drawn_sized(&mut shell, 120, 30).contains("Status /?"));
    shell.key(press(KeyCode::Esc));
    assert!(!shell.help, "then hides the keys");
    assert!(record.writes.borrow().is_empty());
}

/// The Key of the related Issue selected on the Issue shown.
fn related_key(shell: &mut Shell) -> Option<String> {
    match shell.shown() {
        Screen::Issue(issue) => issue.selected_key(),
        _ => unreachable!("an Issue is shown"),
    }
}

/// `child`'s parent set to `parent`, as another process would.
fn parented(store: &mut Store, child: &str, parent: &str) {
    let parent = store.issue_by_key(parent).unwrap().unwrap();
    set(store, child, "parent", &parent.to_string());
}

/// TM-2 with TM-1 as its parent, opened from the list.
fn with_parent() -> (Store, Shell) {
    let mut store = store();
    parented(&mut store, "TM-2", "TM-1");
    let shell = open(&store, "TM-2");
    (store, shell)
}

#[test]
fn enter_opens_the_issue_laid_out_from_what_the_cli_reports() {
    let store = store();
    let mut shell = open(&store, "TM-2");
    insta::assert_snapshot!(drawn(&mut shell));
}

#[test]
fn the_description_is_markdown_and_the_related_issues_sit_beside_it() {
    let (_store, mut shell) = with_parent();
    let screen = drawn_sized(&mut shell, 110, 16);
    insta::assert_snapshot!(screen);
    assert!(screen.contains("• line 1 "), "{screen}");
    assert!(screen.contains("parent  TM-1  todo  first"), "{screen}");
    let narrow = drawn_sized(&mut shell, 60, 16);
    assert!(
        narrow.contains("parent  TM-1"),
        "below on a narrow terminal: {narrow}"
    );
}

#[test]
fn enter_opens_a_related_issue_and_back_returns_to_this_one() {
    let (store, mut shell) = with_parent();
    assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Reload);
    shell.load(&store, &Fake);
    let parent = drawn_sized(&mut shell, 110, 16);
    assert!(
        parent.contains("tm · TM › Issues › TM-2 › TM-1"),
        "{parent}"
    );
    assert!(parent.contains("child  TM-2  todo  second"), "{parent}");
    assert_eq!(shell.key(press(KeyCode::Char('q'))), Flow::Reload);
    shell.load(&store, &Fake);
    assert!(drawn_sized(&mut shell, 110, 16).contains("┌ TM-2 "));
}

#[test]
fn tab_moves_between_related_issues_and_wraps() {
    let (mut store, _) = with_parent();
    parented(&mut store, "TM-3", "TM-1");
    let mut shell = open(&store, "TM-1");
    let first = related_key(&mut shell);
    shell.key(press(KeyCode::Tab));
    let second = related_key(&mut shell);
    assert_ne!(first, second);
    shell.key(press(KeyCode::Tab));
    assert_eq!(
        related_key(&mut shell),
        first,
        "two children, so two Tabs wrap"
    );
    shell.key(press(KeyCode::BackTab));
    assert_eq!(related_key(&mut shell), second, "and back the other way");
    shell.key(press(KeyCode::Char('n')));
    assert_eq!(related_key(&mut shell), first);
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
        bottom.contains("  created") && !bottom.contains("line 29"),
        "{bottom}"
    );
    shell.key(press(KeyCode::Char('g')));
    assert!(drawn(&mut shell).contains("second"));
    shell.key(press(KeyCode::Char('G')));
    assert_eq!(drawn(&mut shell), bottom);
    shell.key(press(KeyCode::Char('k')));
    shell.load(&store, &Fake);
    let reread = drawn(&mut shell);
    assert!(
        reread.contains("History") && !reread.contains("  created"),
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

/// [`Fake`], reporting TM-2 with a Claim, a Link to TM-3 and one to an
/// Issue the store does not have, a question, its edited answer, a
/// retracted comment, and seven writes.
struct Rich;

impl Actions for Rich {
    fn me(&self) -> Result<&str, &str> {
        Fake.me()
    }

    fn issue(&self, store: &Store, id: Uuid) -> Result<Issue, String> {
        let mut data = reported(store, id)?;
        if data["shown"]["key"] == "TM-2" {
            let linked = store.issue_by_key("TM-3").unwrap().unwrap();
            let shown = &mut data["shown"];
            shown["claim"] = serde_json::json!({
                "agent": "claude",
                "on_behalf_of": "jace",
                "branch": "main",
                "expires_ms": 120_000,
                "expires_in_ms": 120_000,
                "handed": false,
            });
            shown["links"] = serde_json::json!([
                { "name": "blocks", "other": { "id": linked, "key": "TM-3" } },
                { "name": "is blocked by", "other": { "id": Uuid::now_v7(), "key": "TM-9" } },
            ]);
            let said = |body: Option<&str>, role: Option<&str>| {
                serde_json::json!({
                    "actor": "kim",
                    "on_behalf_of": null,
                    "body": body,
                    "utc_ms": 0,
                    "retracted": body.is_none(),
                    "role": role,
                    "edited_utc_ms": null,
                })
            };
            let mut question = said(Some("which store?"), Some("question"));
            question["options"] = serde_json::json!(["SQLite", "Postgres"]);
            let mut answer = said(Some("**SQLite**"), Some("answer"));
            answer["edited_utc_ms"] = serde_json::json!(5);
            shown["comments"] = serde_json::json!([question, answer, said(None, None)]);
            let writes: Vec<_> = (1..=7)
                .map(|n| {
                    serde_json::json!({
                        "actor": "jace",
                        "on_behalf_of": null,
                        "utc_ms": 0,
                        "changes": [{ "line": format!("change {n}") }],
                    })
                })
                .collect();
            data["history"] = serde_json::json!({ "total": 7, "writes": writes[2..] });
        }
        serde_json::from_value(data).map_err(|err| err.to_string())
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

fn rich(store: &Store) -> Shell {
    let mut shell = Shell::new("TM");
    shell.load(store, &Rich);
    while list(&shell).selected_key().as_deref() != Some("TM-2") {
        shell.key(press(KeyCode::Down));
    }
    shell.key(press(KeyCode::Enter));
    shell.load(store, &Rich);
    shell
}

#[test]
fn the_claim_comments_and_latest_writes_read_as_the_cli_reports_them() {
    let store = store();
    let mut shell = rich(&store);
    let screen = drawn_sized(&mut shell, 140, 70);
    for said in [
        "Claim   claude for jace on main, 2m left",
        "kim asked",
        "which store?",
        "options: 1. SQLite  2. Postgres",
        "(edited)",
        "  retracted",
        "2 earlier: tm history TM-2",
    ] {
        assert!(screen.contains(said), "{said}: {screen}");
    }
    let answered = screen.find("kim answered").unwrap();
    assert!(screen[answered..].contains("  SQLite"), "{screen}");
    let newest = screen.find("change 7").unwrap();
    assert!(
        screen.find("change 3") > Some(newest),
        "newest first: {screen}"
    );
    assert!(!screen.contains("change 2"), "{screen}");
}

#[test]
fn a_link_opens_and_an_issue_the_store_lacks_shows_as_missing() {
    let store = store();
    let mut shell = rich(&store);
    let screen = drawn_sized(&mut shell, 140, 20);
    assert!(
        screen.contains("blocks         TM-3  todo  third"),
        "{screen}"
    );
    assert!(screen.contains("is blocked by  TM-9  missing"), "{screen}");
    assert_eq!(related_key(&mut shell).as_deref(), Some("TM-3"));
    assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Reload);
    shell.load(&store, &Rich);
    assert!(drawn_sized(&mut shell, 140, 20).contains("› TM-2 › TM-3"));
}

#[test]
fn an_archived_child_is_listed_and_marked() {
    let (mut store, _) = with_parent();
    let child = store.issue_by_key("TM-2").unwrap().unwrap();
    let project = store.project_by_key("TM").unwrap().unwrap();
    let change = Change::SetField {
        field: "archived".into(),
        label: String::new(),
        value: serde_json::json!(true),
        value_label: None,
    };
    let draft = Draft {
        actor: Actor::human("jace"),
        project,
        subject: child,
        change,
    };
    store.append(draft).unwrap();
    let mut shell = open(&store, "TM-1");
    let screen = drawn_sized(&mut shell, 140, 20);
    assert!(screen.contains("child  TM-2  todo, archived"), "{screen}");
}

#[test]
fn the_keys_name_shift_tab() {
    let store = store();
    let mut shell = open(&store, "TM-2");
    shell.key(press(KeyCode::Char('?')));
    assert!(drawn_sized(&mut shell, 80, 24).contains("Tab n Shift-Tab N"));
}
