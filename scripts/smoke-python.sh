#!/usr/bin/env bash
# Generates the Python SDK and MCP server from the sarvam-lite IR fixture, installs both into a
# fresh venv (needs the network for pip packages) and exercises them against a local stub server.
set -euo pipefail
cd "$(dirname "$0")/.."

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

ir=crates/oryva-emit-python/tests/fixtures/sarvam-lite.ir.json
cargo run -q -p oryva-emit-python --example generate -- "$ir" "$tmp/out"

uv venv -q --python 3.12 "$tmp/venv"
uv pip install -q --python "$tmp/venv/bin/python" "$tmp/out/sdk" "$tmp/out/mcp"
echo "installed: $(uv pip list --python "$tmp/venv/bin/python" 2>/dev/null | grep -E '^(sarvam|sarvam-mcp|mcp) ' | tr -s ' ' '=' | tr '\n' ' ')"

# -I: don't import anything from the current directory.
"$tmp/venv/bin/python" -I scripts/smoke_python.py "$tmp/venv/bin"
echo "smoke-python: ok"
