#!/bin/bash
# Naive opens (no -readonly, no URI; Python default connect) and a hot rollback journal, under Codex read-only sandbox.
cd "$(dirname "$0")"
SQ=/opt/homebrew/opt/sqlite/bin/sqlite3
CX=(codex sandbox -c 'sandbox_mode="read-only"' --)
for d in e_rbj e_clean e_persist; do
  echo "== $d (files: $(ls $d | tr '\n' ' '))"
  out=$(cd $d && "${CX[@]}" $SQ t.db "SELECT count(*) FROM i;" 2>&1); echo "   [sqlite3 t.db, no flags] exit=$? -> $out"
  out=$(cd $d && "${CX[@]}" /usr/bin/python3 -I -c "import sqlite3; print(sqlite3.connect('t.db').execute('select count(*) from i').fetchone())" 2>&1 | tail -1); echo "   [python3 sqlite3.connect('t.db')] exit=$? -> $out"
done

rm -rf f_hot; mkdir f_hot
$SQ f_hot/t.db "PRAGMA journal_mode=DELETE; CREATE TABLE i(x); INSERT INTO i VALUES(1),(2);" >/dev/null
# Writer modifies pages with a cache too small to hold them, forcing spills to the db file, then is killed mid-transaction.
$SQ f_hot/t.db "PRAGMA cache_size=1; BEGIN; WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM c WHERE n<5000) INSERT INTO i SELECT randomblob(500) FROM c;" ".shell sleep 30" >/dev/null 2>&1 &
sleep 2; pkill -9 -f "f_hot/t.db"; sleep 1
echo "== f_hot: DELETE-mode writer killed mid-txn (files: $(ls f_hot | tr '\n' ' '))"
for uri in "t.db" "file:t.db?mode=ro" "file:t.db?immutable=1"; do
  out=$(cd f_hot && "${CX[@]}" $SQ -readonly "$uri" "SELECT count(*) FROM i;" 2>&1); echo "   [$uri -readonly] exit=$? -> $out"
done
out=$($SQ f_hot/t.db "SELECT count(*) FROM i;" 2>&1); echo "   [unsandboxed read-write open rolls back] -> $out; files now: $(ls f_hot | tr '\n' ' ')"
