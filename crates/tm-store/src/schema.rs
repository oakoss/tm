//! Table definitions. The log tables are permanent; the projection tables are
//! dropped and rebuilt from the log whenever `PROJECTION_VERSION` changes.

/// Stamped into `PRAGMA user_version`; a store with a different value is
/// rebuilt on open.
pub(crate) const PROJECTION_VERSION: i64 = 1;

/// Format of the log tables; a binary refuses a store with a higher one.
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

-- Stored operations are immutable; only the columns the server fills in later
-- may change.
CREATE TRIGGER IF NOT EXISTS ops_immutable
BEFORE UPDATE OF op_id, op_hash, v, kind, replica_id, actor, project, issue_id, field, elem,
                 hlc, base_seq, utc_ms, offset_min, payload ON ops
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
";

pub(crate) const DROP_PROJECTIONS: &str = "
DROP TABLE IF EXISTS comments;
DROP TABLE IF EXISTS set_members;
DROP TABLE IF EXISTS field_versions;
DROP TABLE IF EXISTS claims;
DROP TABLE IF EXISTS keys;
DROP TABLE IF EXISTS issues;
";

/// Built-in typed columns arrive with the Scheme; until then every field value
/// lives in `issues.fields`.
pub(crate) const PROJECTIONS: &str = "
CREATE TABLE issues (
  id BLOB PRIMARY KEY,
  project TEXT NOT NULL,
  created_hlc INTEGER NOT NULL,
  fields TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(fields))
) STRICT;

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
  PRIMARY KEY (issue_id, field, elem)
) STRICT, WITHOUT ROWID;

CREATE INDEX set_members_by_elem ON set_members (field, elem);

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

CREATE TABLE claims (
  issue_id BLOB PRIMARY KEY,
  agent TEXT NOT NULL,
  actor TEXT NOT NULL,
  op_id BLOB NOT NULL,
  expires_ms INTEGER
) STRICT;
";
