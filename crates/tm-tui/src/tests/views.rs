use tm_store::view::{Column, Layout, Swimlanes, View};

use super::*;

/// [`Fake`], with the Views the repo file holds or why it cannot be read.
struct Saved(Result<Vec<View>, String>);

impl Actions for Saved {
    fn write(&self, write: &Write) -> Result<Written, String> {
        Fake.write(write)
    }

    fn undo(&self, batch: Uuid) -> Result<Written, String> {
        Fake.undo(batch)
    }

    fn me(&self) -> Result<&str, &str> {
        NOBODY
    }

    fn issue(&self, store: &Store, id: Uuid) -> Result<Issue, String> {
        laid_out(store, id)
    }

    fn ready(&self, store: &Store) -> Result<String, String> {
        Fake.ready(store)
    }

    fn views(&self) -> Result<Vec<View>, String> {
        self.0.clone()
    }
}

fn view(name: &str, filter: &str, layout: Layout) -> View {
    View {
        name: name.into(),
        filter: filter.into(),
        layout,
        project: None,
        columns: Vec::new(),
        swimlanes: None,
    }
}

fn saved() -> Saved {
    Saved(Ok(vec![
        view("Firsts", "title:first", Layout::List),
        view("Flow", "", Layout::Board),
    ]))
}

fn sidebar(store: &Store, actions: &dyn Actions) -> Shell {
    let mut shell = Shell::new("TM");
    shell.load(store, actions);
    assert_eq!(shell.key(press(KeyCode::Char('v'))), Flow::Reload);
    shell.load(store, actions);
    shell
}

#[test]
fn v_lists_the_views_and_enter_opens_a_list_view_with_its_filter() {
    let store = store();
    let views = saved();
    let mut shell = sidebar(&store, &views);
    let shown = drawn_sized(&mut shell, 100, 12);
    assert!(
        shown.contains("┌ Views") && shown.contains("Firsts") && shown.contains("Flow · board"),
        "{shown}"
    );
    assert!(shown.contains("Enter open"), "the footer says so: {shown}");
    assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Reload);
    shell.load(&store, &views);
    let opened = drawn_sized(&mut shell, 100, 12);
    assert!(opened.contains("tm · TM › Issues › Firsts"), "{opened}");
    assert!(
        opened.contains("first") && !opened.contains("second"),
        "{opened}"
    );
    assert_eq!(shell.key(press(KeyCode::Char('q'))), Flow::Reload);
    assert!(drawn_sized(&mut shell, 100, 12).contains("second"));
}

#[test]
fn a_board_view_opens_the_board_narrowed_by_its_filter() {
    let store = store();
    let views = Saved(Ok(vec![view("Flow", "title:first", Layout::Board)]));
    let mut shell = sidebar(&store, &views);
    shell.key(press(KeyCode::Enter));
    shell.load(&store, &views);
    let board = drawn_sized(&mut shell, 140, 12);
    assert!(
        board.contains("› Flow") && board.contains("TM-1"),
        "{board}"
    );
    assert!(!board.contains("TM-2"), "{board}");
}

#[test]
fn the_sidebar_moves_closes_and_says_when_there_are_none_or_why() {
    let store = store();
    let views = saved();
    let mut shell = sidebar(&store, &views);
    shell.key(press(KeyCode::Char('j')));
    shell.key(press(KeyCode::Char('j')));
    shell.key(press(KeyCode::Enter));
    shell.load(&store, &views);
    assert!(drawn_sized(&mut shell, 140, 12).contains("› Flow"));

    let mut shell = sidebar(&store, &views);
    assert_eq!(shell.key(press(KeyCode::Char('?'))), Flow::Stay);
    assert!(!shell.help, "the sidebar takes the keys");
    assert_eq!(shell.key(press(KeyCode::Esc)), Flow::Stay);
    assert!(!drawn_sized(&mut shell, 100, 12).contains("┌ Views"));

    let none = Saved(Ok(Vec::new()));
    let mut shell = sidebar(&store, &none);
    assert!(drawn_sized(&mut shell, 100, 12).contains("No Views yet"));
    assert_eq!(shell.key(press(KeyCode::Enter)), Flow::Stay);

    let broken = Saved(Err("line 3: unknown field `sort`".into()));
    let mut shell = sidebar(&store, &broken);
    let said = drawn_sized(&mut shell, 100, 12);
    assert!(said.contains("cannot read the"), "{said}");
}

