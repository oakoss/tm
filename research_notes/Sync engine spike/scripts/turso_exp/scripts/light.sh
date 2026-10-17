#!/usr/bin/env bash
# usage: light.sh <exp_root> <runs> "<procs> <rows>"...
E=$1; RUNS=$2; shift 2
SHAPES=("$@")
for v in ${VERSIONS:-v0.8.3 v0.9.0-pre.1}; do
  T=$E/bin/$v/turso_cli-aarch64-apple-darwin/tursodb
  for shape in "${SHAPES[@]}"; do
    read -r P R <<< "$shape"
    bad=0; lost=0; panics=0; mixrej=0; acked=0
    for i in $(seq 1 "$RUNS"); do
      D=$E/runs/light-$v-${P}x${R}-$i
      out=$(bash "$E/scripts/mp_ack.sh" "$T" "$D" "$P" "$R" 2>&1)
      l=$(echo "$out" | sed -nE 's/^tursodb view: acked_but_missing=([0-9]+)/\1/p')
      a=$(echo "$out" | sed -nE 's/.* acked=([0-9]+) .*/\1/p')
      nz=$(echo "$out" | sed -nE 's/.*nonzero_exit=([0-9]+).*/\1/p')
      lost=$((lost + l)); acked=$((acked + a)); [ "$nz" != 0 ] && bad=$((bad + 1))
      panics=$((panics + $(cat "$D"/logs/out_*.txt | grep -c panicked)))
      mixrej=$((mixrej + $(cat "$D"/logs/out_*.txt | grep -c 'already open without experimental')))
    done
    echo "$v procs=$P rows=$R runs=$RUNS expected_total=$((RUNS*P*R)) acked_total=$acked runs_with_failed_proc=$bad panics=$panics spurious_mode_rejections=$mixrej acked_but_missing_total=$lost"
  done
done
