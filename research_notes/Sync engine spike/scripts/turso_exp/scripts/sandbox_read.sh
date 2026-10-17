#!/usr/bin/env bash
# usage: sandbox_read.sh <tursodb> <workdir> "<writer flags>" "<reader flags>"
# Writer inserts 500 rows, then stays open (no close checkpoint) while a sandboxed reader
# (deny all file writes) opens the DB and counts rows.
set -u
T=$1; D=$2; WF=$3; RF=$4
rm -rf "$D"; mkdir -p "$D"; cd "$D"
PROFILE="(version 1)(allow default)(deny file-write*)"
"$T" -q $WF t.db "CREATE TABLE ops(id INTEGER PRIMARY KEY, n INTEGER);" >/dev/null 2>&1
{ echo "PRAGMA wal_autocheckpoint=0;"; for n in $(seq 1 500); do echo "INSERT INTO ops(n) VALUES ($n);"; done; sleep 3; } | "$T" -q $WF t.db > w.out 2>&1 &
sleep 1
echo "  files while writer open: $(ls -l t.db* | awk '{print $NF"="$5}' | tr '\n' ' ')"
echo "  sandbox write probe: $(sandbox-exec -p "$PROFILE" /usr/bin/touch probe 2>&1 || true)"
echo "  sandboxed reader ($RF): $(sandbox-exec -p "$PROFILE" "$T" -q -m list $RF t.db 'SELECT count(*) FROM ops;' 2>&1 | tr '\n' ' ')"
echo "  sandboxed sqlite3 -readonly: $(sandbox-exec -p "$PROFILE" /usr/bin/sqlite3 -readonly t.db 'SELECT count(*) FROM ops;' 2>&1 | tr '\n' ' ')"
wait
echo "  writer out: $(cat w.out | tr '\n' ' ')"
echo "  files after: $(ls -l t.db* | awk '{print $NF"="$5}' | tr '\n' ' ')"
