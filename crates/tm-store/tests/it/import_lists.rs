//! Record import of Short-lists: lists found by source, then name, or made,
//! and each record's place compared three ways (ADR 0013, ADR 0016).

use serde_json::{Value, json};
use tm_store::{PassedOver, Place, RecordsImported, ShortlistEdit, Store, StoreError};
use uuid::Uuid;

use crate::support::{
    ManualClock, OTHER, PROJECT, TempDir, draft, field, jace, new_issue, open, set_field,
};

fn store(dir: &TempDir) -> Store {
    let mut store = open(&dir.db(), &ManualClock::at(1_900_000_000_000));
    store.set_workspace_id(Uuid::now_v7()).unwrap();
    store
}

fn import(store: &mut Store, lines: &[Value]) -> Result<RecordsImported, StoreError> {
    let bytes: Vec<u8> = lines
        .iter()
        .flat_map(|line| {
            let mut bytes = serde_json::to_vec(line).unwrap();
            bytes.push(b'\n');
            bytes
        })
        .collect();
    store.import_records(&mut bytes.as_slice(), &jace())
}

fn project(lists: &Value) -> Value {
    json!({"_type": "project", "key": "CPL", "name": "Coupling", "shortlists": lists})
}

/// A record from `x:<n>`, on `list` at `position` when it names one.
fn record(n: u32, place: Option<(&str, u64)>) -> Value {
    let mut record = json!({
        "_type": "issue", "source": format!("x:{n}"), "project": "CPL", "type": "task",
        "title": format!("t{n}"), "status": "todo", "reporter": "jace",
    });
    if let Some((list, position)) = place {
        let source = format!("x:{list}");
        record["shortlist"] = json!({"source": source, "name": list, "position": position});
    }
    record
}

fn issue(imported: &RecordsImported, n: u32) -> Uuid {
    issue_from(imported, &format!("x:{n}"))
}

fn order(store: &Store, name: &str) -> Vec<Uuid> {
    let list = store.shortlist_named(PROJECT, name).unwrap();
    let members = store.shortlist_view(list.id).unwrap();
    members.into_iter().map(|member| member.issue.id).collect()
}

fn create(store: &mut Store, name: &str) -> Uuid {
    let actors = ["jace".to_owned()];
    let created = store.create_shortlist(&jace(), PROJECT, name, &actors, None);
    created.unwrap().0
}

#[test]
fn a_file_makes_its_lists_and_places_its_issues_in_order() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let docs = json!([{"source": "x:docs", "name": "docs", "actors": ["codex"], "size": 4}]);
    let lines = [
        project(&docs),
        record(1, Some(("docs", 2))),
        record(2, Some(("docs", 1))),
        record(3, Some(("later", 1))),
        record(4, None),
    ];
    let imported = import(&mut store, &lines).unwrap();
    let docs = store.shortlist_named(PROJECT, "docs").unwrap();
    assert_eq!((docs.actors, docs.size), (vec!["codex".to_owned()], 4));
    let (one, two) = (issue(&imported, 1), issue(&imported, 2));
    assert_eq!(order(&store, "docs"), [two, one]);
    let later = store.shortlist_named(PROJECT, "later").unwrap();
    assert_eq!((later.actors, later.size), (vec!["jace".to_owned()], 10));
    assert_eq!(order(&store, "later"), [issue(&imported, 3)]);
    assert!(imported.shortlists.iter().all(|list| list.created));
    let written = imported.records[1].written.last();
    assert_eq!(written.map(String::as_str), Some("shortlist"));

    let before = store.history(one).unwrap().len();
    let again = import(&mut store, &lines).unwrap();
    let quiet = |list: &tm_store::ListOutcome| !list.created && list.batches.is_empty();
    assert!(again.shortlists.iter().all(quiet), "{again:?}");
    assert!(again.records.iter().all(|record| record.batches.is_empty()));
    assert_eq!(store.history(one).unwrap().len(), before);
}

