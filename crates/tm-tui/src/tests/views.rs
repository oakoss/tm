use tm_store::view::{Column, Layout, Swimlanes, View};

use crate::{Composed, Proposal, ViewEdit};

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

/// [`Saved`], where Views may be changed, recording each change shown and
/// each saved, refusing a save when `refuse` holds a reason.
struct Editing {
    views: Vec<View>,
    may: Result<(), String>,
    refuse: Option<String>,
    proposed: std::cell::RefCell<Vec<ViewEdit>>,
    saved: std::cell::RefCell<Vec<Proposal>>,
}

impl Editing {
    fn new(may: Result<(), String>) -> Self {
        Self {
            views: saved().0.unwrap(),
            may,
            refuse: None,
            proposed: std::cell::RefCell::default(),
            saved: std::cell::RefCell::default(),
        }
    }
}

impl Actions for Editing {
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
        Ok(self.views.clone())
    }

    fn may_edit_views(&self) -> Result<(), String> {
        self.may.clone()
    }

    fn propose(&self, edit: &ViewEdit) -> Result<Proposal, String> {
        self.proposed.borrow_mut().push(edit.clone());
        Ok(Proposal {
            diff: "--- a/.config/tm.toml\n+++ b/.config/tm.toml\n-name = \"Firsts\"\n+name = \"Seconds\"\n".into(),
            before: "before".into(),
            after: "after".into(),
        })
    }

    fn save(&self, proposal: &Proposal) -> Result<String, String> {
        if let Some(why) = &self.refuse {
            return Err(why.clone());
        }
        self.saved.borrow_mut().push(proposal.clone());
        Ok("saved to .config/tm.toml".into())
    }
}

/// A draft holding `text`, as an editor would leave it.
fn draft_of(text: &str) -> Composed {
    let draft = std::env::temp_dir().join(format!("tm-View-{}.toml", Uuid::now_v7()));
    std::fs::write(&draft, text).unwrap();
    Composed {
        text: text.to_owned(),
        draft,
    }
}

/// Edits the first View to `text` and lets the editor finish.
fn edited(store: &Store, actions: &Editing, text: &str) -> (Shell, std::path::PathBuf) {
    let mut shell = sidebar(store, actions);
    assert_eq!(shell.key(press(KeyCode::Char('e'))), Flow::Compose);
    let (start, draft) = shell.drafting();
    assert_eq!(draft.suffix, "toml");
    assert!(start.contains("name = \"Firsts\""), "{start}");
    let composed = draft_of(text);
    let path = composed.draft.clone();
    shell.compose(Ok(composed));
    shell.write(actions);
    (shell, path)
}

#[test]
fn e_edits_a_view_and_y_on_its_diff_saves_it() {
    let store = store();
    let actions = Editing::new(Ok(()));
    let text = "[[views]]\nname = \"Seconds\"\nfilter = \"title:first\"\n";
    let (mut shell, draft) = edited(&store, &actions, text);
    let edit = actions.proposed.borrow()[0].clone();
    assert_eq!(
        edit.was.as_ref().map(|view| view.name.as_str()),
        Some("Firsts")
    );
    assert_eq!(edit.becomes.map(|view| view.name), Some("Seconds".into()));
    let shown = drawn_sized(&mut shell, 100, 12);
    assert!(
        shown.contains("Save this change?") && shown.contains("+name = \"Seconds\""),
        "{shown}"
    );
    assert!(actions.saved.borrow().is_empty(), "nothing saved before y");
    assert_eq!(
        shell.key(press(KeyCode::Char('j'))),
        Flow::Stay,
        "j scrolls"
    );
    assert_eq!(shell.key(press(KeyCode::Char('y'))), Flow::Write);
    shell.write(&actions);
    assert_eq!(actions.saved.borrow().len(), 1);
    assert!(drawn_sized(&mut shell, 100, 12).contains("✓ saved to .config/tm.toml"));
    assert!(!draft.exists(), "a saved View's draft is gone");
}

#[test]
fn any_key_but_y_leaves_the_view_unsaved_and_keeps_its_draft() {
    let store = store();
    let actions = Editing::new(Ok(()));
    let text = "[[views]]\nname = \"Seconds\"\n";
    for key in [KeyCode::Enter, KeyCode::Char('n'), KeyCode::Esc] {
        let (mut shell, draft) = edited(&store, &actions, text);
        assert_eq!(shell.key(press(key)), Flow::Stay, "{key:?}");
        assert!(actions.saved.borrow().is_empty(), "{key:?}");
        assert_eq!(shell.kept_drafts(), [("View", draft.clone())]);
        assert!(drawn_sized(&mut shell, 100, 12).contains("View kept"));
        std::fs::remove_file(draft).unwrap();
    }
}

#[test]
fn a_refused_save_keeps_the_draft_and_says_why() {
    let store = store();
    let mut actions = Editing::new(Ok(()));
    actions.refuse = Some("tm.toml changed since the diff was shown".into());
    let (mut shell, draft) = edited(&store, &actions, "[[views]]\nname = \"Seconds\"\n");
    shell.key(press(KeyCode::Char('y')));
    shell.write(&actions);
    assert_eq!(shell.kept_drafts(), [("View", draft.clone())]);
    let shown = drawn_sized(&mut shell, 140, 12);
    assert!(
        shown.contains("changed since the diff was shown"),
        "{shown}"
    );
    std::fs::remove_file(draft).unwrap();
}

