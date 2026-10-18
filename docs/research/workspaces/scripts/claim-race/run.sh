#!/usr/bin/env bash
# Builds claim-race into a scratch target directory and runs every variant
# reported in claims-and-links.md: 50 processes, 20 runs each.
# Usage: run.sh <scratch-dir>
set -euo pipefail
scratch=$1
here=$(cd "$(dirname "$0")" && pwd)
CARGO_TARGET_DIR="$scratch/target" cargo build --release --manifest-path "$here/Cargo.toml"
bin="$scratch/target/release/claim-race"
mkdir -p "$scratch/race"

# mode rule agents issues gap_us
while read -r mode rule agents issues gap; do
  "$bin" "$scratch/race" "$mode" "$rule" "$agents" 50 "$issues" 20 "$gap" | tail -1
done <<'EOF'
autocommit strict distinct 1 0
autocommit strict distinct 1 1000
deferred strict distinct 1 0
immediate strict distinct 1 0
immediate strict distinct 1 1000
conditional strict distinct 1 0
immediate strict same 1 0
immediate idempotent same 1 0
next-autocommit strict distinct 10 0
next-immediate strict distinct 10 0
EOF
