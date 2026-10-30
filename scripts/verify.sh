#!/usr/bin/env bash
# The definition of done. Usage: scripts/verify.sh [-p <crate>]...  (no args = whole workspace)
set -euo pipefail
cd "$(dirname "$0")/.."

scope=("$@")
[ ${#scope[@]} -eq 0 ] && scope=(--workspace)

cargo fmt "${scope[@]/--workspace/--all}" -- --check
cargo clippy "${scope[@]}" --all-targets -- -D warnings
cargo test "${scope[@]}"
echo "verify: ok (${scope[*]})"
