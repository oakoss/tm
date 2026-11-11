use tm_store::view::{Layout, View};

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
