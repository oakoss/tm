#!/usr/bin/env bash
# usage: reader.sh <tursodb> <workdir> "<writer flags>" "<reader flags>"
# A long-lived writer inserts 3000 rows one statement at a time; a separate reader process
# opens the DB 8 times during the write and reports count(*) each time.
set -u
T=$1; D=$2; WF=$3; RF=$4
rm -rf "$D"; mkdir -p "$D"; cd "$D"
"$T" -q $WF t.db "CREATE TABLE ops(id INTEGER PRIMARY KEY, n INTEGER);" >/dev/null 2>&1
{ echo "PRAGMA busy_timeout=10000;"; for n in $(seq 1 3000); do echo "INSERT INTO ops(n) VALUES ($n);"; [ $((n % 100)) = 0 ] && sleep 0.1; done; } | "$T" -q $WF t.db > w.out 2>&1 &
WP=$!
sleep 0.2
for i in $(seq 1 8); do
  echo "read $i: $("$T" -q -m list $RF t.db 'PRAGMA busy_timeout=5000; SELECT count(*) FROM ops;' 2>&1 | tr '\n' ' ')"
  sleep 0.3
done
wait $WP; echo "writer rc=$? writer_errors=$(grep -c . w.out)"
echo "final: $("$T" -q -m list $WF t.db 'SELECT count(*) FROM ops;' 2>&1)"
