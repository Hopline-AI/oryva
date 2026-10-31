#!/usr/bin/env bash
# Live agent run against a generated MCP server. Generates the ledger SDK and MCP server from
# fixtures/specs/ledger.yaml, installs them, serves canned responses from scripts/stub_ledger.py,
# and runs fixtures/evals/ledger.yaml with Claude. Costs real tokens: needs ANTHROPIC_API_KEY, read
# from .env when present. Usage: scripts/live-eval.sh [report.json]
set -euo pipefail
cd "$(dirname "$0")/.."
[ -f .env ] && { set -a; . ./.env; set +a; }

tmp="$(mktemp -d)"
stub=""
trap 'kill "$stub" 2>/dev/null; wait "$stub" 2>/dev/null || true; rm -rf "$tmp"' EXIT

cargo build -q -p oryva
target/debug/oryva generate fixtures/specs/ledger.yaml --out "$tmp/out" >/dev/null
uv venv -q --python 3.12 "$tmp/venv"
uv pip install -q --python "$tmp/venv/bin/python" "$tmp/out/python" "$tmp/out/mcp"

python3 -I scripts/stub_ledger.py > "$tmp/port" &
stub=$!
for _ in $(seq 50); do [ -s "$tmp/port" ] && break; sleep 0.1; done

export ORYVA_VENV="$tmp/venv" LEDGER_BASE_URL="http://127.0.0.1:$(head -1 "$tmp/port")"
target/debug/oryva eval fixtures/evals/ledger.yaml --json "${1:-$tmp/report.json}"
