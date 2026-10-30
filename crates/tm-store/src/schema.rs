//! Table definitions. The log tables are permanent; the projection tables are
//! dropped and rebuilt from the log whenever `PROJECTION_VERSION` changes.

/// Stamped into `PRAGMA user_version`; a store with a different value is
/// rebuilt on open.
pub(crate) const PROJECTION_VERSION: i64 = 14;

/// Format of the log tables; a binary refuses a store with a higher one.
/// Until tm v1 is released the log changes in place (ADR 0010).
pub(crate) const STORE_FORMAT: i64 = 1;

/// The `ops` columns in order; a store whose log has others is refused, since
/// `CREATE TABLE IF NOT EXISTS` never adds a column.
pub(crate) const OPS_COLUMNS: [&str; 21] = [
    "seq",
    "op_id",
    "op_hash",
    "v",
    "kind",
    "replica_id",
    "actor",
    "on_behalf_of",
    "source",
    "project",
    "issue_id",
    "field",
    "elem",
    "hlc",
    "base_seq",
    "utc_ms",
    "offset_min",
    "payload",
    "server_seq",
    "server_received_ms",
    "assigned_key",
];

pub(crate) const LOG: &str = "
CREATE TABLE IF NOT EXISTS meta (
  k TEXT PRIMARY KEY,
  v ANY
) STRICT;

CREATE TABLE IF NOT EXISTS ops (
  seq INTEGER PRIMARY KEY,
  op_id BLOB NOT NULL UNIQUE,
  op_hash BLOB NOT NULL,
  v INTEGER NOT NULL,
  kind TEXT NOT NULL,
  replica_id BLOB NOT NULL,
  actor TEXT NOT NULL,
  on_behalf_of TEXT,
  source TEXT,
  project TEXT NOT NULL,
  issue_id BLOB NOT NULL,
  field TEXT,
  elem TEXT,
  hlc INTEGER NOT NULL,
  base_seq INTEGER NOT NULL,
  utc_ms INTEGER NOT NULL,
  offset_min INTEGER NOT NULL,
  payload TEXT NOT NULL,
  server_seq INTEGER,
  server_received_ms INTEGER,
  assigned_key TEXT
) STRICT;

CREATE INDEX IF NOT EXISTS ops_target ON ops (issue_id, field, elem);

-- A re-import finds what it wrote before by source (ADR 0013).
CREATE INDEX IF NOT EXISTS ops_by_source ON ops (source) WHERE source IS NOT NULL;

-- Server order is per Project (one chain per Project, ADR 0006).
CREATE UNIQUE INDEX IF NOT EXISTS ops_server_position ON ops (project, server_seq);

-- A write reads its own Replica's latest clock value with one seek.
CREATE INDEX IF NOT EXISTS ops_by_replica ON ops (replica_id, hlc);

-- Stored operations are immutable; only the columns the server fills in later
-- may change.
CREATE TRIGGER IF NOT EXISTS ops_immutable
BEFORE UPDATE OF op_id, op_hash, v, kind, replica_id, actor, on_behalf_of, source, project, issue_id,
                 field, elem, hlc, base_seq, utc_ms, offset_min, payload ON ops
BEGIN
  SELECT RAISE(ABORT, 'stored operations are immutable');
END;

-- `seq` aliases the rowid, which `UPDATE OF seq` misses when written as
-- `rowid`, `oid` or `_rowid_`.
CREATE TRIGGER IF NOT EXISTS ops_position_frozen
BEFORE UPDATE ON ops WHEN NEW.seq IS NOT OLD.seq
BEGIN
  SELECT RAISE(ABORT, 'stored operations are immutable');
END;

CREATE TRIGGER IF NOT EXISTS ops_append_only
BEFORE DELETE ON ops
BEGIN
  SELECT RAISE(ABORT, 'the operation log is append-only');
END;

-- Server order, receipt time and an assigned Key are filled in once, later
-- than the operation itself; rewriting one would renumber or reorder history.
CREATE TRIGGER IF NOT EXISTS ops_effects_set_once
BEFORE UPDATE OF server_seq, server_received_ms, assigned_key ON ops
WHEN (OLD.server_seq IS NOT NULL AND NEW.server_seq IS NOT OLD.server_seq)
  OR (OLD.server_received_ms IS NOT NULL
      AND NEW.server_received_ms IS NOT OLD.server_received_ms)
  OR (OLD.assigned_key IS NOT NULL AND NEW.assigned_key IS NOT OLD.assigned_key)
BEGIN
  SELECT RAISE(ABORT, 'server effects are set once');
END;

