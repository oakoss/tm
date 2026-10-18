//! Table definitions. The log tables are permanent; the projection tables are
//! dropped and rebuilt from the log whenever `PROJECTION_VERSION` changes.

/// Stamped into `PRAGMA user_version`; a store with a different value is
/// rebuilt on open.
pub(crate) const PROJECTION_VERSION: i64 = 6;

/// Format of the log tables; a binary refuses a store with a higher one.
/// Until tm v1 is released the log changes in place (ADR 0010).
pub(crate) const STORE_FORMAT: i64 = 1;

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

-- Server order is per Project (one chain per Project, ADR 0006).
CREATE UNIQUE INDEX IF NOT EXISTS ops_server_position ON ops (project, server_seq);

-- A write reads its own Replica's latest clock value with one seek.
CREATE INDEX IF NOT EXISTS ops_by_replica ON ops (replica_id, hlc);

-- Stored operations are immutable; only the columns the server fills in later
-- may change.
CREATE TRIGGER IF NOT EXISTS ops_immutable
BEFORE UPDATE OF op_id, op_hash, v, kind, replica_id, actor, on_behalf_of, project, issue_id,
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
";

pub(crate) const DROP_PROJECTIONS: &str = "
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
  updated_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX issues_by_status ON issues (type, status);
CREATE INDEX issues_by_priority ON issues (priority);

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

CREATE TABLE comments (
  op_id BLOB PRIMARY KEY,
  issue_id BLOB NOT NULL REFERENCES issues (id),
  actor TEXT NOT NULL,
  body TEXT NOT NULL,
  hlc INTEGER NOT NULL,
  utc_ms INTEGER NOT NULL,
  offset_min INTEGER NOT NULL
) STRICT;

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
  expires_ms INTEGER
) STRICT;
";
