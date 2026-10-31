#!/usr/bin/env bash
# Stress test on large public specs. Downloads each spec once into target/real-specs/, runs
# `oryva generate`, then type-checks the Python SDK with mypy and the TypeScript SDK with tsc.
# Needs the network on the first run. Prints one row per spec and exits 1 if any step failed.
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
)
dir=target/real-specs
mkdir -p "$dir"
cargo build -q -p oryva
venv=$dir/venv
[ -x "$venv/bin/mypy" ] || { uv venv -q --python 3.12 "$venv" && uv pip install -q --python "$venv/bin/python" httpx pydantic mypy; }
tsc=(npx --yes -p typescript@7.0.2 tsc)

failed=0
printf '%-8s %6s  %-40s %-28s %s\n' spec secs generate python typescript
for entry in "${specs[@]}"; do
  read -r name url <<<"$entry"
  [ $# -gt 0 ] && [[ " $* " != *" $name "* ]] && continue
  spec="$dir/$name.${url##*.}"
  [ -s "$spec" ] || curl -sfL "$url" -o "$spec" || { printf '%-8s download failed\n' "$name"; failed=1; continue; }

  start=$SECONDS
  out="$dir/out/$name"
  if ! gen=$(target/debug/oryva generate "$spec" --out "$out" 2>&1); then
    printf '%-8s %6s  %-40.40s\n' "$name" $((SECONDS - start)) "FAIL ${gen#error: }"
    failed=1
    continue
  fi
  pkg=$(ls "$out/python/src")
  py=$(MYPYPATH="$out/python/src" "$venv/bin/mypy" -p "$pkg" --no-incremental 2>&1 | tail -1)
  [[ $py == Success* ]] && py="ok" || failed=1
  ts_errors=$(cd "$out/typescript" && "${tsc[@]}" --noEmit -p . 2>&1 | grep -c 'error TS')
  [ "$ts_errors" -eq 0 ] && ts="ok" || { ts="$ts_errors errors"; failed=1; }
  printf '%-8s %6s  %-40s %-28.28s %s\n' "$name" $((SECONDS - start)) ok "$py" "$ts"
done
exit $failed
