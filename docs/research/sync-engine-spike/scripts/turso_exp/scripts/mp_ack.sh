#!/usr/bin/env bash
# usage: mp_ack.sh <tursodb> <workdir> <procs> <rows_per_proc> [busy_ms]
# After each autocommitted INSERT, a separate SELECT reads the row back and prints "ACK:<proc>:<n>";
# acked rows missing from the final DB are lost writes.
set -u
T=$1; D=$2; P=$3; R=$4; BUSY=${5:-10000}
FLAG="--experimental-multiprocess-wal"
rm -rf "$D"; mkdir -p "$D/logs"; cd "$D"
"$T" -q $FLAG t.db "CREATE TABLE ops(id INTEGER PRIMARY KEY, proc INTEGER, n INTEGER);" >/dev/null 2>&1
for p in $(seq 1 "$P"); do
  { echo "PRAGMA busy_timeout=$BUSY;"
    for n in $(seq 1 "$R"); do echo "INSERT INTO ops(proc,n) VALUES ($p,$n);"; echo "SELECT 'ACK:'||proc||':'||n FROM ops WHERE proc=$p AND n=$n;"; done
  } > "logs/in_$p.sql"
done
for p in $(seq 1 "$P"); do
  ( "$T" -q -m list $FLAG t.db < "logs/in_$p.sql" > "logs/out_$p.txt" 2>&1; echo $? > "logs/rc_$p" ) &
done
wait 2>/dev/null
mkdir pristine && command cp t.db* pristine/
echo "files after run: $(ls -l t.db* | awk '{print $NF"="$5}' | tr '\n' ' ')"
# Read with tursodb (single-process mode) on a copy so no reader mutates the pristine files.
mkdir tcheck && command cp pristine/t.db* tcheck/ && rm -f tcheck/t.db-tshm
echo "tursodb count on copy: $("$T" -q -m list tcheck/t.db 'SELECT count(*) FROM ops;' 2>&1)"
"$T" -q -m list tcheck/t.db "SELECT 'ACK:'||proc||':'||n FROM ops;" 2>/dev/null | grep '^ACK:' | sort -u > indb_turso.txt
echo "tursodb view: acked_but_missing=$(grep -h '^ACK:' logs/out_*.txt | sort -u | comm -23 - indb_turso.txt | wc -l | tr -d ' ')"
grep -h '^ACK:' logs/out_*.txt | sort -u > acked.txt
sqlite3 t.db "SELECT 'ACK:'||proc||':'||n FROM ops;" | sort -u > indb.txt
nz=0; for p in $(seq 1 "$P"); do [ "$(cat logs/rc_$p)" != 0 ] && nz=$((nz+1)); done
echo "expected=$((P*R)) acked=$(wc -l < acked.txt | tr -d ' ') in_db=$(wc -l < indb.txt | tr -d ' ') acked_but_missing=$(comm -23 acked.txt indb.txt | wc -l | tr -d ' ') in_db_not_acked=$(comm -13 acked.txt indb.txt | wc -l | tr -d ' ') nonzero_exit=$nz ic=$(sqlite3 t.db 'PRAGMA integrity_check')"
cat logs/out_*.txt | grep -v '^ACK:' | grep -v '^[0-9]*$' | grep -v '^ *$' | sed -E 's/[0-9]+/N/g' | sort | uniq -c | sort -rn | head -6
