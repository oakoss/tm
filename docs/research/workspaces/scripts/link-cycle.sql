-- Cost of refusing a blocking Link that would close a cycle, checked at write
-- time with a recursive query over a links projection.
-- Run: sqlite3 <scratch.db> < link-cycle.sql

PRAGMA journal_mode = WAL;
DROP TABLE IF EXISTS links;

-- One row per present Link in its canonical direction: "B is blocked by A" is
-- stored as (blocks, A, B).
CREATE TABLE links (
  type TEXT NOT NULL,
  src INTEGER NOT NULL,
  dst INTEGER NOT NULL,
  PRIMARY KEY (type, src, dst)
) STRICT, WITHOUT ROWID;
CREATE INDEX links_by_dst ON links (dst, type, src);

-- 10,000 Issues; each blocks the next two and one at a pseudo-random later
-- position, so the graph is acyclic and Issue 1 reaches nearly everything.
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 10000)
INSERT OR IGNORE INTO links (type, src, dst)
SELECT 'blocks', i, i + 1 FROM n WHERE i < 10000
UNION ALL SELECT 'blocks', i, i + 2 FROM n WHERE i < 9999
UNION ALL SELECT 'blocks', i, i + 1 + (i * 7919) % (10000 - i) FROM n WHERE i < 9999;
-- Unrelated Links that the check must ignore.
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 10000)
INSERT OR IGNORE INTO links (type, src, dst)
SELECT 'relates_to', min(i, 10001 - i), max(i, 10001 - i) FROM n WHERE i <> 10001 - i;

SELECT 'links: ' || count(*) || ' (blocks: ' || sum(type = 'blocks') || ')' FROM links;

-- Adding "X blocks Y" closes a cycle when Y already reaches X. Each query walks
-- from Y. `reach CROSS JOIN links` pins the join order: with `links JOIN reach`
-- and no ANALYZE statistics the planner scans every `blocks` row per step.
.timer on

.print '-- planner-chosen order (links JOIN reach), full walk from Issue 2'
SELECT 'reachable from 2: ' || count(*) FROM (
  WITH RECURSIVE reach(id) AS (
    SELECT 2
    UNION
    SELECT l.dst FROM links l JOIN reach r ON l.type = 'blocks' AND l.src = r.id
  )
  SELECT id FROM reach
);

.print '-- pinned order (reach CROSS JOIN links), full walk from Issue 2'
SELECT 'reachable from 2: ' || count(*) FROM (
  WITH RECURSIVE reach(id) AS (
    SELECT 2
    UNION
    SELECT l.dst FROM reach r CROSS JOIN links l ON l.type = 'blocks' AND l.src = r.id
  )
  SELECT id FROM reach
);

.print '-- refused: adding "10000 blocks 1" (1 reaches 10000)'
SELECT 'closes a cycle: ' || EXISTS (
  WITH RECURSIVE reach(id) AS (
    SELECT 1
    UNION
    SELECT l.dst FROM reach r CROSS JOIN links l ON l.type = 'blocks' AND l.src = r.id
  )
  SELECT 1 FROM reach WHERE id = 10000
);

.print '-- refused: adding "5000 blocks 4990" (4990 reaches 5000)'
SELECT 'closes a cycle: ' || EXISTS (
  WITH RECURSIVE reach(id) AS (
    SELECT 4990
    UNION
    SELECT l.dst FROM reach r CROSS JOIN links l ON l.type = 'blocks' AND l.src = r.id
  )
  SELECT 1 FROM reach WHERE id = 5000
);

.print '-- allowed: adding "1 blocks 10000" (10000 reaches nothing)'
SELECT 'closes a cycle: ' || EXISTS (
  WITH RECURSIVE reach(id) AS (
    SELECT 10000
    UNION
    SELECT l.dst FROM reach r CROSS JOIN links l ON l.type = 'blocks' AND l.src = r.id
  )
  SELECT 1 FROM reach WHERE id = 1
);

.print '-- allowed, worst case: adding "10001 blocks 1" walks everything 1 reaches'
SELECT 'closes a cycle: ' || EXISTS (
  WITH RECURSIVE reach(id) AS (
    SELECT 1
    UNION
    SELECT l.dst FROM reach r CROSS JOIN links l ON l.type = 'blocks' AND l.src = r.id
  )
  SELECT 1 FROM reach WHERE id = 10001
);
