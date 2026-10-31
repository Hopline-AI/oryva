#!/usr/bin/env bash
# End to end: generates the TypeScript SDK from fixtures/specs/ledger.yaml with the oryva CLI,
# builds it with tsc (needs the network for npx), then type-checks scripts/smoke_typescript_usage.ts
# and runs scripts/smoke_typescript.mjs against the shared stub, scripts/stub_ledger.py.
set -euo pipefail
cd "$(dirname "$0")/.."

TYPESCRIPT=typescript@7.0.2
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cargo build -q -p oryva
target/debug/oryva generate fixtures/specs/ledger.yaml --out "$tmp/out" >/dev/null
mv "$tmp/out/typescript" "$tmp/sdk"

(cd "$tmp/sdk" && npx --yes -p "$TYPESCRIPT" tsc)

# What a user's editor sees: wrong calls must be errors, right ones must compile.
mkdir "$tmp/sdk/usage"
cp scripts/smoke_typescript_usage.ts "$tmp/sdk/usage/usage.ts"
echo '{"extends": "./tsconfig.json", "compilerOptions": {"noEmit": true, "rootDir": "."}, "include": ["src", "usage"]}' \
  > "$tmp/sdk/tsconfig.usage.json"
(cd "$tmp/sdk" && npx --yes -p "$TYPESCRIPT" tsc -p tsconfig.usage.json)

SDK_DIR="$tmp/sdk" node --test scripts/smoke_typescript.mjs
echo "smoke-typescript: ok"
