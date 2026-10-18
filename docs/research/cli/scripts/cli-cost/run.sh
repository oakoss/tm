#!/usr/bin/env bash
# Builds cli-cost and the tm binary into a scratch target directory, then
# measures what core-commands.md reports: process start-up of `tm`, and list
# and ready costs at 1,000 and 10,000 Issues.
# Usage: run.sh <repo> <scratch-dir>
set -euo pipefail
repo=$1
scratch=$2
here=$(cd "$(dirname "$0")" && pwd)
export CARGO_TARGET_DIR="$scratch/target"
cargo build -q --release --manifest-path "$here/Cargo.toml"
cargo build -q --release --manifest-path "$repo/Cargo.toml" -p tm-cli
bin="$scratch/target/release/cli-cost"
tm="$scratch/target/release/tm"

# Wall time of 50 runs of a command, in ms per run (macOS bash 3.2 has no
# EPOCHREALTIME, so python times the loop).
per_run() {
  python3 -I -c '
import subprocess, sys, time
cmd = sys.argv[1:]
subprocess.run(cmd, stdout=subprocess.DEVNULL, check=True)
t = time.perf_counter()
for _ in range(50):
    subprocess.run(cmd, stdout=subprocess.DEVNULL, check=True)
args = " ".join(cmd[1:])
print(f"{(time.perf_counter() - t) * 1000 / 50:.2f} ms per run: {args}")
' "$@"
}

per_run "$tm" --version
per_run "$tm" scheme show
per_run "$tm" scheme show --json

for n in 1000 10000; do
  dir="$scratch/store-$n"
  rm -rf "$dir" && mkdir -p "$dir"
  "$bin" seed "$dir" "$n"
  for _ in 1 2 3; do "$bin" list "$dir" >/dev/null; done
  "$bin" open "$dir"
  "$bin" ready "$dir"
done

# These write, so they run last, on the 1,000-Issue store.
"$bin" errors "$scratch/store-1000"
"$bin" undo "$scratch/store-1000"