#[test]
fn a_board_view_naming_a_state_puts_finished_issues_in_their_own_column() {
    let store = store();
    let views = Saved(Ok(vec![view("Shipped", "status:done", Layout::Board)]));
    let mut shell = sidebar(&store, &views);
    shell.key(press(KeyCode::Enter));
    shell.load(&store, &views);
    let Screen::Board(board) = shell.shown() else {
        panic!("a Board is shown");
    };
    assert_eq!(board.cursor(), ("done".to_owned(), Some("TM-4".to_owned())));
}

fn column(status: &str, filter: &str) -> Column {
    Column {
        status: status.into(),
        filter: filter.into(),
    }
}

#[test]
fn a_board_views_columns_are_its_statuses_in_order_each_narrowed_by_its_filter() {
    let store = store();
    let mut flow = view("Flow", "-title:second", Layout::Board);
    flow.columns = vec![column("done", ""), column("todo", "-title:third")];
    let views = Saved(Ok(vec![flow]));
    let mut shell = sidebar(&store, &views);
    shell.key(press(KeyCode::Enter));
    shell.load(&store, &views);
    let board = drawn_sized(&mut shell, 140, 12);
    let done = board.find("Done").expect("a Done column");
    let todo = board.find("Todo").expect("a Todo column");
    assert!(done < todo, "in the View's order: {board}");
    assert!(
        !board.contains("Triage") && !board.contains("In Progress"),
        "{board}"
    );
    assert!(
        board.contains("TM-4") && board.contains("TM-1"),
        "a listed done column shows done work: {board}"
    );
    assert!(
        !board.contains("TM-2") && !board.contains("TM-3"),
        "the column's filter and the View's both narrow it: {board}"
    );
}

#[test]
fn a_filter_error_names_the_view_or_the_column_it_is_in() {
    let store = store();
    for (filter, todo, said) in [
        ("titel:x", "", "Flow: unknown field \"titel\""),
        ("title:\"first", "x\"", "Flow: unterminated quote"),
        ("", "titel:x", "Flow, column todo: unknown field \"titel\""),
    ] {
        let mut flow = view("Flow", filter, Layout::Board);
        flow.columns = vec![column("todo", todo), column("done", "")];
        let views = Saved(Ok(vec![flow]));
        let mut shell = sidebar(&store, &views);
        shell.key(press(KeyCode::Enter));
        shell.load(&store, &views);
        let board = drawn_sized(&mut shell, 160, 12);
        assert!(board.contains(said), "{filter} / {todo}: {board}");
    }
}

#[test]
fn h_and_l_move_to_the_status_of_the_column_beside() {
    let store = store();
    let mut flow = view("Flow", "", Layout::Board);
    flow.columns = vec![column("done", ""), column("todo", "")];
    let record = Record::new();
    let views = Saved(Ok(vec![flow]));
    let mut shell = sidebar(&store, &views);
    shell.key(press(KeyCode::Enter));
    shell.load(&store, &views);
    shell.key(press(KeyCode::Char('l')));
    assert_eq!(shell.key(press(KeyCode::Char('H'))), Flow::Write);
    shell.write(&record);
    let moved = record.writes.borrow();
    assert!(
        matches!(moved.as_slice(), [Write::Move { status, .. }] if status == "done"),
        "{moved:?}"
    );
    drop(moved);
    assert_eq!(
        shell.key(press(KeyCode::Char('L'))),
        Flow::Stay,
        "no column beyond the last"
    );
}

fn swimlaned(field: &str, filter: &str) -> Saved {
    let mut flow = view("Flow", "", Layout::Board);
    flow.swimlanes = Some(Swimlanes {
        field: field.into(),
        filter: filter.into(),
    });
    Saved(Ok(vec![flow]))
}

fn opened(store: &Store, views: &Saved) -> Shell {
    let mut shell = sidebar(store, views);
    shell.key(press(KeyCode::Enter));
    shell.load(store, views);
    shell
}

