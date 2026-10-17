#!/usr/bin/env bash
# Can a stored operation's server effects (server_seq, server_received_ms,
# assigned_key) be overwritten once set? Runs the log DDL from schema.rs
# against a scratch database.
# Usage: server-effects-rewrite.sh <path-to-repo> <scratch-dir>
set -euo pipefail
repo=$1
db="$2/effects.db"
rm -f "$db"

# The LOG constant in crates/tm-store/src/schema.rs, between its opening and closing quotes.
ddl=$(awk '/^pub\(crate\) const LOG: &str = "$/{on=1; next} on && /^";$/{exit} on' \
  "$repo/crates/tm-store/src/schema.rs")

sqlite3 "$db" <<SQL
$ddl
INSERT INTO ops (op_id, op_hash, v, kind, replica_id, actor, project, issue_id,
                 hlc, base_seq, utc_ms, offset_min, payload)
VALUES (x'01', x'02', 1, 'create', x'03', 'jace', 'CPL', x'04', 1, 0, 1, 0, '{"kind":"create"}');
.print 'first assignment (NULL -> value):'
UPDATE ops SET server_seq = 1, server_received_ms = 10, assigned_key = 'CPL-1';
SELECT '  changes=' || changes();
.print 'second assignment (value -> other value):'
UPDATE ops SET server_seq = 7, server_received_ms = 99, assigned_key = 'CPL-2';
SELECT '  changes=' || changes();
.print 'clearing (value -> NULL):'
UPDATE ops SET assigned_key = NULL;
SELECT '  changes=' || changes();
SELECT '  final: server_seq=' || server_seq || ' received=' || server_received_ms
       || ' assigned_key=' || coalesce(assigned_key, 'NULL') FROM ops;
SQL

echo
echo 'with a set-once trigger added:'
rm -f "$db"
sqlite3 "$db" <<SQL
$ddl
CREATE TRIGGER ops_effects_set_once
BEFORE UPDATE OF server_seq, server_received_ms, assigned_key ON ops
WHEN (OLD.server_seq IS NOT NULL AND NEW.server_seq IS NOT OLD.server_seq)
  OR (OLD.server_received_ms IS NOT NULL AND NEW.server_received_ms IS NOT OLD.server_received_ms)
  OR (OLD.assigned_key IS NOT NULL AND NEW.assigned_key IS NOT OLD.assigned_key)
BEGIN
  SELECT RAISE(ABORT, 'server effects are set once');
END;
INSERT INTO ops (op_id, op_hash, v, kind, replica_id, actor, project, issue_id,
                 hlc, base_seq, utc_ms, offset_min, payload)
VALUES (x'01', x'02', 1, 'create', x'03', 'jace', 'CPL', x'04', 1, 0, 1, 0, '{"kind":"create"}');
UPDATE ops SET server_seq = 1, server_received_ms = 10, assigned_key = 'CPL-1';
SELECT '  first assignment changes=' || changes();
SQL
for stmt in "UPDATE ops SET assigned_key = 'CPL-2'" "UPDATE ops SET assigned_key = NULL" \
            "UPDATE ops SET server_seq = 7" "UPDATE ops SET assigned_key = 'CPL-1'"; do
  printf '  %s -> ' "$stmt"
  sqlite3 -bail "$db" "$stmt; SELECT 'ok, changes=' || changes();" 2>&1 || true
done
