-- Run against a scratch file: sqlite3 /path/to/scratch.db < scheme-probes.sql
.echo on
.headers off
SELECT sqlite_version();

CREATE TABLE issues(
  id INTEGER PRIMARY KEY,
  type TEXT NOT NULL,
  status TEXT NOT NULL,
  status_category TEXT,
  priority_text TEXT,
  priority_rank INTEGER NOT NULL
);
CREATE TABLE scheme_statuses(
  type TEXT NOT NULL,
  status TEXT NOT NULL,
  category TEXT NOT NULL,
  PRIMARY KEY (type, status)
) WITHOUT ROWID;
INSERT INTO scheme_statuses VALUES
  ('task','triage','triage'), ('task','todo','todo'), ('task','in_progress','active'),
  ('task','in_review','active'), ('task','done','done'), ('task','cancelled','done'),
  ('bug','triage','triage'), ('bug','todo','todo'), ('bug','in_progress','active'),
  ('bug','in_review','active'), ('bug','done','done'), ('bug','cancelled','done');

INSERT INTO issues(id, type, status, priority_text, priority_rank)
SELECT value,
       CASE value % 2 WHEN 0 THEN 'task' ELSE 'bug' END,
       CASE value % 6 WHEN 0 THEN 'triage' WHEN 1 THEN 'todo' WHEN 2 THEN 'in_progress'
                      WHEN 3 THEN 'in_review' WHEN 4 THEN 'done' ELSE 'cancelled' END,
       CASE value % 5 WHEN 0 THEN 'none' WHEN 1 THEN 'urgent' WHEN 2 THEN 'high'
                      WHEN 3 THEN 'medium' ELSE 'low' END,
       CASE value % 5 WHEN 0 THEN 0 WHEN 1 THEN 4 WHEN 2 THEN 3 WHEN 3 THEN 2 ELSE 1 END
FROM generate_series(1, 10000);
UPDATE issues SET status_category =
  (SELECT category FROM scheme_statuses s WHERE s.type = issues.type AND s.status = issues.status);
CREATE INDEX issues_category ON issues(status_category);
CREATE INDEX issues_status ON issues(type, status);
CREATE INDEX issues_priority ON issues(priority_rank);
ANALYZE;

SELECT '--- (a) priority sorted as stored text';
SELECT priority_text, count(*) FROM issues GROUP BY priority_text ORDER BY priority_text;

SELECT '--- (a) priority sorted as rank, most important first';
SELECT priority_rank, priority_text, count(*) FROM issues GROUP BY priority_rank ORDER BY priority_rank DESC;
EXPLAIN QUERY PLAN SELECT id FROM issues ORDER BY priority_rank DESC LIMIT 20;
EXPLAIN QUERY PLAN SELECT id FROM issues WHERE priority_rank >= 3;

SELECT '--- (b) category as a stored column versus a join to the Scheme';
EXPLAIN QUERY PLAN SELECT id FROM issues WHERE status_category = 'active';
EXPLAIN QUERY PLAN SELECT i.id FROM issues i
  JOIN scheme_statuses s ON s.type = i.type AND s.status = i.status
  WHERE s.category = 'active';
EXPLAIN QUERY PLAN SELECT id FROM issues
  WHERE (type, status) IN (VALUES ('task','in_progress'), ('task','in_review'),
                                  ('bug','in_progress'), ('bug','in_review'));
SELECT 'in-list active', count(*) FROM issues
  WHERE (type, status) IN (VALUES ('task','in_progress'), ('task','in_review'),
                                  ('bug','in_progress'), ('bug','in_review'));
SELECT 'stored active', count(*) FROM issues WHERE status_category = 'active';
SELECT 'joined active', count(*) FROM issues i
  JOIN scheme_statuses s ON s.type = i.type AND s.status = i.status WHERE s.category = 'active';
.timer on
SELECT count(*) FROM issues WHERE status_category = 'active';
SELECT count(*) FROM issues i
  JOIN scheme_statuses s ON s.type = i.type AND s.status = i.status WHERE s.category = 'active';
.timer off

SELECT '--- (c) Scheme edit: in_review moves from active to done';
UPDATE scheme_statuses SET category = 'done' WHERE status = 'in_review';
SELECT 'stored active after edit', count(*) FROM issues WHERE status_category = 'active';
SELECT 'joined active after edit', count(*) FROM issues i
  JOIN scheme_statuses s ON s.type = i.type AND s.status = i.status WHERE s.category = 'active';

SELECT '--- (d) Scheme edit: status cancelled removed while Issues still hold it';
DELETE FROM scheme_statuses WHERE status = 'cancelled';
SELECT 'inner join, all categories', count(*) FROM issues i
  JOIN scheme_statuses s ON s.type = i.type AND s.status = i.status;
SELECT 'left join, no category', count(*) FROM issues i
  LEFT JOIN scheme_statuses s ON s.type = i.type AND s.status = i.status WHERE s.category IS NULL;
SELECT 'total issues', count(*) FROM issues;
