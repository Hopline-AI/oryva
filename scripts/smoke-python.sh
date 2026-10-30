#!/usr/bin/env bash
# End to end: generates the Python SDK and MCP server from fixtures/specs/sarvam-lite.yaml with the
# oryva CLI, installs both into a fresh venv (needs the network for pip packages), exercises them
# against a local stub server, then lints the generated MCP server with oryva's own lint.
set -euo pipefail
cd "$(dirname "$0")/.."

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cargo build -q -p oryva
oryva=target/debug/oryva
"$oryva" generate fixtures/specs/sarvam-lite.yaml --out "$tmp/out"

uv venv -q --python 3.12 "$tmp/venv"
uv pip install -q --python "$tmp/venv/bin/python" "$tmp/out/sdk" "$tmp/out/mcp"
echo "installed: $(uv pip list --python "$tmp/venv/bin/python" 2>/dev/null | grep -E '^(sarvam|sarvam-mcp|mcp) ' | tr -s ' ' '=' | tr '\n' ' ')"

# -I: don't import anything from the current directory.
"$tmp/venv/bin/python" -I scripts/smoke_python.py "$tmp/venv/bin"

uv pip install -q --python "$tmp/venv/bin/python" mypy
"$tmp/venv/bin/python" -m mypy -p sarvam -p sarvam_mcp

# A generated MCP server must pass oryva's own lint with zero errors.
cat > "$tmp/lint.yaml" <<EOF
server:
  command: $tmp/venv/bin/sarvam-mcp
  env:
    SARVAM_API_KEY: dummy
tasks: []
EOF
"$oryva" eval "$tmp/lint.yaml" --lint-only
echo "smoke-python: ok"
