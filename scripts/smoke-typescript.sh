#!/usr/bin/env bash
# End to end: generates the TypeScript SDK from the hand-written Ledger IR (the spec parser isn't
# involved), builds it with tsc (needs the network for npx), then runs scripts/smoke_typescript.mjs
# against the shared stub, scripts/stub_ledger.py.
set -euo pipefail
cd "$(dirname "$0")/.."

TYPESCRIPT=typescript@7.0.2
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cargo run -q -p oryva-emit-typescript --example generate -- \
  crates/oryva-emit-typescript/tests/fixtures/ledger.ir.json "$tmp/sdk"

(cd "$tmp/sdk" && npx --yes -p "$TYPESCRIPT" tsc)

SDK_DIR="$tmp/sdk" node --test scripts/smoke_typescript.mjs
echo "smoke-typescript: ok"
