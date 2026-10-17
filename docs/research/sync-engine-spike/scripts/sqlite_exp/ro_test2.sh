#!/bin/bash
# Follow-up probes: crash-leftover -wal/-shm, and whether an unsandboxed mode=ro open creates files.
cd "$(dirname "$0")"
SQ=/opt/homebrew/opt/sqlite/bin/sqlite3
SB="sandbox-exec -f $PWD/deny_write.sb"
rm -rf d_*
mkdir d_crash d_rofiles
$SQ d_crash/t.db "PRAGMA journal_mode=WAL; CREATE TABLE i(id INTEGER PRIMARY KEY, title TEXT); INSERT INTO i(title) VALUES('a'),('b');" >/dev/null

$SQ d_crash/t.db "PRAGMA wal_autocheckpoint=0; INSERT INTO i(title) VALUES('c');" ".shell sleep 30" >/dev/null 2>&1 &
sleep 1; pkill -9 -f "d_crash/t.db"; sleep 1
echo "== crash-left -wal/-shm, no live connection (files: $(ls d_crash | tr '\n' ' '))"
for uri in "t.db" "file:t.db?mode=ro" "file:t.db?immutable=1"; do
  out=$(cd d_crash && $SB $SQ -readonly "$uri" "SELECT count(*) FROM i;" 2>&1); echo "   [$uri sandboxed] exit=$? -> $out"
done

echo "== unsandboxed mode=ro on cleanly closed WAL db: does it create files?"
$SQ d_rofiles/t.db "PRAGMA journal_mode=WAL; CREATE TABLE i(x); INSERT INTO i VALUES(1);" >/dev/null
echo "   before: $(ls d_rofiles | tr '\n' ' ')"
$SQ "file:d_rofiles/t.db?mode=ro" "SELECT count(*) FROM i;" ".shell ls d_rofiles"
echo "   after close: $(ls d_rofiles | tr '\n' ' ')"

echo "== sandboxed reader, live writer, -shm file mode 444 is irrelevant; check lock interaction: reader during writer's open write txn"
mkdir d_lock; $SQ d_lock/t.db "PRAGMA journal_mode=WAL; CREATE TABLE i(x); INSERT INTO i VALUES(1);" >/dev/null
$SQ d_lock/t.db "BEGIN IMMEDIATE; INSERT INTO i VALUES(2);" ".shell sleep 4" "COMMIT;" >/dev/null 2>&1 &
sleep 1
out=$(cd d_lock && $SB $SQ -readonly "file:t.db?mode=ro" "SELECT count(*) FROM i;" 2>&1); echo "   [mode=ro sandboxed, writer mid-txn] exit=$? -> $out"
wait