#[test]
fn a_list_matched_by_name_gains_the_source_and_is_found_by_it_after() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let docs = create(&mut store, "Docs");
    let lines = [record(1, Some(("docs", 1)))];
    let imported = import(&mut store, &lines).unwrap();
    let list = &imported.shortlists[0];
    assert_eq!((list.list, list.by_name, list.created), (docs, true, false));
    let sourced: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM ops WHERE kind = 'shortlist_source' AND source = 'x:docs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(sourced, 1);

    let edit = ShortlistEdit {
        name: Some("renamed"),
        ..Default::default()
    };
    store.edit_shortlist(&jace(), docs, &edit).unwrap();
    let again = import(&mut store, &lines).unwrap();
    let list = &again.shortlists[0];
    assert_eq!((list.list, list.by_name), (docs, false));
    assert_eq!(list.batches, Vec::<Uuid>::new(), "{list:?}");
    assert_eq!(list.conflicts, Vec::new(), "only tm renamed it");
    assert_eq!(list.name.as_deref(), Some("renamed"), "tm's name is kept");
    assert_eq!(store.shortlist_named(PROJECT, "renamed").unwrap().id, docs);
}

/// ADR 0016: members the file and the list share keep tm's order, and one
/// joining goes after the nearest the file puts before it.
#[test]
fn an_issue_joins_beside_the_members_the_file_shares_with_the_list() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let unplaced = [record(1, None), record(2, None), record(3, None)];
    let first = import(&mut store, &unplaced).unwrap();
    let [a, b, c] = [1, 2, 3].map(|n| issue(&first, n));
    let docs = create(&mut store, "docs");
    let tm_only = new_issue(&mut store);
    let members = [a, tm_only, b];
    let added = store.shortlist_add(&jace(), docs, &members, Place::End);
    added.unwrap();

    let lines = [
        record(2, Some(("docs", 1))),
        record(3, Some(("docs", 2))),
        record(1, Some(("docs", 3))),
    ];
    import(&mut store, &lines).unwrap();
    assert_eq!(order(&store, "docs"), [a, tm_only, b, c]);
}

#[test]
fn a_place_is_compared_three_ways() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let on = [record(1, Some(("docs", 1))), record(2, Some(("docs", 2)))];
    let imported = import(&mut store, &on).unwrap();
    let (one, two) = (issue(&imported, 1), issue(&imported, 2));

    store.shortlist_remove(&jace(), &[one]).unwrap();
    let kept = import(&mut store, &on).unwrap();
    assert_eq!(order(&store, "docs"), [two], "tm's removal is kept");
    let calm = |record: &tm_store::RecordOutcome| record.conflicts.is_empty();
    assert!(kept.records.iter().all(calm));

    let unlisted = import(&mut store, &[record(3, None)]).unwrap();
    let three = issue(&unlisted, 3);
    let docs = store.shortlist_named(PROJECT, "docs").unwrap().id;
    let added = store.shortlist_add(&jace(), docs, &[three], Place::End);
    added.unwrap();
    let silent = import(&mut store, &[record(3, None)]).unwrap();
    let conflicts = &silent.records[0].conflicts;
    assert!(conflicts.is_empty(), "a record on no list says nothing");
    store.shortlist_remove(&jace(), &[three]).unwrap();

    let off = [record(1, Some(("docs", 1))), record(2, None)];
    let taken = import(&mut store, &off).unwrap();
    assert_eq!(order(&store, "docs"), Vec::<Uuid>::new(), "taken off");
    assert_eq!(taken.records[1].removed, ["shortlist"]);
    assert_eq!(taken.shortlists[0].taken_off, [two]);

    let other = create(&mut store, "other");
    let added = store.shortlist_add(&jace(), other, &[one], Place::End);
    added.unwrap();
    let moved = [record(1, Some(("later", 1))), record(2, Some(("docs", 1)))];
    let conflicted = import(&mut store, &moved).unwrap();
    assert_eq!(order(&store, "other"), [one], "tm's place is kept");
    assert_eq!(conflicted.records[0].conflicts[0].field, "shortlist");
    assert_eq!(order(&store, "later"), Vec::<Uuid>::new());
    assert_eq!(order(&store, "docs"), [two], "the source put it back");
}

