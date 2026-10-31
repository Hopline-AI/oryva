#!/usr/bin/env bash
# End to end: generates the Python SDK and MCP server from the ledger IR fixture, installs both
# into a fresh venv (needs the network for pip packages), runs scripts/smoke_python.py against
# scripts/stub_ledger.py, and type-checks both packages with mypy.
# The `oryva` CLI is not used until the spec parser lands; the main agent re-adds the CLI lint
# step (`oryva eval --lint-only` on the generated server) at merge.
set -euo pipefail
cd "$(dirname "$0")/.."

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cargo run -q -p oryva-emit-python --example generate -- \
  crates/oryva-emit-python/tests/fixtures/ledger.ir.json "$tmp/out"

uv venv -q --python 3.12 "$tmp/venv"
uv pip install -q --python "$tmp/venv/bin/python" "$tmp/out/sdk" "$tmp/out/mcp" mypy
echo "installed: $(uv pip list --python "$tmp/venv/bin/python" 2>/dev/null | grep -E '^(ledger|ledger-mcp|mcp) ' | tr -s ' ' '=' | tr '\n' ' ')"

# -I: don't import anything from the current directory.
"$tmp/venv/bin/python" -I scripts/smoke_python.py "$tmp/venv/bin"

"$tmp/venv/bin/python" -m mypy -p ledger -p ledger_mcp
echo "smoke-python: ok"
