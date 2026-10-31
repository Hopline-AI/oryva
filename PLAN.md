# Plan

## Product principles

Match what Stainless, Speakeasy and Fern ship:

- Output that reads as if a person wrote it, in every language a customer asks for.
- Sell maintenance, not generation: regenerate on every spec change, open a PR, keep the
  customer's own code, and block breaking changes.

Then go further:

- Agents score every release: lint for agent-facing problems, live task runs, published numbers.
- The agent surface is generated from the same model as the SDKs, so it can't drift.
- Guardrails for APIs that change things: scopes, confirmation before destructive calls,
  spending limits.
- A way in for teams leaving Stainless: import `stainless.yml` and prove the public API didn't
  change.

## Feature parity

What the leading generators ship, and where oryva stands. "Leaders" means at least two of
Stainless, Speakeasy and Fern.

| Feature | Leaders | oryva | Wave |
|---|---|---|---|
| OpenAPI 3.0 / 3.1 input | yes | done | — |
| Python SDK | yes | done (Python, TypeScript) | 1 |
| TypeScript SDK | yes | done (Python, TypeScript) | 1 |
| Go, Java, Kotlin, C#, PHP, Ruby | yes | — | 4 |
| Typed models, typed errors | yes | done (Python, TypeScript) | 1 |
| Retries with backoff, `Retry-After` | yes | done (Python, TypeScript) | 1 |
| Pagination with auto-paging | yes | done (Python, TypeScript) | 1 |
| Server-sent event streaming | yes | done (Python, TypeScript) | 1 |
| Async client (Python) | yes | done (Python, TypeScript) | 1 |
| Auth: API key, bearer, basic, OAuth2 client credentials | yes | done (Python, TypeScript) | 1 |
| Environments (multiple servers) | yes | done (Python, TypeScript) | 1 |
| Idempotency keys | Stainless | done (Python, TypeScript) | 1 |
| Per-request options, raw responses | yes | done (Python, TypeScript) | 1 |
| Forward-compatible enums and unions | Speakeasy | done (Python, TypeScript) | 1 |
| Omitted vs null | Stainless | done (Python, TypeScript) | 1 |
| Webhook verification and parsing | Stainless, Fern | done (Python, TypeScript) | 1 |
| File upload and download | yes | done (Python, TypeScript) | 1 |
| MCP server | Stainless, Speakeasy | done (Python) | 2 |
| Generated tests against a mock server | Speakeasy, APIMatic | — | 2 |
| SDK READMEs with an example per method | yes | basic | 2 |
| API reference docs, `llms.txt` | Fern | — | 2 |
| Keep custom code across regenerations | yes | — | 3 |
| Breaking-change detection | Speakeasy | — | 3 |
| Versioning, changelog, publishing to PyPI / npm | yes | — | 3 |
| GitHub integration: regeneration PRs | yes | — | 3 |
| Config file: rename, hide, group endpoints; spec overlays | yes | x-oryva extensions | 5 |
| CLI target | Stainless | — | 5 |
| Terraform provider | Speakeasy | — | 5 |
| WebSockets | partial | — | 5 |
| Agent lint of any MCP server | — | done | — |
| Live agent scoring | — | done | — |
| Destructive / read-only / idempotent tool flags | partial | done | — |

## Done

- `oryva-eval`: lint any MCP server; run task files with Claude; report pass rate, tokens
  (cache traffic included), tool errors, recoveries. Live runs work (`scripts/live-eval.sh`).
- `oryva-spec`: OpenAPI 3.0/3.1 → IR v1, snapshot-tested.
- `oryva-emit-python`: Python SDK and MCP server from IR v1; smoke test installs both, calls them
  against a stub API, type-checks them with mypy and lints the server.
- `oryva` CLI: `generate` and `eval`.

## Wave 1 — the SDK features every leader has (done)

- [x] IR v2: environments, auth schemes, idempotency header, pagination, event streams, binary
  bodies, defaults, integer enums, discriminators, webhooks
- [x] `fixtures/specs/ledger.yaml`, a neutral fixture that exercises all of it
- [x] `oryva-spec` produces IR v2 from `ledger.yaml`
- [x] Python SDK implements the behaviour contract in `CLAUDE.md`, sync and async
- [x] TypeScript SDK implements the behaviour contract
- [x] Smoke tests for both languages run against a `ledger` stub; the live eval moves to `ledger`

Done when: both SDKs pass end-to-end checks for every row of the behaviour contract against the
stub, type-check (`mypy`, `tsc`), and the generated MCP server still lints clean.

Result: from `ledger.yaml` through the CLI, Python passes 140 checks and mypy, TypeScript passes 59
tests and `tsc`, the MCP server lints with 0 errors on 18 tools, and Claude passes 4/4 live tasks
on it (paging, a checked refund, an event stream) at about 13k tokens per task.

## Next — prove it on real specs

- [x] Run `oryva generate` plus `mypy` and `tsc` on large public specs
  (`scripts/real-specs.sh`: Stripe, GitHub, OpenAI, Twilio, Asana, Spotify, Box). The first run
  generated none of them; 12 root causes later all 7 generate and type-check in both languages,
  Stripe in 8 s and GitHub in 11 s. Regression cases live in `fixtures/specs/edge-cases.yaml`.
- [x] IR: keep a binary body's content type (IR v3)
- [ ] IR: a literal type for `const`, so discriminators don't depend on single-value enums
- [ ] Widen the stress test: more specs (OpenAPI 3.1 ones, Swagger 2.0 via conversion, external
  `$ref`s via bundling), and run the generated SDKs' smoke checks, not only type checks

## Wave 2 — tests, docs and the agent surface

- [ ] Generated test suite in each SDK, run against a mock built from the spec's examples
- [ ] README per SDK with a working example for every method; API reference; `llms.txt`
- [ ] MCP server aware of pagination and streaming; code mode (search docs + run code against
  the SDK) for large APIs
- [ ] When a field has no description but its named type does, use the type's description
- [ ] Live eval of the generated MCP server against `ledger` in CI

## Wave 3 — the maintenance loop

- [ ] Generated code in `_gen/`, never edited by hand; everything else is owned code that
  regeneration never touches
- [ ] Breaking-change gate (`griffe check` for Python, `api-extractor` for TypeScript)
- [ ] Version bumps and changelogs from the spec diff
- [ ] Publishing workflows for PyPI and npm
- [ ] GitHub Action that opens the regeneration PR

## Wave 4 — more languages

Go, then Java and Kotlin, then C#, PHP and Ruby, each against the same behaviour contract and
the same conformance checks. Starts after wave 3: two languages kept in sync sell better than five
that aren't.

## Wave 5 — configuration and reach

- [ ] `oryva.yml`: rename, hide and group endpoints; OpenAPI overlays to patch weak specs
- [ ] `stainless.yml` import with a public-API diff against the old SDK
- [ ] Derive a spec from an existing SDK when there is none
- [ ] CLI and Terraform targets; WebSockets

## Proof, for every wave

- Mutation check: inject bugs into generated code; the checks must catch at least 90%.
- Conformance: the same language-neutral checks run against every SDK.

## Known risks

- Nobody has shown they will pay yet. Talk to API companies alongside the build.
- Customers' own coding agents are probably the biggest competitor.
- Plain generation is commoditising. The moat is measured proof and the edge cases collected
  over time, not the generator code.

## Outreach

Material for specific companies lives under `examples/`.

- `examples/sarvam`: an approximate spec and a task file for their hand-written `sarvam-mcp`.
  A live comparison needs `SARVAM_API_KEY`.
- Teams leaving Stainless: wave 5's import is the way in.
