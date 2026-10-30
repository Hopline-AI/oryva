# oryva

An SDK generator built for agents. Give it an OpenAPI spec and it writes a Python SDK plus an
MCP server that wraps it, then measures how well agents can use the result.

Early stage: Python is the only target so far.

## Why

- **No drift.** The SDK and the MCP server come from one model of the API, so tool names,
  descriptions and limits can't fall out of step with the API the way hand-written MCP servers do.
- **Measured, not claimed.** `oryva eval` lints any MCP server for agent-facing problems (missing
  descriptions, destructive tools not marked destructive, retired tools still exposed, context
  cost), then runs real tasks with Claude and reports pass rate, tokens, tool errors and recoveries.

## Install

Needs Rust 1.90 or later, and [ruff](https://docs.astral.sh/ruff/) on `PATH` for `generate`.

```sh
cargo install --path crates/oryva
```

## Use

```sh
oryva generate spec.yaml --out out/myapi   # writes out/myapi/sdk and out/myapi/mcp
oryva eval tasks.yaml --lint-only          # lint an MCP server's tools; no API key needed
oryva eval tasks.yaml --json report.json   # also run the tasks; needs ANTHROPIC_API_KEY
```

A task file names the MCP server to start and the tasks to run; see
`fixtures/evals/sarvam-mcp.yaml`. Specs can carry agent-facing notes in an `x-oryva-agent`
extension on each operation (`read_only`, `destructive`, `idempotent`, `hint`, `examples`).

## Development

- `scripts/verify.sh` is the definition of done: format, lints and tests.
- `scripts/smoke-python.sh` runs the whole path: spec to SDK and MCP server, installs both, calls
  them against a stub API, type-checks them and lints the server.
- `CLAUDE.md` holds the decisions and conventions; `PLAN.md` holds the phases and their status.

No license has been chosen yet.