/// An import places what an add refuses, and says what `tm next` passes
/// over and which lists it fills past their size.
#[test]
fn an_import_reports_what_an_add_would_refuse() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let mut epic = record(2, Some(("docs", 2)));
    epic["type"] = json!("epic");
    let docs = json!([{"source": "x:docs", "name": "docs", "actors": ["jace"], "size": 1}]);
    let lines = [project(&docs), record(1, Some(("docs", 1))), epic];
    let imported = import(&mut store, &lines).unwrap();
    let list = &imported.shortlists[0];
    assert_eq!((list.members, list.size), (2, 1));
    let epic = issue(&imported, 2);
    assert_eq!(list.passed_over, [(epic, vec![PassedOver::Epic])]);
}

/// Each refusal names the line, and the message says why.
#[test]
fn a_list_the_import_cannot_settle_is_refused_by_line() {
    let refusal = |lines: &[Value], setup: &dyn Fn(&mut Store)| {
        let dir = TempDir::new();
        let mut store = store(&dir);
        setup(&mut store);
        match import(&mut store, lines).unwrap_err() {
            err @ (StoreError::InvalidExport { line, .. }
            | StoreError::RecordRefused { line, .. }) => (line, err.to_string()),
            other => panic!("{other}"),
        }
    };
    let none = |_: &mut Store| {};
    let mut zero = record(1, Some(("docs", 1)));
    zero["shortlist"]["position"] = json!(0);
    let (line, message) = refusal(&[zero], &none);
    assert!(line == 1 && message.contains("starts at 1"), "{message}");

    let mut unnamed = record(1, Some(("docs", 1)));
    unnamed["shortlist"].as_object_mut().unwrap().remove("name");
    let (line, message) = refusal(&[record(2, None), unnamed], &none);
    assert!(line == 2 && message.contains("no name"), "{message}");

    let mut elsewhere = record(2, Some(("docs", 2)));
    elsewhere["project"] = json!("OPS");
    let (line, message) = refusal(&[record(1, Some(("docs", 1))), elsewhere], &none);
    assert!(line == 2 && message.contains("not OPS"), "{message}");

    let dir = TempDir::new();
    let mut store = store(&dir);
    let actors = ["jace".to_owned()];
    let made = store.create_shortlist(&jace(), OTHER, "ops", &actors, None);
    let mut by_id = record(1, Some(("docs", 1)));
    by_id["shortlist"]["source"] = json!(format!("tm:{}", made.unwrap().0));
    let err = import(&mut store, &[by_id]).unwrap_err();
    let StoreError::RecordRefused { written, .. } = &err else {
        panic!("{err}");
    };
    let message = err.to_string();
    assert!(message.contains("another Project"), "{message}");
    assert_eq!(written, &Vec::<Uuid>::new(), "refused before any record");

    // As a merge can leave them.
    let shared = |store: &mut Store| {
        create(store, "docs");
        create(store, "spare");
        let rename = "UPDATE shortlists SET name = 'docs'";
        store.connection().execute(rename, []).unwrap();
    };
    let (line, message) = refusal(&[record(1, Some(("docs", 1)))], &shared);
    assert!(line == 1 && message.contains("rename one"), "{message}");
}

/// A tm export imported as records into another Workspace brings each list
/// with its Actors, size and order.
#[test]
fn an_export_brings_its_lists_to_another_workspace() {
    let dir = TempDir::new();
    let mut source = store(&dir);
    let [a, b, c] = [(); 3].map(|()| new_issue(&mut source));
    let docs = create(&mut source, "docs");
    let added = source.shortlist_add(&jace(), docs, &[c, a, b], Place::End);
    added.unwrap();
    let mut file = Vec::new();
    source.export(&mut file, 1).unwrap();

    let other = TempDir::new();
    let mut store = store(&other);
    let imported = store.import_records(&mut file.as_slice(), &jace()).unwrap();
    let here = |id: Uuid| issue_from(&imported, &format!("tm:{id}"));
    assert_eq!(order(&store, "docs"), [here(c), here(a), here(b)]);
    let list = store.shortlist_named(PROJECT, "docs").unwrap();
    assert_eq!((list.actors, list.size), (vec!["jace".to_owned()], 10));
    let again = store.import_records(&mut file.as_slice(), &jace()).unwrap();
    assert!(again.shortlists.iter().all(|list| list.batches.is_empty()));
}

