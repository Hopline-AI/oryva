# oryva

An SDK generator built for agents. Give it an OpenAPI spec and it writes Python and TypeScript
SDKs plus an MCP server, then measures how well agents can use the result.

Early stage: two languages so far, tested on a fictional fixture API and not yet on large
real-world specs. See `PLAN.md` for what's next.

## What you get

From one spec, `oryva generate` writes:

- **A Python SDK** (`httpx` + `pydantic`): sync and async clients, typed models and errors.
- **A TypeScript SDK**: zero runtime dependencies, runs on Node 18+, browsers and edge runtimes.
- **An MCP server** that wraps the Python SDK, with every tool marked read-only, destructive or
  idempotent, and agent hints from the spec in its descriptions.

Both SDKs follow one behaviour contract: API key, bearer, basic and OAuth2 client-credentials auth;
named environments; retries that honour `Retry-After`; automatic idempotency keys; cursor, offset
and page pagination with auto-paging; server-sent event streams; file uploads and downloads;
per-request options; raw responses; omitted vs `null`; enums and unions that tolerate values the
SDK doesn't know yet; webhook signature verification; and redacted debug logging.

## Why

- **No drift.** The SDKs and the MCP server come from one model of the API, so tool names,
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
oryva generate spec.yaml --out out/myapi   # out/myapi/python, out/myapi/mcp, out/myapi/typescript
oryva eval tasks.yaml --lint-only          # lint an MCP server's tools; no API key needed
oryva eval tasks.yaml --json report.json   # also run the tasks; needs ANTHROPIC_API_KEY
```

A task file names the MCP server to start and the tasks to run; see `fixtures/evals/ledger.yaml`.
Specs can carry optional `x-oryva-*` extensions for pagination, idempotency, webhook signing,
environment names and agent hints; `CLAUDE.md` lists them.

## Config, regeneration and CI

`oryva generate` with no arguments reads `./oryva.yml` (`--config <path>` reads another). Paths
in it are relative to the file; unknown keys are errors.

```yaml
spec: openapi.yaml
name: ledger              # optional; overrides x-oryva-name
version: 1.4.0            # optional SDK version; default comes from the spec
targets:                  # optional; default: all three under out/<name>/
  python: { out: sdks/python, package: ledger, dist: ledger-sdk }
  mcp: { out: sdks/mcp }
  typescript: { out: sdks/typescript, package: "@acme/ledger" }
```

A target you leave out is skipped. Each output directory gets a `.oryva/manifest.json` listing
the files oryva wrote and their hashes, and `.oryva/ir.json`, the API model used. Regeneration:

- deletes generated files the new output no longer has, and any directory left empty;
- aborts, before writing anything, if you edited a generated file. Move custom code into files
  oryva doesn't generate, or rerun with `--force` to overwrite your edits;
- never touches a file the manifest doesn't list. If oryva needs that path, it aborts and names it.

Commit `.oryva/` with the SDK so the next run can report what changed.

`oryva diff <old> <new> [--version <current>] [--json]` compares two specs, or `ir.json` files,
and prints a markdown report of breaking, minor and patch changes. With `--version` it also
prints the next semver version (breaking bumps the major, or the minor while on 0.x). It exits 1
when there are breaking changes, so CI can gate on it:

```sh
oryva diff main-openapi.yaml openapi.yaml --version 1.4.0
```

## Development

- `scripts/verify.sh` is the definition of done: format, lints and tests.
- `scripts/smoke-python.sh` and `scripts/smoke-typescript.sh` run the whole path: spec to SDKs,
  then every behaviour against `scripts/stub_ledger.py`, type checks, and a lint of the MCP server.
- `scripts/live-eval.sh` runs Claude against the generated MCP server (costs a few cents).
- `CLAUDE.md` holds the decisions and conventions; `PLAN.md` holds the roadmap and its status.

No license has been chosen yet.