-- Only a create is given a Key; one on any other kind would never be read.
CREATE TRIGGER IF NOT EXISTS ops_key_on_create_insert
BEFORE INSERT ON ops
WHEN NEW.assigned_key IS NOT NULL AND NEW.kind <> 'create'
BEGIN
  SELECT RAISE(ABORT, 'only a create operation is given a Key');
END;

CREATE TRIGGER IF NOT EXISTS ops_key_on_create_update
BEFORE UPDATE OF assigned_key ON ops
WHEN NEW.assigned_key IS NOT NULL AND NEW.kind <> 'create'
BEGIN
  SELECT RAISE(ABORT, 'only a create operation is given a Key');
END;

-- Which write each local operation came from, for undo. Kept by this Replica
-- only: not hashed, synced or replayed, and a rebuild leaves it in place.
CREATE TABLE IF NOT EXISTS op_batches (
  op_id BLOB PRIMARY KEY,
  batch BLOB NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS op_batches_by_batch ON op_batches (batch);

-- Which batch each undo reverted, so a bare undo steps back (ADR 0012). Local
-- as op_batches is.
CREATE TABLE IF NOT EXISTS undo_batches (
  batch BLOB PRIMARY KEY,
  undid BLOB NOT NULL
) STRICT, WITHOUT ROWID;
";

pub(crate) const DROP_PROJECTIONS: &str = "
DROP TABLE IF EXISTS shortlist_actors;
DROP TABLE IF EXISTS shortlist_versions;
DROP TABLE IF EXISTS shortlists;
DROP TABLE IF EXISTS comments;
DROP TABLE IF EXISTS links;
DROP TABLE IF EXISTS set_members;
DROP TABLE IF EXISTS field_values;
DROP TABLE IF EXISTS unapplied;
DROP TABLE IF EXISTS field_versions;
DROP TABLE IF EXISTS claims;
DROP TABLE IF EXISTS keys;
DROP TABLE IF EXISTS key_numbers;
DROP TABLE IF EXISTS issues;
DROP TABLE IF EXISTS project_keys;
DROP TABLE IF EXISTS projects;
";

/// Built-in fields are columns on `issues`; Scheme fields are rows in
/// `field_values`. Nothing here depends on the Scheme: Status categories and
/// labels are looked up when reading.
pub(crate) const PROJECTIONS: &str = "
CREATE TABLE issues (
  id BLOB PRIMARY KEY,
  project TEXT NOT NULL,
  created_hlc INTEGER NOT NULL,
  type TEXT,
  title TEXT,
  description TEXT,
  status TEXT,
  priority INTEGER NOT NULL DEFAULT 0,
  assignee TEXT,
  reporter TEXT NOT NULL,
  parent_id BLOB,
  archived INTEGER NOT NULL DEFAULT 0,
  created_ms INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL,
  shortlist BLOB,
  shortlist_rank TEXT
) STRICT;

CREATE INDEX issues_by_status ON issues (type, status);
CREATE INDEX issues_by_priority ON issues (priority);
-- A list's members in order, ties broken by id (ADR 0016).
CREATE INDEX issues_by_shortlist ON issues (shortlist, shortlist_rank, id)
  WHERE shortlist IS NOT NULL;

-- `issue_id` is NOT NULL here and in `set_members` because filters compile
-- negation to `NOT IN`, which matches nothing once the subquery yields a NULL.
CREATE TABLE field_values (
  issue_id BLOB NOT NULL REFERENCES issues (id),
  field TEXT NOT NULL,
  txt TEXT,
  num REAL,
  PRIMARY KEY (issue_id, field)
) STRICT, WITHOUT ROWID;

-- Logged operations replay could not apply in full, such as a value another
-- Replica wrote that does not fit or a create whose Key collided; ADR 0006 has
-- every pulled operation stay in the log, so they are recorded here instead of
-- stopping the rebuild.
CREATE TABLE unapplied (
  seq INTEGER PRIMARY KEY,
  op_id BLOB NOT NULL,
  reason TEXT NOT NULL
) STRICT;

CREATE INDEX field_values_by_txt ON field_values (field, txt, issue_id);
CREATE INDEX field_values_by_num ON field_values (field, num, issue_id);

-- The operation that currently wins each field or set element; `elem` is ''
-- for single-value fields.
CREATE TABLE field_versions (
  issue_id BLOB NOT NULL REFERENCES issues (id),
  field TEXT NOT NULL,
  elem TEXT NOT NULL,
  present INTEGER NOT NULL,
  op_id BLOB NOT NULL,
  hlc INTEGER NOT NULL,
  replica_id BLOB NOT NULL,
  base_seq INTEGER NOT NULL,
  server_seq INTEGER,
  server_received_ms INTEGER,
  PRIMARY KEY (issue_id, field, elem)
) STRICT, WITHOUT ROWID;

CREATE TABLE set_members (
  issue_id BLOB NOT NULL REFERENCES issues (id),
  field TEXT NOT NULL,
  elem TEXT NOT NULL,
  display TEXT,
  PRIMARY KEY (issue_id, field, elem)
) STRICT, WITHOUT ROWID;

CREATE INDEX set_members_by_elem ON set_members (field, elem);

-- Present Links in canonical form (ADR 0009); removals live in
-- `field_versions` under the source's `links` field. No foreign keys: a
-- target in another Project's chain can replay after the Link.
CREATE TABLE links (
  type TEXT NOT NULL,
  src BLOB NOT NULL,
  dst BLOB NOT NULL,
  PRIMARY KEY (type, src, dst)
) STRICT, WITHOUT ROWID;

CREATE INDEX links_by_dst ON links (dst, type, src);
CREATE INDEX links_by_src ON links (src, type, dst);

CREATE TABLE comments (
  op_id BLOB PRIMARY KEY,
  issue_id BLOB NOT NULL REFERENCES issues (id),
  actor TEXT NOT NULL,
  body TEXT NOT NULL,
  hlc INTEGER NOT NULL,
  utc_ms INTEGER NOT NULL,
  offset_min INTEGER NOT NULL,
  retracted INTEGER NOT NULL DEFAULT 0,
  -- 'question' or 'answer' for the comments tm ask and tm answer write.
  role TEXT
) STRICT;

CREATE INDEX comments_by_issue ON comments (issue_id, hlc);

-- Keys outlive purged Issues, so they carry no foreign key.
CREATE TABLE keys (
  key TEXT PRIMARY KEY,
  issue_id BLOB NOT NULL,
  project TEXT NOT NULL,
  number INTEGER,
  provisional INTEGER NOT NULL
) STRICT;

CREATE INDEX keys_by_issue ON keys (issue_id);
CREATE UNIQUE INDEX keys_by_number ON keys (project, number);

-- The highest Key number each Project has given, counting creates that did
-- not apply, and the create that gave it.
CREATE TABLE key_numbers (
  project TEXT PRIMARY KEY,
  highest INTEGER NOT NULL,
  op_id BLOB NOT NULL
) STRICT;

CREATE TABLE projects (
  id TEXT PRIMARY KEY,
  key TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  created_ms INTEGER NOT NULL
) STRICT;

-- Every key a Project has held. A retired key keeps resolving to its Project
-- and is never given to another.
CREATE TABLE project_keys (
  key TEXT PRIMARY KEY,
  project TEXT NOT NULL REFERENCES projects (id)
) STRICT;

CREATE TABLE claims (
  issue_id BLOB PRIMARY KEY,
  agent TEXT NOT NULL,
  on_behalf_of TEXT NOT NULL,
  op_id BLOB NOT NULL,
  expires_ms INTEGER,
  ttl_ms INTEGER,
  session TEXT,
  handed INTEGER NOT NULL DEFAULT 0
) STRICT;

-- Short-lists (ADR 0016). A name is not unique here: two Replicas may each
-- choose one, which a write refuses but a merge can bring.
CREATE TABLE shortlists (
  id BLOB PRIMARY KEY,
  project TEXT NOT NULL,
  created_hlc INTEGER NOT NULL,
  name TEXT,
  size INTEGER NOT NULL DEFAULT 10,
  deleted INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE INDEX shortlists_by_name ON shortlists (project, name COLLATE NOCASE);

-- As `field_versions`, for Short-list fields and Actors.
CREATE TABLE shortlist_versions (
  shortlist_id BLOB NOT NULL REFERENCES shortlists (id),
  field TEXT NOT NULL,
  elem TEXT NOT NULL,
  present INTEGER NOT NULL,
  op_id BLOB NOT NULL,
  hlc INTEGER NOT NULL,
  replica_id BLOB NOT NULL,
  base_seq INTEGER NOT NULL,
  server_seq INTEGER,
  server_received_ms INTEGER,
  PRIMARY KEY (shortlist_id, field, elem)
) STRICT, WITHOUT ROWID;

CREATE TABLE shortlist_actors (
  shortlist_id BLOB NOT NULL REFERENCES shortlists (id),
  actor TEXT NOT NULL,
  PRIMARY KEY (shortlist_id, actor)
) STRICT, WITHOUT ROWID;

CREATE INDEX shortlist_actors_by_actor ON shortlist_actors (actor, shortlist_id);
";