/// ADR 0013: an older export of this Workspace overwrites nothing, a place
/// tm took away included.
#[test]
fn an_older_export_puts_back_no_place_tm_took_away() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let [a, b] = [(); 2].map(|()| new_issue(&mut store));
    let docs = create(&mut store, "docs");
    let added = store.shortlist_add(&jace(), docs, &[a, b], Place::End);
    added.unwrap();
    let mut file = Vec::new();
    store.export(&mut file, 1).unwrap();
    store.shortlist_remove(&jace(), &[a]).unwrap();

    let imported = store.import_records(&mut file.as_slice(), &jace()).unwrap();
    assert_eq!(order(&store, "docs"), [b]);
    let record = issue_from(&imported, &format!("tm:{a}"));
    let outcome = imported.records.iter().find(|r| r.issue == record).unwrap();
    assert_eq!(outcome.conflicts[0].field, "shortlist");
}

/// Each batch an import writes is named in what it returns, so it can be
/// undone, and a list an Issue left says so.
#[test]
fn every_place_batch_is_named_and_every_list_left_says_so() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let on = [
        record(1, Some(("zeta", 1))),
        record(2, Some(("zeta", 2))),
        record(3, Some(("zeta", 3))),
    ];
    let imported = import(&mut store, &on).unwrap();
    let [one, two, three] = [1, 2, 3].map(|n| issue(&imported, n));

    let zeta = json!([{"source": "x:zeta", "name": "zeta", "actors": ["jace"], "size": 10}]);
    let moved = [
        project(&zeta),
        record(1, Some(("alpha", 1))),
        record(2, None),
        record(3, Some(("alpha", 2))),
    ];
    let again = import(&mut store, &moved).unwrap();
    let list = |name: &str| {
        let named = |l: &&tm_store::ListOutcome| l.name.as_deref() == Some(name);
        again.shortlists.iter().find(named).unwrap().clone()
    };
    assert_eq!(list("alpha").placed, [one, three]);
    assert_eq!(list("zeta").taken_off, [one, three, two]);
    let left = &again.records[1];
    assert_eq!(left.removed, ["shortlist"]);
    assert_eq!(left.batches.len(), 1);
    let moving = again.records[0].batches[0];
    assert_eq!(list("alpha").batches[1], moving);
    assert_eq!(list("zeta").batches, [moving, left.batches[0]]);
}

#[test]
fn a_finished_issue_is_not_placed_and_says_so() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let mut done = record(1, Some(("docs", 1)));
    done["status"] = json!("done");
    let imported = import(&mut store, &[done]).unwrap();
    assert_eq!(order(&store, "docs"), Vec::<Uuid>::new());
    let conflict = &imported.records[0].conflicts[0];
    assert_eq!(conflict.field, "shortlist");
    assert_eq!(conflict.tm, json!("finished"));

    // Finished in tm, while the source still holds the place it last sent.
    let lines = [record(2, Some(("docs", 1)))];
    let placed = import(&mut store, &lines).unwrap();
    let two = issue(&placed, 2);
    let finish = draft(two, set_field("status", json!("done")));
    store.append(finish).unwrap();
    let again = import(&mut store, &lines).unwrap();
    let shortlist = |c: &&tm_store::Conflict| c.field == "shortlist";
    let conflict = again.records[0].conflicts.iter().find(shortlist).unwrap();
    assert_eq!(conflict.tm, json!("finished"));
}

/// A placed or moved record names the batch that placed it, whether or not
/// the file names the list it left.
#[test]
fn a_placed_record_names_its_batch() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    import(&mut store, &[record(1, Some(("a", 1)))]).unwrap();
    let moved = import(&mut store, &[record(1, Some(("b", 1)))]).unwrap();
    let record = &moved.records[0];
    assert_eq!(record.written, ["shortlist"]);
    let list = &moved.shortlists[0].batches;
    assert_eq!(record.batches, list[1..], "after the list's create");
    assert_eq!(order(&store, "a"), Vec::<Uuid>::new());
}

