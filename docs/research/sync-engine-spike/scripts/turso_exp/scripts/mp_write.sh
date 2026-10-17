#!/usr/bin/env bash
# usage: mp_write.sh <tursodb> <workdir> <procs> <rows_per_proc> <flag|noflag> [busy_ms]
set -u
T=$1; D=$2; P=$3; R=$4; MODE=$5; BUSY=${6:-0}
FLAG=""; [ "$MODE" = flag ] && FLAG="--experimental-multiprocess-wal"
rm -rf "$D"; mkdir -p "$D/logs"; cd "$D"
"$T" -q $FLAG t.db "CREATE TABLE ops(id INTEGER PRIMARY KEY, proc INTEGER, n INTEGER);" >/dev/null 2>&1
for p in $(seq 1 "$P"); do
  {
    [ "$BUSY" != 0 ] && echo "PRAGMA busy_timeout=$BUSY;"
    for n in $(seq 1 "$R"); do echo "INSERT INTO ops(proc,n) VALUES ($p,$n);"; done
  } > "logs/in_$p.sql"
done
start=$(date +%s)
for p in $(seq 1 "$P"); do
  ( "$T" -q $FLAG t.db < "logs/in_$p.sql" > "logs/out_$p.txt" 2>&1; echo $? > "logs/rc_$p" ) &
done
wait
end=$(date +%s)
nz=0; for p in $(seq 1 "$P"); do [ "$(cat logs/rc_$p)" != 0 ] && nz=$((nz+1)); done
echo "procs=$P rows_per_proc=$R mode=$MODE busy=$BUSY elapsed=$((end-start))s nonzero_exit=$nz"
echo "error lines (top kinds):"
cat logs/out_*.txt | grep -iv '^$' | sed -E 's/[0-9]+/N/g' | sort | uniq -c | sort -rn | head -8
echo "expected=$((P*R)) actual(same flag)=$("$T" -q $FLAG -m list t.db "SELECT count(*) FROM ops;" 2>&1)"
echo "integrity=$("$T" -q $FLAG -m list t.db "PRAGMA integrity_check;" 2>&1 | head -3)"
ls -la "$D" | grep t.db
