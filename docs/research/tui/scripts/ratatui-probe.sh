#!/usr/bin/env bash
# Builds tm with ratatui linked into one minimal frame, in a copy of HEAD, so
# binary size, cold start and cargo-deny can be compared with the real build.
# Usage: ratatui-probe.sh SCRATCH_DIR   (run from the repository root)
set -euo pipefail

dir=${1:?scratch directory}
rm -rf "${dir:?}/tuiprobe"
mkdir -p "$dir/tuiprobe"
git archive HEAD | tar -x -C "$dir/tuiprobe"
cd "$dir/tuiprobe"
# The copy is not a trusted mise project; use rustup's cargo directly.
rm -f .mise.toml
python3 -I - crates/tm-cli/Cargo.toml crates/tm-cli/src/main.rs <<'EOF'
import sys
manifest, main = sys.argv[1], sys.argv[2]
text = open(manifest).read().replace("[dependencies]\n", '[dependencies]\nratatui = "0.30.2"\n', 1)
open(manifest, "w").write(text)
probe = (
    "    if std::env::var_os(\"TM_TUI_PROBE\").is_some() {\n"
    "        let _ = ratatui::run(|t| t.draw(|f| f.render_widget("
    "ratatui::widgets::Paragraph::new(\"probe\").block(ratatui::widgets::Block::bordered()), "
    "f.area())).map(|_| ()));\n"
    "        return ExitCode::SUCCESS;\n"
    "    }\n"
)
text = open(main).read().replace("fn main() -> ExitCode {\n", "fn main() -> ExitCode {\n" + probe, 1)
open(main, "w").write(text)
EOF
PATH=$HOME/.cargo/bin:$PATH cargo build --release -p tm-cli
ls -l target/release/tm
echo "cargo-deny: run \"\$(mise which cargo-deny)\" --workspace check all here with rustup's cargo first on PATH"
