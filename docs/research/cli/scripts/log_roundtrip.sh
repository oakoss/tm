#!/usr/bin/env bash
# Does a store holding only the operation log, under a new Replica, rebuild to
# the same `tm show --json` and `tm list --json`? This is export layer (a): the
# log carried verbatim.
#
# Usage: log_roundtrip.sh <tm-binary> <scratch-dir>
# Writes only under <scratch-dir>; needs git, sqlite3 and python3.
set -u

tm=$1
scratch=$2
mkdir -p "$scratch"
# A global excludes file may ignore .config/, which `tm init` refuses; this
# child shell reads no global git config.
export TM_HOME="$scratch/home" TM_ACTOR=jace GIT_CONFIG_GLOBAL=/dev/null
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CODEX_SESSION_ID CODEX_THREAD_ID \
  GEMINI_CLI OPENCODE CURSOR_AGENT TM_AGENT TM_JSON
repo="$scratch/repo"
mkdir -p "$repo"
git -C "$repo" init -q
cd "$repo" || exit 1

"$tm" init --key RT --name roundtrip --yes >/dev/null || exit 1
key() { python3 -I -c 'import json,sys; print(json.load(sys.stdin)["data"]["key"])'; }
batch() { python3 -I -c 'import json,sys; print(json.load(sys.stdin)["data"]["batch"])'; }

epic=$("$tm" create --type epic --title "Epic" --json | key)
a=$("$tm" create --type task --title "A" --parent "$epic" --label Alpha --label beta \
  --set priority=high --set "description=Body *with* markdown" --json | key)
b=$("$tm" create --type bug --title "B" --set assignee=jace --json | key)
"$tm" link "$a" blocks "$b" --json >/dev/null
"$tm" link "$b" relates to "$epic" --json >/dev/null
"$tm" move "$a" in_progress --json >/dev/null
"$tm" comment "$a" "kept" --json >/dev/null
retracted=$("$tm" comment "$a" "retracted" --json | batch)
"$tm" undo "$retracted" --json >/dev/null
gone=$("$tm" create --type task --title "Archived by undo" --json | batch)
"$tm" undo "$gone" --json >/dev/null
TM_AGENT=claude "$tm" claim "$b" --json >/dev/null

snapshot() {
  {
    "$tm" list archived:true --json
    "$tm" list archived:false --json
    for k in "$epic" "$a" "$b" RT-4; do "$tm" show "$k" --json; done
  } | python3 -I -c '
import json, sys
dec = json.JSONDecoder(); text = sys.stdin.read(); i = 0; docs = []
while i < len(text):
    if text[i].isspace(): i += 1; continue
    doc, i = dec.raw_decode(text, i); docs.append(doc)
    if "error" in doc: sys.exit("command failed: " + json.dumps(doc))
print(json.dumps(docs, sort_keys=True, indent=1))'
}

snapshot >"$scratch/before.json" || exit 1
db=$(ls "$TM_HOME"/workspaces/*/store.db)
cp "$db" "$scratch/original.db"
sqlite3 "$db" "SELECT 'ops', count(*), count(DISTINCT replica_id) FROM ops;
  SELECT 'op_batches', count(*) FROM op_batches; SELECT 'undo_batches', count(*) FROM undo_batches;
  SELECT 'replica', hex(v) FROM meta WHERE k = 'replica_id';"

# A new Replica id, not none: a store with operations and no Replica id is
# refused as corrupt.
sqlite3 "$db" "DELETE FROM op_batches; DELETE FROM undo_batches;
  UPDATE meta SET v = randomblob(16) WHERE k = 'replica_id';
  UPDATE meta SET v = 0 WHERE k = 'last_hlc';
  DROP TABLE comments; DROP TABLE links; DROP TABLE set_members; DROP TABLE field_values;
  DROP TABLE unapplied; DROP TABLE field_versions; DROP TABLE claims; DROP TABLE keys;
  DROP TABLE key_numbers; DROP TABLE issues; DROP TABLE project_keys; DROP TABLE projects;
  PRAGMA user_version = 0;"

snapshot >"$scratch/after.json" || exit 1
sqlite3 "$db" "SELECT 'replica after', hex(v) FROM meta WHERE k = 'replica_id';
  SELECT 'unapplied', count(*) FROM unapplied;"
if cmp -s "$scratch/before.json" "$scratch/after.json"; then
  echo "show and list identical after rebuilding from the log alone ($(wc -c <"$scratch/before.json") bytes)"
else
  echo "DIFFER"; diff "$scratch/before.json" "$scratch/after.json" | head -40
fi

# Nothing observed the copied clocks, so this holds only because wall time moved on.
"$tm" create --type task --title "after" --json | key
sqlite3 "$db" "SELECT 'hlc ordered after the log', (SELECT hlc FROM ops ORDER BY seq DESC LIMIT 1) >
  (SELECT max(hlc) FROM ops WHERE seq < (SELECT max(seq) FROM ops));"

# The same log copied into a Workspace that already has a Project keyed RT.
export TM_HOME="$scratch/home2"
repo2="$scratch/repo2"
mkdir -p "$repo2"
git -C "$repo2" init -q
cd "$repo2" || exit 1
"$tm" init --key RT --name other --yes >/dev/null || exit 1
"$tm" create --type task --title "local one" --json >/dev/null
"$tm" create --type task --title "local two" --json >/dev/null
db2=$(ls "$TM_HOME"/workspaces/*/store.db)
sqlite3 "$db2" "ATTACH '$scratch/original.db' AS src;
  INSERT INTO ops (op_id, op_hash, v, kind, replica_id, actor, on_behalf_of, project, issue_id,
                   field, elem, hlc, base_seq, utc_ms, offset_min, payload, server_seq,
                   server_received_ms, assigned_key)
  SELECT op_id, op_hash, v, kind, replica_id, actor, on_behalf_of, project, issue_id,
         field, elem, hlc, base_seq, utc_ms, offset_min, payload, server_seq,
         server_received_ms, assigned_key FROM src.ops ORDER BY seq;
  DETACH src; PRAGMA user_version = 0;"
"$tm" list --json >/dev/null
sqlite3 "$db2" "SELECT 'populated: issues', count(*) FROM issues;
  SELECT 'populated: projects', count(*) FROM projects;
  SELECT 'populated: keys', count(*) FROM keys;
  SELECT 'populated: unapplied', count(*) FROM unapplied;
  SELECT 'populated: reason', reason, count(*) FROM unapplied GROUP BY reason;"
