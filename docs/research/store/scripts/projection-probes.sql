-- Run against a scratch file: sqlite3 /path/to/scratch.db < projection-probes.sql
.echo on
.headers off
SELECT sqlite_version();
SELECT '--- (a) expression index on json_extract';
CREATE TABLE issues(id INTEGER PRIMARY KEY, fields TEXT);
CREATE INDEX issues_area ON issues(json_extract(fields,'$.area'));
CREATE INDEX issues_due ON issues(json_extract(fields,'$.due'));
EXPLAIN QUERY PLAN SELECT id FROM issues WHERE json_extract(fields,'$.area')='cli';
EXPLAIN QUERY PLAN SELECT id FROM issues WHERE fields->>'$.area'='cli';
EXPLAIN QUERY PLAN SELECT id FROM issues WHERE fields->>'area'='cli';
EXPLAIN QUERY PLAN SELECT id FROM issues ORDER BY json_extract(fields,'$.due');
EXPLAIN QUERY PLAN SELECT id FROM issues ORDER BY fields->>'$.due';
CREATE INDEX issues_area_arrow ON issues(fields->>'$.area');
EXPLAIN QUERY PLAN SELECT id FROM issues WHERE fields->>'$.area'='cli';
EXPLAIN QUERY PLAN SELECT id FROM issues WHERE fields->>'area'='cli';
DROP INDEX issues_area_arrow;

SELECT '--- (b) generated VIRTUAL column';
CREATE TABLE issues2(id INTEGER PRIMARY KEY, fields TEXT,
  area TEXT GENERATED ALWAYS AS (json_extract(fields,'$.area')) VIRTUAL);
CREATE INDEX issues2_area ON issues2(area);
EXPLAIN QUERY PLAN SELECT id FROM issues2 WHERE area='cli';
EXPLAIN QUERY PLAN SELECT id FROM issues2 WHERE json_extract(fields,'$.area')='cli';

SELECT '--- (c) EAV with EXISTS';
CREATE TABLE field_values(issue_id INTEGER, field TEXT, num REAL, txt TEXT);
CREATE INDEX fv_idx ON field_values(field, txt, issue_id);
EXPLAIN QUERY PLAN SELECT i.id FROM issues i
 WHERE EXISTS (SELECT 1 FROM field_values f WHERE f.issue_id=i.id AND f.field='area' AND f.txt='cli')
   AND EXISTS (SELECT 1 FROM field_values f WHERE f.issue_id=i.id AND f.field='priority' AND f.txt='p1');
-- with stats, planner may pick differently
INSERT INTO issues(id, fields) SELECT value, json_object('area', CASE value%5 WHEN 0 THEN 'cli' ELSE 'tui' END) FROM generate_series(1,2000);
INSERT INTO field_values SELECT value, 'area', NULL, CASE value%5 WHEN 0 THEN 'cli' ELSE 'tui' END FROM generate_series(1,2000);
INSERT INTO field_values SELECT value, 'priority', NULL, 'p'||(value%4) FROM generate_series(1,2000);
ANALYZE;
EXPLAIN QUERY PLAN SELECT i.id FROM issues i
 WHERE EXISTS (SELECT 1 FROM field_values f WHERE f.issue_id=i.id AND f.field='area' AND f.txt='cli')
   AND EXISTS (SELECT 1 FROM field_values f WHERE f.issue_id=i.id AND f.field='priority' AND f.txt='p1');
EXPLAIN QUERY PLAN SELECT a.issue_id FROM field_values a JOIN field_values b
  ON b.issue_id=a.issue_id AND b.field='priority' AND b.txt='p1'
 WHERE a.field='area' AND a.txt='cli';
SELECT count(*) FROM issues i
 WHERE EXISTS (SELECT 1 FROM field_values f WHERE f.issue_id=i.id AND f.field='area' AND f.txt='cli')
   AND EXISTS (SELECT 1 FROM field_values f WHERE f.issue_id=i.id AND f.field='priority' AND f.txt='p1');

SELECT '--- (d) FTS5 external content';
CREATE TABLE docs(id INTEGER PRIMARY KEY, title TEXT, description TEXT);
CREATE VIRTUAL TABLE docs_fts USING fts5(title, description, content='docs', content_rowid='id');
CREATE TRIGGER docs_ai AFTER INSERT ON docs BEGIN
  INSERT INTO docs_fts(rowid, title, description) VALUES (new.id, new.title, new.description);
END;
CREATE TRIGGER docs_ad AFTER DELETE ON docs BEGIN
  INSERT INTO docs_fts(docs_fts, rowid, title, description) VALUES('delete', old.id, old.title, old.description);
END;
CREATE TRIGGER docs_au AFTER UPDATE ON docs BEGIN
  INSERT INTO docs_fts(docs_fts, rowid, title, description) VALUES('delete', old.id, old.title, old.description);
  INSERT INTO docs_fts(rowid, title, description) VALUES (new.id, new.title, new.description);
