-- Run against a copy of the runner's filter-100000.db:
--   sqlite3 copy.db < nocase-and-null.sql
-- Kept out of setup.sql so the extra index cannot change the main probes' plans.
.timer off
SELECT sqlite_version();
SELECT NULL <> 'x', NULL IS NOT 'x', 'jace' IS NOT 'x';
WITH t(p) AS (VALUES ('none'), ('urgent'), ('high'), ('medium'), ('low'))
SELECT group_concat(p, ' < ') FROM (SELECT p FROM t ORDER BY p);

EXPLAIN QUERY PLAN SELECT issue_id FROM set_members WHERE field = 'labels' AND elem = 'BUG' COLLATE NOCASE;
CREATE INDEX set_members_nocase ON set_members (field, elem COLLATE NOCASE);
ANALYZE;
EXPLAIN QUERY PLAN SELECT issue_id FROM set_members WHERE field = 'labels' AND elem = 'BUG' COLLATE NOCASE;
EXPLAIN QUERY PLAN SELECT issue_id FROM set_members WHERE field = 'labels' AND elem = 'bug';
SELECT count(*) FROM set_members WHERE field = 'labels' AND elem = 'BUG' COLLATE NOCASE;
SELECT count(*) FROM set_members WHERE field = 'labels' AND elem = 'BUG';
