#!/usr/bin/env bash
# Set up a clean clone: the locked Node toolchain Nx runs on, the pinned Rust toolchain
# and the locked crates, and the cargo test tools the gate runs. Idempotent and quiet.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || { echo "bootstrap: cannot enter the repository root; run scripts/bootstrap.sh from a checkout." >&2; exit 1; }

run() {
  local log
  if ! log="$("$@" 2>&1)"; then
    printf '%s\n' "$log" >&2
    echo "bootstrap: '$*' failed (above); fix it, then re-run 'just bootstrap'." >&2
    exit 1
  fi
}

for tool in cargo bun; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "bootstrap: $tool is not installed; install it (cargo: https://rustup.rs, bun: https://bun.sh), then re-run 'just bootstrap'." >&2
    exit 1
  }
done
run bun install --frozen-lockfile
# With no argument, installs the toolchain rust-toolchain.toml pins.
if command -v rustup >/dev/null 2>&1; then
  run rustup toolchain install
fi
run cargo fetch --locked
for tool in cargo-llvm-cov cargo-nextest; do
  command -v "$tool" >/dev/null 2>&1 || run cargo install --locked "$tool"
done
