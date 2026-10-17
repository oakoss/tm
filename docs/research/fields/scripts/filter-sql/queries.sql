-- Each block: `-- name:` label, `-- filter:` the tm filter it stands for,
-- `-- params:` values bound to ?1, ?2, ... (separated by `|`), then one query.
-- Every query keeps `archived = 0`, the default a View would add.

-- name: a_exists
-- filter: area:cli label:bug -type:spike status:review,testing
-- params: review|testing|spike|area|cli|labels|bug
SELECT i.id FROM issues i
WHERE i.archived = 0 AND i.status IN (?1, ?2) AND i.type <> ?3
  AND EXISTS (SELECT 1 FROM field_values f WHERE f.issue_id = i.id AND f.field = ?4 AND f.txt = ?5)
  AND EXISTS (SELECT 1 FROM set_members s WHERE s.issue_id = i.id AND s.field = ?6 AND s.elem = ?7);

-- name: a_join
-- filter: area:cli label:bug -type:spike status:review,testing
-- params: review|testing|spike|area|cli|labels|bug
SELECT i.id FROM issues i
JOIN field_values f ON f.issue_id = i.id AND f.field = ?4 AND f.txt = ?5
JOIN set_members s ON s.issue_id = i.id AND s.field = ?6 AND s.elem = ?7
WHERE i.archived = 0 AND i.status IN (?1, ?2) AND i.type <> ?3;

-- name: a_in
-- filter: area:cli label:bug -type:spike status:review,testing
-- params: review|testing|spike|area|cli|labels|bug
SELECT i.id FROM issues i
WHERE i.archived = 0 AND i.status IN (?1, ?2) AND i.type <> ?3
  AND i.id IN (SELECT issue_id FROM field_values WHERE field = ?4 AND txt = ?5)
  AND i.id IN (SELECT issue_id FROM set_members WHERE field = ?6 AND elem = ?7);

-- name: a2_exists
-- filter: area:cli label:bug
-- params: area|cli|labels|bug
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND EXISTS (SELECT 1 FROM field_values f WHERE f.issue_id = i.id AND f.field = ?1 AND f.txt = ?2)
  AND EXISTS (SELECT 1 FROM set_members s WHERE s.issue_id = i.id AND s.field = ?3 AND s.elem = ?4);

-- name: a2_join
-- filter: area:cli label:bug
-- params: area|cli|labels|bug
SELECT i.id FROM issues i
JOIN field_values f ON f.issue_id = i.id AND f.field = ?1 AND f.txt = ?2
JOIN set_members s ON s.issue_id = i.id AND s.field = ?3 AND s.elem = ?4
WHERE i.archived = 0;

-- name: a2_in
-- filter: area:cli label:bug
-- params: area|cli|labels|bug
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND i.id IN (SELECT issue_id FROM field_values WHERE field = ?1 AND txt = ?2)
  AND i.id IN (SELECT issue_id FROM set_members WHERE field = ?3 AND elem = ?4);

-- name: b_any_exists
-- filter: label:bug,ui
-- params: labels|bug|ui
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND EXISTS (SELECT 1 FROM set_members s WHERE s.issue_id = i.id AND s.field = ?1 AND s.elem IN (?2, ?3));

-- name: b_any_join_distinct
-- filter: label:bug,ui
-- params: labels|bug|ui
SELECT DISTINCT i.id FROM issues i
JOIN set_members s ON s.issue_id = i.id AND s.field = ?1 AND s.elem IN (?2, ?3)
WHERE i.archived = 0;

-- name: b_any_join_no_distinct
-- filter: label:bug,ui (wrong: an Issue with both labels appears twice)
-- params: labels|bug|ui
SELECT i.id FROM issues i
JOIN set_members s ON s.issue_id = i.id AND s.field = ?1 AND s.elem IN (?2, ?3)
WHERE i.archived = 0;

-- name: b_any_in
-- filter: label:bug,ui
-- params: labels|bug|ui
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND i.id IN (SELECT issue_id FROM set_members WHERE field = ?1 AND elem IN (?2, ?3));

-- name: b_all_exists
-- filter: label:bug label:ui
-- params: labels|bug|ui
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND EXISTS (SELECT 1 FROM set_members s WHERE s.issue_id = i.id AND s.field = ?1 AND s.elem = ?2)
  AND EXISTS (SELECT 1 FROM set_members s WHERE s.issue_id = i.id AND s.field = ?1 AND s.elem = ?3);