#[test]
fn swimlanes_band_the_board_by_a_fields_value_unset_last() {
    let store = store();
    let views = swimlaned("assignee", "");
    let mut shell = opened(&store, &views);
    let board = drawn_sized(&mut shell, 140, 16);
    let jace = board.find("── jace ──").expect("a band for jace");
    let none = board.find("── none ──").expect("a band for none");
    assert!(jace < none, "unset last: {board}");
    let tm3 = board.find("TM-3").expect("TM-3, assigned to jace");
    let tm1 = board.find("TM-1").expect("TM-1, assigned to no one");
    assert!(jace < tm3 && tm3 < none && none < tm1, "{board}");

    let views = swimlaned("assignee", "-assignee:jace");
    let mut shell = opened(&store, &views);
    let board = drawn_sized(&mut shell, 140, 16);
    assert!(
        !board.contains("── jace ──") && !board.contains("TM-3"),
        "the swimlanes' filter narrows the Board: {board}"
    );

    let views = swimlaned("owner", "");
    let mut shell = opened(&store, &views);
    let board = drawn_sized(&mut shell, 140, 16);
    assert!(board.contains("Flow: no field \"owner\""), "{board}");
}

#[test]
fn a_band_starts_on_the_same_row_in_every_column() {
    let mut store = store();
    set(&mut store, "TM-1", "status", "in_progress");
    let views = swimlaned("assignee", "");
    let mut shell = opened(&store, &views);
    let board = drawn_sized(&mut shell, 140, 16);
    assert!(
        board
            .lines()
            .any(|row| row.matches("── none ──").count() == 4),
        "{board}"
    );
}

fn label(store: &mut Store, key: &str, label: &str) {
    let id = store.issue_by_key(key).unwrap().unwrap();
    let project = store.project_by_key("TM").unwrap().unwrap();
    let add = Change::Add {
        field: "labels".into(),
        label: String::new(),
        elem: label.into(),
        elem_label: None,
    };
    store
        .append(Draft {
            actor: Actor::human("jace"),
            project,
            subject: id,
            change: add,
        })
        .unwrap();
}

/// The band names drawn, top to bottom, in the first column showing them.
fn bands(board: &str) -> Vec<String> {
    board
        .lines()
        .filter_map(|row| {
            let at = row.find("── ")?;
            let rest = &row[at + "── ".len()..];
            Some(rest[..rest.find(" ──")?].to_owned())
        })
        .collect()
}

#[test]
fn bands_follow_the_schemes_order_and_read_sets_too() {
    let mut store = store();
    set(&mut store, "TM-1", "priority", "high");
    set(&mut store, "TM-2", "priority", "urgent");
    let views = swimlaned("priority", "");
    let mut shell = opened(&store, &views);
    let drawn = drawn_sized(&mut shell, 140, 16);
    assert_eq!(bands(&drawn), ["urgent", "high", "none"], "{drawn}");

    label(&mut store, "TM-1", "ui");
    let views = swimlaned("labels", "");
    let mut shell = opened(&store, &views);
    let drawn = drawn_sized(&mut shell, 140, 16);
    assert_eq!(bands(&drawn), ["ui", "none"], "{drawn}");
}

#[test]
fn a_quote_cannot_run_from_the_views_filter_into_the_swimlanes() {
    let store = store();
    let mut flow = view("Flow", "title:\"fir", Layout::Board);
    flow.swimlanes = Some(Swimlanes {
        field: "assignee".into(),
        filter: "st\"".into(),
    });
    let views = Saved(Ok(vec![flow]));
    let mut shell = opened(&store, &views);
    let drawn = drawn_sized(&mut shell, 140, 12);
    assert!(drawn.contains("Flow: unterminated quote"), "{drawn}");
}

#[test]
fn swimlanes_band_a_board_views_own_columns_too() {
    let store = store();
    let mut flow = view("Flow", "", Layout::Board);
    flow.columns = vec![column("todo", "")];
    flow.swimlanes = Some(Swimlanes {
        field: "assignee".into(),
        filter: String::new(),
    });
    let views = Saved(Ok(vec![flow]));
    let mut shell = opened(&store, &views);
    let drawn = drawn_sized(&mut shell, 140, 12);
    assert_eq!(bands(&drawn), ["jace", "none"], "{drawn}");
}

#[test]
fn the_bands_never_scroll_past_their_rows() {
    let mut store = store();
    let jace = Actor::human("jace");
    let project = store.project_by_key("TM").unwrap().unwrap();
    for n in 0..12 {
        store
            .create_issue(&jace, project, "task", &format!("more {n}"))
            .unwrap();
    }
    let views = swimlaned("assignee", "");
    let mut shell = opened(&store, &views);
    shell.key(press(KeyCode::Char('G')));
    drawn_sized(&mut shell, 140, 8);
    shell.key(press(KeyCode::Char('l')));
    for n in 4..=16 {
        set(&mut store, &format!("TM-{n}"), "status", "done");
    }
    shell.load(&store, &views);
    let drawn = drawn_sized(&mut shell, 140, 9);
    assert!(
        drawn.contains("TM-1") && drawn.contains("── jace ──"),
        "{drawn}"
    );
}