#[test]
fn an_unchanged_or_unreadable_view_shows_no_diff() {
    let store = store();
    let actions = Editing::new(Ok(()));
    let first = saved().0.unwrap().remove(0);
    let (mut shell, draft) = edited(&store, &actions, &first.snippet());
    assert!(actions.proposed.borrow().is_empty());
    assert!(drawn_sized(&mut shell, 100, 12).contains("unchanged"));
    assert!(!draft.exists());

    let (mut shell, draft) = edited(&store, &actions, "[[views]]\nfilter = \"x\"\n");
    assert!(actions.proposed.borrow().is_empty());
    assert_eq!(shell.kept_drafts(), [("View", draft.clone())]);
    assert!(drawn_sized(&mut shell, 140, 12).contains("missing field `name`"));
    std::fs::remove_file(draft).unwrap();
}

#[test]
fn n_starts_a_new_view_and_d_removes_one_after_its_diff() {
    let store = store();
    let actions = Editing::new(Ok(()));
    let mut shell = sidebar(&store, &actions);
    assert_eq!(shell.key(press(KeyCode::Char('n'))), Flow::Compose);
    assert_eq!(shell.drafting().0, tm_store::view::BLANK);

    let mut shell = sidebar(&store, &actions);
    assert_eq!(shell.key(press(KeyCode::Char('d'))), Flow::Write);
    shell.write(&actions);
    let edit = actions.proposed.borrow()[0].clone();
    assert_eq!(edit.becomes, None);
    assert_eq!(edit.was.map(|view| view.name), Some("Firsts".into()));
    assert_eq!(shell.key(press(KeyCode::Char('y'))), Flow::Write);
    shell.write(&actions);
    assert_eq!(actions.saved.borrow().len(), 1);
}

#[test]
fn a_person_who_may_not_edit_is_told_before_any_editor_opens() {
    let store = store();
    let actions = Editing::new(Err("an Agent is shown the Views".into()));
    for key in ['e', 'n', 'd'] {
        let mut shell = sidebar(&store, &actions);
        assert_eq!(shell.key(press(KeyCode::Char(key))), Flow::Stay, "{key}");
        let shown = drawn_sized(&mut shell, 100, 12);
        assert!(
            shown.contains("✗ an Agent is shown the Views"),
            "{key}: {shown}"
        );
    }
    assert!(actions.proposed.borrow().is_empty());
}

#[test]
fn a_draft_still_waiting_on_its_diff_is_named_however_the_tui_ends() {
    let store = store();
    let actions = Editing::new(Ok(()));
    let (mut shell, draft) = edited(&store, &actions, "[[views]]\nname = \"Seconds\"\n");
    assert_eq!(shell.kept_drafts(), [("View", draft.clone())]);
    let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    assert_eq!(shell.key(ctrl('y')), Flow::Stay, "only a plain y says yes");
    assert!(actions.saved.borrow().is_empty());
    let (mut shell, draft) = edited(&store, &actions, "[[views]]\nname = \"Seconds\"\n");
    assert_eq!(shell.key(ctrl('c')), Flow::Quit);
    assert_eq!(shell.kept_drafts(), [("View", draft.clone())]);
    std::fs::remove_file(draft).unwrap();
}

#[test]
fn a_new_view_left_untouched_is_dropped_quietly() {
    let store = store();
    let actions = Editing::new(Ok(()));
    let mut shell = sidebar(&store, &actions);
    shell.key(press(KeyCode::Char('n')));
    let composed = draft_of(tm_store::view::BLANK);
    let draft = composed.draft.clone();
    shell.compose(Ok(composed));
    shell.write(&actions);
    assert!(drawn_sized(&mut shell, 100, 12).contains("no View added"));
    assert_eq!(shell.kept_drafts(), [] as [(&str, std::path::PathBuf); 0]);
    assert!(!draft.exists());
}

#[test]
fn u_after_a_view_save_names_the_earlier_action_it_undoes() {
    let store = store();
    let actions = Editing::new(Ok(()));
    let mut shell = sidebar(&store, &actions);
    shell.key(press(KeyCode::Esc));
    shell.key(press(KeyCode::Char('j')));
    shell.key(press(KeyCode::Enter));
    shell.load(&store, &actions);
    pick(&mut shell, 'p', "high");
    shell.write(&actions);
    shell.key(press(KeyCode::Char('v')));
    shell.load(&store, &actions);
    assert_eq!(shell.key(press(KeyCode::Char('d'))), Flow::Write);
    shell.write(&actions);
    shell.key(press(KeyCode::Char('y')));
    shell.write(&actions);
    assert_eq!(shell.key(press(KeyCode::Char('u'))), Flow::Write);
    shell.write(&actions);
    let shown = drawn_sized(&mut shell, 140, 12);
    assert!(shown.contains("the earlier action: "), "{shown}");
}
