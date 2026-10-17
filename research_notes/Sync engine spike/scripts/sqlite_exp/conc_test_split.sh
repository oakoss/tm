#!/bin/bash
# Variant: separate SELECT before the INSERT, so a DEFERRED txn must upgrade read->write lock.
cd "$(dirname "$0")"
SQ=/opt/homebrew/opt/sqlite/bin/sqlite3
run() { # label begin_stmt busy_timeout_ms
  rm -f c.db c.db-wal c.db-shm
  $SQ c.db "PRAGMA journal_mode=WAL; CREATE TABLE ops(id INTEGER PRIMARY KEY, n INT);" >/dev/null
  rm -f errs.txt
  start=$(date +%s.%N 2>/dev/null || python3 -c 'import time;print(time.time())')
  for p in $(seq 1 50); do
    ( for k in $(seq 1 10); do
        $SQ -cmd ".timeout $3" c.db "$2; SELECT max(n) FROM ops; INSERT INTO ops(n) SELECT coalesce(max(n),0)+1 FROM ops; COMMIT;" >/dev/null 2>>errs.txt
      done ) &
  done
  wait
  rows=$($SQ c.db "SELECT count(*), count(DISTINCT n) FROM ops;")
  errs=$(grep -c . errs.txt); sample=$(head -1 errs.txt)
  echo "$1: rows(total|distinct n)=$rows of 500, errors=$errs ${sample:+e.g. '$sample'}"
}
run "BEGIN DEFERRED, busy_timeout=0    " "BEGIN DEFERRED" 0
run "BEGIN DEFERRED, busy_timeout=5000 " "BEGIN DEFERRED" 5000
run "BEGIN IMMEDIATE, busy_timeout=0   " "BEGIN IMMEDIATE" 0
run "BEGIN IMMEDIATE, busy_timeout=5000" "BEGIN IMMEDIATE" 5000