#[test]
fn scrolling_back_up_to_a_bands_first_card_shows_its_name() {
    let mut store = store();
    let jace = Actor::human("jace");
    let project = store.project_by_key("TM").unwrap().unwrap();
    for n in 0..12 {
        store
            .create_issue(&jace, project, "task", &format!("more {n}"))
            .unwrap();
    }
    let views = swimlaned("assignee", "");
    let mut shell = opened(&store, &views);
    shell.key(press(KeyCode::Char('G')));
    let bottom = drawn_sized(&mut shell, 140, 8);
    assert!(!bottom.contains("── jace ──"), "{bottom}");
    shell.key(press(KeyCode::Char('g')));
    let top = drawn_sized(&mut shell, 140, 8);
    assert!(top.contains("── jace ──") && top.contains("TM-3"), "{top}");
}

#[test]
fn j_steps_down_through_the_bands_as_drawn() {
    let mut store = store();
    set(&mut store, "TM-2", "assignee", "zed");
    set(&mut store, "TM-1", "assignee", "amy");
    let views = swimlaned("assignee", "");
    let mut shell = opened(&store, &views);
    let mut seen = Vec::new();
    for _ in 0..3 {
        let Screen::Board(board) = shell.shown() else {
            unreachable!("a Board is shown");
        };
        seen.push(board.cursor().1.unwrap());
        shell.key(press(KeyCode::Char('j')));
    }
    assert_eq!(seen, ["TM-1", "TM-3", "TM-2"], "amy, jace, then zed");
}

#[test]
fn the_bands_scroll_together_to_keep_the_cursor_in_view() {
    let store = store();
    let views = swimlaned("assignee", "");
    let mut shell = opened(&store, &views);
    let tall = drawn_sized(&mut shell, 140, 16);
    assert!(tall.contains("── jace ──"), "{tall}");
    shell.key(press(KeyCode::Char('G')));
    let short = drawn_sized(&mut shell, 140, 7);
    let last = match shell.shown() {
        Screen::Board(board) => board.cursor().1.unwrap(),
        _ => unreachable!("a Board is shown"),
    };
    assert!(
        short.contains(&last),
        "the cursor's {last} is in view: {short}"
    );
    assert!(
        !short.contains("── jace ──"),
        "the first band scrolled away: {short}"
    );
    assert!(
        short.contains("── none ──"),
        "the cursor's band keeps its name: {short}"
    );
}

#[test]
fn a_column_naming_no_status_says_so() {
    let store = store();
    let mut flow = view("Flow", "", Layout::Board);
    flow.columns = vec![column("todo", ""), column("shipped", "")];
    let views = Saved(Ok(vec![flow]));
    let mut shell = sidebar(&store, &views);
    shell.key(press(KeyCode::Enter));
    shell.load(&store, &views);
    let board = drawn_sized(&mut shell, 140, 12);
    assert!(board.contains("Flow: no Status \"shipped\""), "{board}");
}

#[test]
fn what_opens_over_the_keys_hides_them_as_it_takes_their_place() {
    let store = store();
    let views = saved();
    let mut shell = Shell::new("TM");
    shell.load(&store, &views);
    shell.key(press(KeyCode::Char('?')));
    shell.key(press(KeyCode::Char('v')));
    assert!(!shell.help, "the sidebar takes the keys");

    let mut shell = Shell::new("TM");
    shell.load(&store, &views);
    shell.key(press(KeyCode::Char('b')));
    shell.load(&store, &views);
    shell.key(press(KeyCode::Char('?')));
    shell.key(press(KeyCode::Char('s')));
    assert!(!shell.help, "the picker takes the keys");
}

#[test]
fn v_is_text_while_a_filter_is_typed() {
    let store = store();
    let views = saved();
    let mut shell = Shell::new("TM");
    shell.load(&store, &views);
    shell.key(press(KeyCode::Char('/')));
    assert_eq!(shell.key(press(KeyCode::Char('v'))), Flow::Stay);
    assert_eq!(list(&shell).prompt().as_deref(), Some("/v"));
}
