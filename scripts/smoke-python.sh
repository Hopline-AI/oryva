#!/usr/bin/env bash
# End to end: generates the Python SDK and MCP server from fixtures/specs/ledger.yaml with the oryva
# CLI, installs both into a fresh venv (needs the network for pip packages), runs
# scripts/smoke_python.py against scripts/stub_ledger.py, type-checks both packages with mypy, and
# lints the generated MCP server with oryva's own lint.
set -euo pipefail
cd "$(dirname "$0")/.."

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cargo build -q -p oryva
target/debug/oryva generate fixtures/specs/ledger.yaml --out "$tmp/out"

uv venv -q --python 3.12 "$tmp/venv"
uv pip install -q --python "$tmp/venv/bin/python" "$tmp/out/python" "$tmp/out/mcp" mypy
echo "installed: $(uv pip list --python "$tmp/venv/bin/python" 2>/dev/null | grep -E '^(ledger|ledger-mcp|mcp) ' | tr -s ' ' '=' | tr '\n' ' ')"

# -I: don't import anything from the current directory.
"$tmp/venv/bin/python" -I scripts/smoke_python.py "$tmp/venv/bin"

"$tmp/venv/bin/python" -m mypy -p ledger -p ledger_mcp
# A generated MCP server must pass oryva's own lint with zero errors.
cat > "$tmp/lint.yaml" <<EOF
server:
  command: $tmp/venv/bin/ledger-mcp
  env:
    LEDGER_API_KEY: test-key
tasks: []
EOF
target/debug/oryva eval "$tmp/lint.yaml" --lint-only
echo "smoke-python: ok"