/// An Issue the source made, placed by tm on the list the source then names
/// too, has that place recorded, so a later move by the source is not a
/// conflict.
#[test]
fn a_place_the_source_comes_to_share_is_recorded() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let imported = import(&mut store, &[record(1, None)]).unwrap();
    let one = issue(&imported, 1);
    let docs = create(&mut store, "docs");
    let added = store.shortlist_add(&jace(), docs, &[one], Place::End);
    added.unwrap();
    import(&mut store, &[record(1, Some(("docs", 1)))]).unwrap();
    let moved = import(&mut store, &[record(1, Some(("later", 1)))]).unwrap();
    assert_eq!(moved.records[0].conflicts, Vec::new());
    assert_eq!(order(&store, "later"), [one]);
}

/// A name matched in another case takes the source's case, so it is no
/// conflict, and a later change of case at the source is written.
#[test]
fn a_name_matched_in_another_case_is_no_conflict() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let docs = create(&mut store, "Docs");
    let lines = [record(1, Some(("docs", 1)))];
    for written in [vec!["name"], vec![]] {
        let imported = import(&mut store, &lines).unwrap();
        let list = &imported.shortlists[0];
        assert_eq!(list.conflicts, Vec::new());
        assert_eq!(list.written, written);
    }
    let mut shouted = record(1, Some(("docs", 1)));
    shouted["shortlist"]["name"] = json!("DOCS");
    let renamed = import(&mut store, &[shouted]).unwrap();
    assert_eq!(renamed.shortlists[0].written, ["name"]);
    assert_eq!(store.shortlist_named(PROJECT, "docs").unwrap().id, docs);
    let name = store.shortlist_named(PROJECT, "docs").unwrap().name;
    assert_eq!(name.as_deref(), Some("DOCS"));
}

/// An Issue already on the list that `tm next` passes over is reported, not
/// only one the import placed.
#[test]
fn every_member_tm_next_passes_over_is_reported() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let lines = [record(1, Some(("docs", 1)))];
    let imported = import(&mut store, &lines).unwrap();
    let one = issue(&imported, 1);
    let mut epic = record(1, Some(("docs", 1)));
    epic["type"] = json!("epic");
    let again = import(&mut store, &[epic]).unwrap();
    let list = &again.shortlists[0];
    assert_eq!(list.placed, Vec::<Uuid>::new());
    assert_eq!(list.passed_over, [(one, vec![PassedOver::Epic])]);
}

/// A list tm deleted is not made again by a file that names it: each place
/// on it is a conflict and the rest of the file imports. A live list of its
/// name is matched instead.
#[test]
fn a_deleted_list_is_kept_deleted() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    import(&mut store, &[record(1, Some(("docs", 1)))]).unwrap();
    let docs = store.shortlist_named(PROJECT, "docs").unwrap().id;
    store.delete_shortlist(&jace(), docs).unwrap();
    let resized = json!([{"source": "x:docs", "name": "docs", "actors": ["jace"], "size": 3}]);
    let lines = [
        project(&resized),
        record(1, Some(("docs", 1))),
        record(2, None),
    ];
    let kept = import(&mut store, &lines).unwrap();
    let list = &kept.shortlists[0];
    assert_eq!((list.list, list.deleted), (docs, true));
    assert_eq!(list.batches, Vec::<Uuid>::new());
    assert_eq!(kept.records[0].conflicts[0].tm, json!("deleted"));
    assert!(store.shortlist_named(PROJECT, "docs").is_err(), "not made");
    assert!(kept.records[1].created);

    // The delete took the Issue off in tm, which a place on the new list
    // contends with.
    let fresh = create(&mut store, "docs");
    let matched = import(&mut store, &lines).unwrap();
    assert_eq!(matched.shortlists[0].list, fresh);
    assert_eq!(matched.records[0].conflicts[0].tm, Value::Null);
    assert_eq!(order(&store, "docs"), Vec::<Uuid>::new());

    let gone = create(&mut store, "gone");
    let mut by_id = record(3, Some(("gone", 1)));
    by_id["shortlist"]["source"] = json!(format!("tm:{gone}"));
    store.delete_shortlist(&jace(), gone).unwrap();
    let skipped = import(&mut store, &[by_id]).unwrap();
    assert!(skipped.shortlists[0].deleted);
    assert_eq!(skipped.records[0].conflicts[0].tm, json!("deleted"));
}

