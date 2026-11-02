#!/usr/bin/env bash
# Stress test on large public specs. Downloads each spec once into target/real-specs/, runs
# `oryva generate`, then per spec: mypy on the Python SDK and MCP server, tsc build of the
# TypeScript SDK, a runtime import of all three (catches circular imports and unresolved forward
# references that type checkers miss), and `oryva eval --lint-only` against the generated MCP server
# with dummy credentials. Needs the network on the first run. Specs are untrusted: they are parsed
# and type-checked, and the generated code is imported, never run against anything.
# Prints one row per spec and exits 1 if any step failed.
# Usage: scripts/real-specs.sh [name...]
set -uo pipefail
cd "$(dirname "$0")/.."

specs=(
  "stripe https://raw.githubusercontent.com/stripe/openapi/master/openapi/spec3.yaml"
  "github https://raw.githubusercontent.com/github/rest-api-description/main/descriptions/api.github.com/api.github.com.yaml"
  "openai https://raw.githubusercontent.com/openai/openai-openapi/master/openapi.yaml"
  "twilio https://raw.githubusercontent.com/twilio/twilio-oai/main/spec/yaml/twilio_api_v2010.yaml"
  "asana https://raw.githubusercontent.com/Asana/openapi/master/defs/asana_oas.yaml"
  "spotify https://raw.githubusercontent.com/sonallux/spotify-web-api/main/fixed-spotify-open-api.yml"
  "box https://raw.githubusercontent.com/box/box-openapi/main/openapi.json"
  "discord https://raw.githubusercontent.com/discord/discord-api-spec/main/specs/openapi.json"
  "vercel https://openapi.vercel.sh/ json"
  "pagerduty https://raw.githubusercontent.com/PagerDuty/api-schema/main/reference/REST/openapiv3.json"
  "train https://raw.githubusercontent.com/bump-sh-examples/train-travel-api/main/openapi.yaml"
)
dir=target/real-specs
mkdir -p "$dir"
cargo build -q -p oryva
venv=$dir/venv
[ -x "$venv/bin/mypy" ] || { uv venv -q --python 3.12 "$venv" && uv pip install -q --python "$venv/bin/python" httpx pydantic mypy; }
tsc=(npx --yes -p typescript@7.0.2 tsc)

failed=0
printf '%-9s %5s  %-8s %-6s %-6s %-6s %-9s %6s %8s\n' spec secs generate mypy tsc import lint-errs tools ~tokens
for entry in "${specs[@]}"; do
  read -r name url ext <<<"$entry"
  [ $# -gt 0 ] && [[ " $* " != *" $name "* ]] && continue
  spec="$dir/$name.${ext:-${url##*.}}"
  [ -s "$spec" ] || curl -sfL "$url" -o "$spec" || { printf '%-9s download failed\n' "$name"; failed=1; continue; }

  start=$SECONDS
  out="$dir/out/$name"
  rm -rf "$out"
  if ! gen=$(target/debug/oryva generate "$spec" --out "$out" 2>&1); then
    printf '%-9s %5s  %s\n' "$name" $((SECONDS - start)) "FAIL ${gen#error: }" | cut -c1-200
    failed=1
    continue
  fi
  pkg=$(ls "$out/python/src")
  py=$(MYPYPATH="$out/python/src:$out/mcp/src" "$venv/bin/mypy" -p "$pkg" -p "${pkg}_mcp" --no-incremental 2>&1 | tail -1)
  [[ $py == Success* ]] && py=ok || { py=FAIL; failed=1; }
  ts_errors=$(cd "$out/typescript" && "${tsc[@]}" -p . 2>&1 | grep -c 'error TS')
  [ "$ts_errors" -eq 0 ] && ts=ok || { ts="${ts_errors}err"; failed=1; }

  # Runtime import: installed packages for Python, the built dist for TypeScript.
  sdk_dist=$(sed -n 's/^name = "\(.*\)"/\1/p' "$out/python/pyproject.toml" | head -1)
  mcp_dist=$(sed -n 's/^name = "\(.*\)"/\1/p' "$out/mcp/pyproject.toml" | head -1)
  uv pip install -q --python "$venv/bin/python" --reinstall-package "$sdk_dist" --reinstall-package "$mcp_dist" "$out/python" "$out/mcp" >"$out/install.log" 2>&1
  if "$venv/bin/python" -I -c "import $pkg, ${pkg}_mcp" >"$out/import.log" 2>&1 \
    && node --input-type=module -e 'await import(process.argv[1])' "$PWD/$out/typescript/dist/index.js" >>"$out/import.log" 2>&1; then
    imp=ok
  else
    imp=FAIL; failed=1
  fi

  # MCP lint: dummy values for every credential variable the SDK reads.
  {
    echo "server:"
    echo "  command: $PWD/$venv/bin/$mcp_dist"
    echo "  env:"
    grep -rhoE 'environ\.get\("[A-Z0-9_]+"' "$out/python/src" | sed 's/.*("//; s/"//' | sort -u \
      | grep -vE '_(BASE_URL|LOG|OAUTH_TOKEN_URL)$' | sed 's/.*/    &: dummy/'
    echo "tasks: []"
  } >"$out/lint.yaml"
  lint="$(target/debug/oryva eval "$out/lint.yaml" --lint-only --json "$out/lint.json" 2>&1 | tail -3)"
  read -r lint_errs tools tokens < <("$venv/bin/python" -I -c '
import json, sys
l = json.load(open(sys.argv[1]))["lint"]
print(sum(f["severity"] == "error" for f in l["findings"]), l["tool_count"], l["approx_tokens"])' "$out/lint.json" 2>/dev/null || echo "FAIL - -")
  [ "$lint_errs" = 0 ] || { failed=1; echo "$lint" | tail -3 >&2; }
  printf '%-9s %5s  %-8s %-6s %-6s %-6s %-9s %6s %8s\n' "$name" $((SECONDS - start)) ok "$py" "$ts" "$imp" "$lint_errs" "$tools" "$tokens"
done
exit $failed
