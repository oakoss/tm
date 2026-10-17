#!/bin/bash
# Same probes as ro_test.sh, but under Codex's own read-only sandbox (codex sandbox, sandbox_mode=read-only).
cd "$(dirname "$0")"
SQ=/opt/homebrew/opt/sqlite/bin/sqlite3
CX=(codex sandbox -c 'sandbox_mode="read-only"' --)
echo "sanity: $("${CX[@]}" touch probe_file 2>&1; echo "exit=$?")"
rm -rf e_*; mkdir e_rbj e_clean e_persist
$SQ e_rbj/t.db "CREATE TABLE i(x); INSERT INTO i VALUES(1),(2);" >/dev/null
$SQ e_clean/t.db "PRAGMA journal_mode=WAL; CREATE TABLE i(x); INSERT INTO i VALUES(1),(2);" >/dev/null
$SQ e_persist/t.db "PRAGMA journal_mode=WAL; CREATE TABLE i(x); INSERT INTO i VALUES(1),(2);" >/dev/null
$SQ "file:e_persist/t.db?mode=ro" "SELECT 1;" >/dev/null # a read-only close leaves -wal/-shm on disk
for d in e_rbj e_clean e_persist; do
  echo "== $d (files: $(ls $d | tr '\n' ' '))"
  for uri in "t.db" "file:t.db?mode=ro" "file:t.db?immutable=1"; do
    out=$(cd $d && "${CX[@]}" $SQ -readonly "$uri" "SELECT count(*) FROM i;" 2>&1); echo "   [$uri] exit=$? -> $out"
  done
done