/// A logged place that is neither a list id nor none is corrupt, not "on no
/// list".
#[test]
fn a_place_logged_as_no_list_id_is_corrupt() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    import(&mut store, &[record(1, Some(("docs", 1)))]).unwrap();
    let odd = "DROP TRIGGER ops_immutable;
               UPDATE ops SET payload = json_set(payload, '$.value', 7)
               WHERE field = 'shortlist' AND source = 'x:1';";
    store.connection().execute_batch(odd).unwrap();
    let err = import(&mut store, &[record(1, None)]).unwrap_err();
    assert!(err.to_string().contains("an imported place"), "{err}");
}

/// A membership the read rule hides, as undoing a removal from a deleted
/// list leaves, is cleared whole when the source drops its place.
#[test]
fn a_hidden_place_the_source_drops_is_cleared_whole() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let imported = import(&mut store, &[record(1, Some(("docs", 1)))]).unwrap();
    let one = issue(&imported, 1);
    let removed = store.shortlist_remove(&jace(), &[one]).unwrap();
    let docs = store.shortlist_named(PROJECT, "docs").unwrap().id;
    store.delete_shortlist(&jace(), docs).unwrap();
    store.undo(&jace(), removed.ops[0].op_id).unwrap();
    let rank = |store: &Store| -> Option<String> {
        let query = "SELECT shortlist_rank FROM issues WHERE id = ?1";
        let held = store
            .connection()
            .query_row(query, [one.as_bytes()], |row| row.get(0));
        held.unwrap()
    };
    assert!(rank(&store).is_some(), "hidden, ranked");

    import(&mut store, &[record(1, None)]).unwrap();
    assert_eq!(rank(&store), None);
}

#[test]
fn a_tm_source_that_names_no_id_is_refused() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let mut odd = record(1, Some(("docs", 1)));
    odd["shortlist"]["source"] = json!("tm:not-an-id");
    let err = import(&mut store, &[odd]).unwrap_err();
    let refused = matches!(err, StoreError::InvalidExport { line: 1, .. });
    assert!(refused, "{err}");
}

/// A record whose Issue lives in another Project than its list is refused
/// at its own line.
#[test]
fn a_place_on_another_projects_list_is_refused_at_its_record() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let mut elsewhere = record(2, None);
    elsewhere["project"] = json!("OPS");
    import(&mut store, &[elsewhere]).unwrap();
    let lines = [record(1, Some(("docs", 1))), record(2, Some(("docs", 2)))];
    let err = import(&mut store, &lines).unwrap_err();
    let StoreError::RecordRefused { line, record, .. } = &err else {
        panic!("{err}");
    };
    assert_eq!((*line, record.as_str()), (2, "x:2"), "{err}");
}

#[test]
fn lines_that_disagree_about_a_list_are_refused_before_anything_is_written() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let renamed = json!([{"source": "x:docs", "name": "other", "actors": ["jace"], "size": 3}]);
    let mut line = project(&renamed);
    line["key"] = json!("NEW");
    let mut placed = record(1, Some(("docs", 1)));
    placed["project"] = json!("NEW");
    let err = import(&mut store, &[line, placed]).unwrap_err();
    let refused = matches!(err, StoreError::InvalidExport { line: 2, .. });
    assert!(refused, "{err}");
    let made = store.project_by_key("NEW").unwrap();
    assert!(made.is_none(), "nothing written");
}

/// ADR 0013's base is what the source last held: a change the source makes
/// to match tm is still recorded, so a later tm edit is kept.
#[test]
fn a_source_that_comes_to_match_tm_is_recorded() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let imported = import(&mut store, &[record(1, Some(("docs", 1)))]).unwrap();
    let one = issue(&imported, 1);
    store.shortlist_remove(&jace(), &[one]).unwrap();
    let matched = import(&mut store, &[record(1, None)]).unwrap();
    assert_eq!(matched.records[0].written, ["shortlist"]);
    let docs = store.shortlist_named(PROJECT, "docs").unwrap().id;
    let added = store.shortlist_add(&jace(), docs, &[one], Place::End);
    added.unwrap();
    import(&mut store, &[record(1, None)]).unwrap();
    assert_eq!(order(&store, "docs"), [one], "tm's place is kept");

    let lists = |actors: &[&str]| {
        project(&json!([{"source": "x:docs", "name": "docs", "actors": actors, "size": 10}]))
    };
    let actors = |store: &Store| store.shortlist_named(PROJECT, "docs").unwrap().actors;
    import(&mut store, &[lists(&["alice", "bob"])]).unwrap();
    let alice = ["alice".to_owned()];
    let edit = ShortlistEdit {
        remove: &alice,
        ..Default::default()
    };
    store.edit_shortlist(&jace(), docs, &edit).unwrap();
    import(&mut store, &[lists(&["bob"])]).unwrap();
    let edit = ShortlistEdit {
        add: &alice,
        ..Default::default()
    };
    store.edit_shortlist(&jace(), docs, &edit).unwrap();
    import(&mut store, &[lists(&["bob"])]).unwrap();
    assert_eq!(actors(&store), ["alice", "bob"], "tm's Actor is kept");
}