-- name: b_all_join
-- filter: label:bug label:ui
-- params: labels|bug|ui
SELECT i.id FROM issues i
JOIN set_members a ON a.issue_id = i.id AND a.field = ?1 AND a.elem = ?2
JOIN set_members b ON b.issue_id = i.id AND b.field = ?1 AND b.elem = ?3
WHERE i.archived = 0;

-- name: b_all_in
-- filter: label:bug label:ui
-- params: labels|bug|ui
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND i.id IN (SELECT issue_id FROM set_members WHERE field = ?1 AND elem = ?2)
  AND i.id IN (SELECT issue_id FROM set_members WHERE field = ?1 AND elem = ?3);

-- name: b_all_group
-- filter: label:bug label:ui
-- params: labels|bug|ui
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND i.id IN (SELECT issue_id FROM set_members WHERE field = ?1 AND elem IN (?2, ?3)
               GROUP BY issue_id HAVING count(*) = 2);

-- name: c_not_exists
-- filter: -label:bug
-- params: labels|bug
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND NOT EXISTS (SELECT 1 FROM set_members s WHERE s.issue_id = i.id AND s.field = ?1 AND s.elem = ?2);

-- name: c_left_join_null
-- filter: -label:bug
-- params: labels|bug
SELECT i.id FROM issues i
LEFT JOIN set_members s ON s.issue_id = i.id AND s.field = ?1 AND s.elem = ?2
WHERE i.archived = 0 AND s.issue_id IS NULL;

-- name: c_not_in
-- filter: -label:bug
-- params: labels|bug
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND i.id NOT IN (SELECT issue_id FROM set_members WHERE field = ?1 AND elem = ?2);

-- name: c_none_of_not_exists
-- filter: -label:bug,ui
-- params: labels|bug|ui
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND NOT EXISTS (SELECT 1 FROM set_members s WHERE s.issue_id = i.id AND s.field = ?1 AND s.elem IN (?2, ?3));

-- name: c_select_not_exists
-- filter: -area:cli (Issues with no area match)
-- params: area|cli
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND NOT EXISTS (SELECT 1 FROM field_values f WHERE f.issue_id = i.id AND f.field = ?1 AND f.txt = ?2);

-- name: c_select_join_ne
-- filter: -area:cli (Jira `!=` reading: Issues with no area do not match)
-- params: area|cli
SELECT i.id FROM issues i
JOIN field_values f ON f.issue_id = i.id AND f.field = ?1 AND f.txt <> ?2
WHERE i.archived = 0;

-- name: c_no_area
-- filter: no:area
-- params: area
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND NOT EXISTS (SELECT 1 FROM field_values f WHERE f.issue_id = i.id AND f.field = ?1);

-- name: d_due_left_join_limit
-- filter: status:todo sort:due (first page)
-- params: todo|due
SELECT i.id FROM issues i
LEFT JOIN field_values f ON f.issue_id = i.id AND f.field = ?2 AND f.elem = ''
WHERE i.archived = 0 AND i.status = ?1
ORDER BY f.num IS NULL, f.num, i.created_ms
LIMIT 50;

-- name: d_due_left_join_all
-- filter: status:todo sort:due (every row)
-- params: todo|due
SELECT i.id FROM issues i
LEFT JOIN field_values f ON f.issue_id = i.id AND f.field = ?2 AND f.elem = ''
WHERE i.archived = 0 AND i.status = ?1
ORDER BY f.num IS NULL, f.num, i.created_ms;

-- name: d_due_scalar_subquery_limit
-- filter: status:todo sort:due (first page, sort key as a correlated subquery)
-- params: todo|due
SELECT i.id FROM issues i
WHERE i.archived = 0 AND i.status = ?1
ORDER BY (SELECT f.num FROM field_values f WHERE f.issue_id = i.id AND f.field = ?2 AND f.elem = '') IS NULL,
         (SELECT f.num FROM field_values f WHERE f.issue_id = i.id AND f.field = ?2 AND f.elem = ''),
         i.created_ms
LIMIT 50;

-- name: d_due_unfiltered_left_join_limit
-- filter: sort:due (first page, no other filter)
-- params: due
SELECT i.id FROM issues i
LEFT JOIN field_values f ON f.issue_id = i.id AND f.field = ?1 AND f.elem = ''
WHERE i.archived = 0
ORDER BY f.num IS NULL, f.num, i.created_ms
LIMIT 50;

