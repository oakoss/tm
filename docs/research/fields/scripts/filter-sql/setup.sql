-- Built-in fields as columns on `issues`, Scheme fields in `field_values` (EAV
-- with typed columns), labels in `set_members`. `{N}` is replaced with the Issue
-- count by the runner.
--
-- Data, by pseudo-random hash of the rowid:
--   status    triage 10%, todo 20%, in_progress 15%, review 5%, testing 5%,
--             done 40%, cancelled 5%
--   type      spike 10%, epic 10%, bug 20%, task 40%, story 10%, design 10%
--   priority  0 none .. 4 low (Linear's numbering), uniform
--   assignee  'jace' 60%, else NULL; archived 5%
--   area      single select on 80% of Issues; cli is 1/6 of those
--   due       date (as epoch ms in `num`) on 30% of Issues
--   labels    each of 20 labels on about 10% of Issues (bug, ui, perf, ...)

PRAGMA journal_mode = WAL;

CREATE TABLE issues (
  id BLOB PRIMARY KEY,
  project TEXT NOT NULL,
  type TEXT NOT NULL,
  title TEXT NOT NULL,
  status TEXT NOT NULL,
  status_category TEXT NOT NULL,
  priority INTEGER NOT NULL,
  assignee TEXT,
  archived INTEGER NOT NULL DEFAULT 0,
  created_ms INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX issues_status ON issues (status);

CREATE TABLE field_values (
  issue_id BLOB NOT NULL,
  field TEXT NOT NULL,
  elem TEXT NOT NULL,
  txt TEXT,
  num REAL,
  ref_uuid BLOB,
  PRIMARY KEY (issue_id, field, elem)
) STRICT, WITHOUT ROWID;

CREATE INDEX field_values_txt ON field_values (field, txt, issue_id);
CREATE INDEX field_values_num ON field_values (field, num, issue_id);

CREATE TABLE set_members (
  issue_id BLOB NOT NULL,
  field TEXT NOT NULL,
  elem TEXT NOT NULL,
  PRIMARY KEY (issue_id, field, elem)
) STRICT, WITHOUT ROWID;

CREATE INDEX set_members_by_elem ON set_members (field, elem);

CREATE TEMP TABLE seq (n INTEGER PRIMARY KEY);
WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < {N})
INSERT INTO seq SELECT n FROM c;

-- h(n, salt): high bits of a multiplicative hash, so `% m` is not tied to n % m.
CREATE TEMP VIEW h AS
SELECT n,
  ((n * 2654435761 + 1 * 40503) % 4294967296) / 65536 AS h1,
  ((n * 2654435761 + 2 * 97561) % 4294967296) / 65536 AS h2,
  ((n * 2654435761 + 3 * 15731) % 4294967296) / 65536 AS h3,
  ((n * 2246822519 + 4 * 31337) % 4294967296) / 65536 AS h4,
  ((n * 3266489917 + 5 * 7919) % 4294967296) / 65536 AS h5
FROM seq;

INSERT INTO issues
SELECT
  -- 16-byte deterministic id, the width of the UUID keys tm stores.
  unhex(printf('%08x%08x0000%012x', h1 * 65536 + h2, n * 2654435761 % 4294967296, n)),
  'TM',
  CASE h2 % 10 WHEN 0 THEN 'spike' WHEN 1 THEN 'epic' WHEN 2 THEN 'bug' WHEN 3 THEN 'bug'
               WHEN 8 THEN 'story' WHEN 9 THEN 'design' ELSE 'task' END,
  'Issue ' || n,
  CASE WHEN h1 % 20 < 2 THEN 'triage' WHEN h1 % 20 < 6 THEN 'todo'
       WHEN h1 % 20 < 9 THEN 'in_progress' WHEN h1 % 20 = 9 THEN 'review'
       WHEN h1 % 20 = 10 THEN 'testing' WHEN h1 % 20 < 19 THEN 'done' ELSE 'cancelled' END,
  CASE WHEN h1 % 20 < 2 THEN 'triage' WHEN h1 % 20 < 6 THEN 'todo'
       WHEN h1 % 20 < 11 THEN 'active' ELSE 'done' END,
  h3 % 5,
  CASE WHEN h4 % 10 < 6 THEN 'jace' END,
  CASE WHEN h5 % 20 = 0 THEN 1 ELSE 0 END,
  1760000000000 + n * 60000,
  1760000000000 + n * 60000 + (h5 % 1000) * 60000
FROM h;

INSERT INTO field_values (issue_id, field, elem, txt)
SELECT i.id, 'area', '',
  CASE h.h3 % 6 WHEN 0 THEN 'cli' WHEN 1 THEN 'tui' WHEN 2 THEN 'web'
                WHEN 3 THEN 'store' WHEN 4 THEN 'sync' ELSE 'core' END
FROM h JOIN issues i ON i.rowid = h.n
WHERE h.h4 % 10 < 8;

INSERT INTO field_values (issue_id, field, elem, num)
SELECT i.id, 'due', '', 1760000000000 + (h.h5 % 365) * 86400000
FROM h JOIN issues i ON i.rowid = h.n
WHERE h.h2 % 10 < 3;

CREATE TEMP TABLE label_names (k INTEGER PRIMARY KEY, name TEXT);
INSERT INTO label_names VALUES
  (0,'bug'),(1,'ui'),(2,'perf'),(3,'docs'),(4,'security'),(5,'cli'),(6,'tui'),
  (7,'web'),(8,'sync'),(9,'store'),(10,'agent'),(11,'git'),(12,'release'),
  (13,'import'),(14,'search'),(15,'board'),(16,'claims'),(17,'findings'),
  (18,'checks'),(19,'good first issue');

INSERT INTO set_members (issue_id, field, elem)
SELECT i.id, 'labels', l.name
FROM h JOIN issues i ON i.rowid = h.n CROSS JOIN label_names l
-- A multiplier per label keeps labels independent (bug and ui co-occur on ~1%).
WHERE ((h.n * (2654435761 + l.k * 81006)) % 4294967296) / 65536 % 10 = 0;

ANALYZE;