/// On a list the source made, an Actor it never named was absent there, and
/// a new set of Actors replaces the old without passing through none.
#[test]
fn a_source_made_lists_actors_follow_the_source() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let lists = |actors: &[&str]| {
        project(&json!([{"source": "x:docs", "name": "docs", "actors": actors, "size": 10}]))
    };
    let actors = |store: &Store| store.shortlist_named(PROJECT, "docs").unwrap().actors;
    import(&mut store, &[lists(&["alice"])]).unwrap();
    let grown = import(&mut store, &[lists(&["alice", "bob"])]).unwrap();
    assert_eq!(grown.shortlists[0].conflicts, Vec::new());
    assert_eq!(actors(&store), ["alice", "bob"]);
    import(&mut store, &[lists(&["alice"])]).unwrap();
    import(&mut store, &[lists(&["bob"])]).unwrap();
    assert_eq!(actors(&store), ["bob"]);
}

/// A finish that takes an Issue off a list the record no longer names says
/// so on the record.
#[test]
fn a_finish_that_drops_an_unnamed_place_says_so() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    import(&mut store, &[record(1, Some(("docs", 1)))]).unwrap();
    let mut done = record(1, None);
    done["status"] = json!("done");
    let finished = import(&mut store, &[done]).unwrap();
    assert_eq!(finished.records[0].removed, ["shortlist"]);
    assert_eq!(order(&store, "docs"), Vec::<Uuid>::new());
}

/// Only a write that clears the place says the Issue left its list.
#[test]
fn an_edit_to_a_placed_issue_takes_nothing_off() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    import(&mut store, &[record(1, Some(("docs", 1)))]).unwrap();
    let mut retitled = record(1, Some(("docs", 1)));
    retitled["title"] = json!("renamed");
    let edited = import(&mut store, &[retitled]).unwrap();
    assert_eq!(edited.records[0].removed, Vec::<String>::new());
    assert_eq!(edited.shortlists[0].taken_off, Vec::<Uuid>::new());
}

fn issue_from(imported: &RecordsImported, source: &str) -> Uuid {
    let record = imported.records.iter().find(|r| r.source == source);
    record.unwrap().issue
}

/// ADR 0016: what finishes an Issue takes it off its list in the same
/// batch, and an imported finish clears from the record's source.
#[test]
fn an_imported_finish_clears_the_place_from_its_source() {
    let dir = TempDir::new();
    let mut store = store(&dir);
    let imported = import(&mut store, &[record(1, Some(("docs", 1)))]).unwrap();
    let one = issue(&imported, 1);
    let mut done = record(1, Some(("docs", 1)));
    done["status"] = json!("done");
    let finished = import(&mut store, &[done]).unwrap();
    assert_eq!(field(&store, one, "shortlist"), None);
    let (record, list) = (&finished.records[0], &finished.shortlists[0]);
    assert_eq!(record.removed, ["shortlist"]);
    assert_eq!(record.conflicts[0].tm, json!("finished"));
    assert_eq!(list.taken_off, [one]);
    assert_eq!(list.batches, record.batches);
    let sources: Vec<Option<String>> = store
        .connection()
        .prepare(
            "SELECT source FROM ops WHERE issue_id = ?1
               AND field IN ('shortlist', 'shortlist_rank')
             ORDER BY seq DESC LIMIT 2",
        )
        .unwrap()
        .query_map([one.as_bytes()], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(sources, [Some("x:1".to_owned()), Some("x:1".to_owned())]);
}
