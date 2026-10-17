#!/bin/bash
# Probes read-only SQLite opens under a macOS Seatbelt profile that denies all file writes.
cd "$(dirname "$0")"
SQ=/opt/homebrew/opt/sqlite/bin/sqlite3
SB="sandbox-exec -f $PWD/deny_write.sb"
rm -rf c_*

mk() { # dir journal_mode
  mkdir -p "$1"
  $SQ "$1/t.db" "PRAGMA journal_mode=$2; CREATE TABLE i(id INTEGER PRIMARY KEY, title TEXT); INSERT INTO i(title) VALUES('a'),('b');" >/dev/null
}
probe() { # label dir
  echo "== $1 (files: $(ls "$2" | tr '\n' ' '))"
  for uri in "t.db" "file:t.db?mode=ro" "file:t.db?immutable=1"; do
    out=$(cd "$2" && $SB $SQ -readonly "$uri" "SELECT count(*) FROM i;" 2>&1); echo "   [$uri -readonly] exit=$? -> $out"
  done
}

mk c_rbj delete
probe "rollback journal, sandboxed reader" c_rbj

mk c_wal_clean wal
probe "WAL, clean close (stock build deletes -wal/-shm)" c_wal_clean

mk c_wal_live wal
# Writer keeps a connection open with uncheckpointed data in -wal for 6s.
( $SQ c_wal_live/t.db "PRAGMA wal_autocheckpoint=0; INSERT INTO i(title) VALUES('c'); SELECT 1 FROM i LIMIT 1;" ".shell sleep 6" >/dev/null 2>&1 & )
sleep 1
probe "WAL, live writer holds -wal/-shm (3 rows committed, 1 only in -wal)" c_wal_live
sleep 6

mk c_wal_rodir wal
ls c_wal_rodir
chmod 444 c_wal_rodir/t.db; chmod 555 c_wal_rodir
probe "WAL, chmod 555 dir + 444 db (no -shm)" c_wal_rodir
chmod 755 c_wal_rodir; chmod 644 c_wal_rodir/t.db

mk c_snap wal
$SQ c_snap/t.db "PRAGMA journal_mode=delete;" >/dev/null
probe "WAL db converted to rollback mode (snapshot)" c_snap
