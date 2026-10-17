#!/usr/bin/env bash
# usage: ro_stress.sh <tursodb> <workdir> <reads>
# Default-mode writer inserts 20000 rows (autocheckpoint on, 1000-frame default) over ~10 s;
# a sandboxed --readonly reader opens and counts repeatedly. Reports errors and count regressions.
set -u
T=$1; D=$2; N=$3
rm -rf "$D"; mkdir -p "$D"; cd "$D"
PROFILE="(version 1)(allow default)(deny file-write*)"
"$T" -q t.db "CREATE TABLE ops(id INTEGER PRIMARY KEY, n INTEGER, pad TEXT);" >/dev/null 2>&1
{ for n in $(seq 1 20000); do echo "INSERT INTO ops(n,pad) VALUES ($n, hex(randomblob(200)));"; [ $((n % 200)) = 0 ] && sleep 0.1; done; } | "$T" -q t.db > w.out 2>&1 &
WP=$!
prev=0; ok=0; err=0; regress=0
for i in $(seq 1 "$N"); do
  r=$(sandbox-exec -p "$PROFILE" "$T" -q -m list --readonly t.db 'SELECT count(*) FROM ops;' 2>&1 | tr '\n' ' ' | sed 's/ *$//')
  if [[ "$r" =~ ^[0-9]+$ ]]; then
    ok=$((ok+1)); [ "$r" -lt "$prev" ] && { regress=$((regress+1)); echo "  regression: $prev -> $r"; }; prev=$r
  else
    err=$((err+1)); echo "$r" >> r.err
  fi
done
wait $WP
echo "reads=$N ok=$ok errors=$err regressions=$regress last_read=$prev final=$("$T" -q -m list t.db 'SELECT count(*) FROM ops;')"
[ -f r.err ] && sed -E 's/[0-9]+/N/g' r.err | sort | uniq -c | sort -rn | head -5
true