END;
INSERT INTO docs(title, description) VALUES ('Fix login crash','segfault on empty password'),('Add export','export issues to csv');
SELECT 'search crash:', rowid, title FROM docs_fts WHERE docs_fts MATCH 'crash';
DROP TRIGGER docs_au;
UPDATE docs SET title='Fix login hang' WHERE id=1;
SELECT 'stale search crash:', rowid, title FROM docs_fts WHERE docs_fts MATCH 'crash';
SELECT 'stale search hang:', count(*) FROM docs_fts WHERE docs_fts MATCH 'hang';
INSERT INTO docs_fts(docs_fts) VALUES('integrity-check');
SELECT 'stale: plain integrity-check done (no error line above = passed)';
INSERT INTO docs_fts(docs_fts, rank) VALUES('integrity-check', 1);
SELECT 'stale: integrity-check rank=1 done';
INSERT INTO docs_fts(docs_fts) VALUES('rebuild');
SELECT 'after rebuild crash:', count(*) FROM docs_fts WHERE docs_fts MATCH 'crash';
SELECT 'after rebuild hang:', rowid, title FROM docs_fts WHERE docs_fts MATCH 'hang';
INSERT INTO docs_fts(docs_fts) VALUES('integrity-check');
INSERT INTO docs_fts(docs_fts, rank) VALUES('integrity-check', 1);
SELECT 'integrity-check after rebuild: ok';
CREATE VIRTUAL TABLE tri USING fts5(title, tokenize='trigram');
INSERT INTO tri SELECT title FROM docs;
SELECT 'trigram substring ogi:', title FROM tri WHERE tri MATCH 'ogi';
SELECT 'trigram LIKE:', title FROM tri WHERE title LIKE '%xpor%';
EXPLAIN QUERY PLAN SELECT title FROM tri WHERE title LIKE '%xpor%';

SELECT '--- (e) rebuild timing';
CREATE TABLE ops(seq INTEGER PRIMARY KEY, issue_id INTEGER, kind TEXT, payload TEXT);
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<100000)
INSERT INTO ops(issue_id, kind, payload)
SELECT ((i-1)/10)+1,
  CASE WHEN (i-1)%10=0 THEN 'create' ELSE 'set' END,
  CASE WHEN (i-1)%10=0 THEN json_object('title','Issue '||(((i-1)/10)+1),'status','open','area','cli')
       ELSE json_object('field', CASE i%3 WHEN 0 THEN 'status' WHEN 1 THEN 'area' ELSE 'title' END,
                        'value', 'v'||i) END
FROM n;
SELECT count(*), count(DISTINCT issue_id) FROM ops;
CREATE TABLE proj(id INTEGER PRIMARY KEY, title TEXT, status TEXT, area TEXT, last_seq INTEGER);
.timer on
BEGIN;
DELETE FROM proj;
INSERT INTO proj(id, title, status, area, last_seq)
SELECT issue_id, json_extract(payload,'$.title'), json_extract(payload,'$.status'), json_extract(payload,'$.area'), seq
FROM ops WHERE kind='create';
-- apply set ops: last write per (issue, field) wins in seq order
UPDATE proj SET
  title  = coalesce((SELECT json_extract(payload,'$.value') FROM ops o WHERE o.issue_id=proj.id AND o.kind='set' AND json_extract(o.payload,'$.field')='title'  ORDER BY seq DESC LIMIT 1), title),
  status = coalesce((SELECT json_extract(payload,'$.value') FROM ops o WHERE o.issue_id=proj.id AND o.kind='set' AND json_extract(o.payload,'$.field')='status' ORDER BY seq DESC LIMIT 1), status),
  area   = coalesce((SELECT json_extract(payload,'$.value') FROM ops o WHERE o.issue_id=proj.id AND o.kind='set' AND json_extract(o.payload,'$.field')='area'   ORDER BY seq DESC LIMIT 1), area),
  last_seq = (SELECT max(seq) FROM ops o WHERE o.issue_id=proj.id);
COMMIT;
.timer off
SELECT 'without ops(issue_id) index above; now with index:';
CREATE INDEX ops_issue ON ops(issue_id, seq);
.timer on
BEGIN;
DELETE FROM proj;
INSERT INTO proj(id, title, status, area, last_seq)
SELECT issue_id, json_extract(payload,'$.title'), json_extract(payload,'$.status'), json_extract(payload,'$.area'), seq
FROM ops WHERE kind='create';
UPDATE proj SET
  title  = coalesce((SELECT json_extract(payload,'$.value') FROM ops o WHERE o.issue_id=proj.id AND o.kind='set' AND json_extract(o.payload,'$.field')='title'  ORDER BY seq DESC LIMIT 1), title),
  status = coalesce((SELECT json_extract(payload,'$.value') FROM ops o WHERE o.issue_id=proj.id AND o.kind='set' AND json_extract(o.payload,'$.field')='status' ORDER BY seq DESC LIMIT 1), status),
  area   = coalesce((SELECT json_extract(payload,'$.value') FROM ops o WHERE o.issue_id=proj.id AND o.kind='set' AND json_extract(o.payload,'$.field')='area'   ORDER BY seq DESC LIMIT 1), area),
  last_seq = (SELECT max(seq) FROM ops o WHERE o.issue_id=proj.id);
COMMIT;
.timer off
SELECT count(*), sum(status LIKE 'v%'), sum(area LIKE 'v%') FROM proj;
SELECT * FROM proj WHERE id=1;
SELECT seq, kind, payload FROM ops WHERE issue_id=1;
