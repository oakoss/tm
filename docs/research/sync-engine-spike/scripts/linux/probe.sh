#!/bin/bash
# Linux port of the macOS read-only and concurrency probes.
# Usage: probe.sh <sandbox>  where sandbox is one of: none, bwrap, codex
cd "$(dirname "$0")"
SQ=sqlite3
case "$1" in
  none)  SB=() ;;
  bwrap) SB=(bwrap --ro-bind / / --dev /dev --proc /proc --) ;;
  codex) SB=(codex sandbox -c 'sandbox_mode="read-only"' --) ;;
  *) echo "usage: $0 none|bwrap|codex"; exit 2 ;;
esac
W=work_$1; rm -rf "$W"; mkdir "$W"; cd "$W"

echo "sanity (write under sandbox): $("${SB[@]}" touch probe_file 2>&1; echo "exit=$?")"
rm -f probe_file

mk() { # dir journal_mode
  mkdir -p "$1"
  $SQ "$1/t.db" "PRAGMA journal_mode=$2; CREATE TABLE i(id INTEGER PRIMARY KEY, title TEXT); INSERT INTO i(title) VALUES('a'),('b');" >/dev/null
}
probe() { # label dir
  echo "== $1 (files: $(ls "$2" | tr '\n' ' '))"
  for uri in "t.db" "file:t.db?mode=ro" "file:t.db?immutable=1"; do
    out=$(cd "$2" && "${SB[@]}" $SQ -readonly "$uri" "SELECT count(*) FROM i;" 2>&1); echo "   [$uri -readonly] exit=$? -> $out"
  done
  echo "   files after: $(ls "$2" | tr '\n' ' ')"
}

mk rbj delete
probe "rollback journal" rbj

mk wal_clean wal
probe "WAL, cleanly closed" wal_clean

mk wal_persist wal
$SQ "file:wal_persist/t.db?mode=ro" "SELECT 1;" >/dev/null
probe "WAL, -wal/-shm left on disk" wal_persist

mk wal_live wal
( $SQ wal_live/t.db "PRAGMA wal_autocheckpoint=0; INSERT INTO i(title) VALUES('c'); SELECT 1 FROM i LIMIT 1;" ".shell sleep 6" >/dev/null 2>&1 & )
sleep 1
probe "WAL, live writer (3 committed, 1 only in -wal)" wal_live
sleep 6

mk wal_rodir wal
chmod 444 wal_rodir/t.db; chmod 555 wal_rodir
probe "WAL, chmod 555 dir + 444 db" wal_rodir
chmod 755 wal_rodir; chmod 644 wal_rodir/t.db

mk snap_src wal
mkdir snap
$SQ snap_src/t.db "VACUUM INTO 'snap/t.db.tmp';" && mv snap/t.db.tmp snap/t.db
$SQ snap/t.db "PRAGMA journal_mode=delete;" >/dev/null
probe "snapshot (VACUUM INTO + rename, rollback mode)" snap

echo "== socket connect under sandbox"
python3 -c "import socket,os;s=socket.socket(socket.AF_UNIX);s.bind('s.sock');s.listen(1);import time;time.sleep(4)" &
sleep 1
out=$("${SB[@]}" python3 -c "import socket;s=socket.socket(socket.AF_UNIX);s.connect('s.sock');print('connected')" 2>&1 | tail -1); echo "   unix socket: $out"
wait; rm -f s.sock
