use super::*;

/// `key` opened from the list.
fn open(store: &Store, key: &str) -> Shell {
    let mut shell = Shell::new("TM");
    shell.load(store, &Fake);
    while list(&shell).selected_key().as_deref() != Some(key) {
        shell.key(press(KeyCode::Down));
    }
    assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Reload);
    shell.load(store, &Fake);
    shell
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

    fn detail(&self, store: &Store, id: Uuid) -> Result<String, String> {
        Fake.detail(store, id)
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
                "expires_in_ms": 120_000,
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