-- name: d_due_unfiltered_index_walk_limit
-- filter: sort:due (first page, Issues with a due date only, driven by the index)
-- params: due
SELECT i.id FROM field_values f
JOIN issues i ON i.id = f.issue_id
WHERE f.field = ?1 AND i.archived = 0
ORDER BY f.num
LIMIT 50;

-- name: d_due_unfiltered_union_limit
-- filter: sort:due (first page: Issues with a due date by the index, then the rest)
-- params: due
SELECT id FROM (
  SELECT i.id FROM field_values f JOIN issues i ON i.id = f.issue_id
  WHERE f.field = ?1 AND i.archived = 0
  ORDER BY f.num LIMIT 50)
UNION ALL
SELECT id FROM (
  SELECT i.id FROM issues i
  WHERE i.archived = 0
    AND NOT EXISTS (SELECT 1 FROM field_values f WHERE f.issue_id = i.id AND f.field = ?1)
  ORDER BY i.created_ms LIMIT 50)
LIMIT 50;

-- name: d_area_case_limit
-- filter: status:todo sort:area (option order from the Scheme, inlined as CASE)
-- params: todo|area|core|cli|tui|web|store|sync
SELECT i.id FROM issues i
LEFT JOIN field_values f ON f.issue_id = i.id AND f.field = ?2 AND f.elem = ''
WHERE i.archived = 0 AND i.status = ?1
ORDER BY CASE f.txt WHEN ?3 THEN 0 WHEN ?4 THEN 1 WHEN ?5 THEN 2 WHEN ?6 THEN 3
                    WHEN ?7 THEN 4 WHEN ?8 THEN 5 ELSE 6 END,
         i.created_ms
LIMIT 50;

-- name: d_area_json_each_limit
-- filter: status:todo sort:area (option order bound as one JSON array)
-- params: todo|area|["core","cli","tui","web","store","sync"]
SELECT i.id FROM issues i
LEFT JOIN field_values f ON f.issue_id = i.id AND f.field = ?2 AND f.elem = ''
LEFT JOIN json_each(?3) o ON o.value = f.txt
WHERE i.archived = 0 AND i.status = ?1
ORDER BY o.key IS NULL, o.key, i.created_ms
LIMIT 50;

-- name: d_priority_rank_limit
-- filter: status:todo sort:priority (urgent first, none last)
-- params: todo
SELECT i.id FROM issues i
WHERE i.archived = 0 AND i.status = ?1
ORDER BY i.priority = 0, i.priority, i.created_ms
LIMIT 50;

-- name: e_nocase_compare
-- filter: area:CLI (matched with COLLATE NOCASE in SQL)
-- params: area|CLI
SELECT i.id FROM issues i
JOIN field_values f ON f.issue_id = i.id AND f.field = ?1 AND f.txt = ?2 COLLATE NOCASE
WHERE i.archived = 0;

-- name: e_canonical_compare
-- filter: area:CLI (resolved to the option `cli` before SQL)
-- params: area|cli
SELECT i.id FROM issues i
JOIN field_values f ON f.issue_id = i.id AND f.field = ?1 AND f.txt = ?2
WHERE i.archived = 0;

-- name: f_or_in
-- filter: area:cli OR label:bug (later grammar; checks the IN form composes)
-- params: area|cli|labels|bug
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND (i.id IN (SELECT issue_id FROM field_values WHERE field = ?1 AND txt = ?2)
       OR i.id IN (SELECT issue_id FROM set_members WHERE field = ?3 AND elem = ?4));

-- name: f_or_exists
-- filter: area:cli OR label:bug (later grammar)
-- params: area|cli|labels|bug
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND (EXISTS (SELECT 1 FROM field_values f WHERE f.issue_id = i.id AND f.field = ?1 AND f.txt = ?2)
       OR EXISTS (SELECT 1 FROM set_members s WHERE s.issue_id = i.id AND s.field = ?3 AND s.elem = ?4));

-- name: f_or_union
-- filter: area:cli OR label:bug (later grammar, as a compound of id sets)
-- params: area|cli|labels|bug
SELECT i.id FROM issues i
WHERE i.archived = 0
  AND i.id IN (SELECT issue_id FROM field_values WHERE field = ?1 AND txt = ?2
               UNION SELECT issue_id FROM set_members WHERE field = ?3 AND elem = ?4);
