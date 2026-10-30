# Plan

## Product principles

Copy from the generators that won (Stainless, Fern, Speakeasy):

- Output that reads as if a person wrote it.
- Sell maintenance, not generation: regenerate on every spec change, open a PR, and keep the
  customer's hand edits (three-way merge).

Be different:

- Quality is measured by agents, and every release ships the score.
- The agent surface is generated from the same model as the SDK, so it can't drift.
- Guardrails for APIs that change things: scopes, confirmation before destructive calls,
  spending limits.
- Door in: import `stainless.yml` and prove the public API didn't change.

## Phase 0 — agent tests

- [x] `oryva-eval`: connect to any MCP server over stdio, list its tools, lint them (no model needed)
- [x] Run task files against Claude; report pass rate, tokens, tool calls, tool errors, recoveries (tested with a scripted model; no live run yet)
- [x] Task file for `sarvam-mcp==0.2.13`

Done when: run against `sarvam-mcp==0.2.13` it flags the "Max 10 pages" description (wrong
since the limits were raised) and the retired `text_analytics` tool. Needs `SARVAM_API_KEY`
and `ANTHROPIC_API_KEY`.

## Phase 1 — spec → Python SDK + MCP server

- [x] `oryva-spec`: OpenAPI 3.0/3.1 → IR, snapshot-tested on `fixtures/specs`
- [x] `oryva-emit-python`: IR → Python SDK
- [x] `oryva-emit-python`: IR → MCP server wrapping the SDK
- [x] `oryva` CLI: `generate` and `eval`
- [x] End to end: `sarvam-lite.yaml` → SDK + MCP server → lint passes, smoke test passes
  (`scripts/smoke-python.sh`; also type-checks both packages with mypy)
- [ ] When a field has no description but its named type does, use the type's description in
  tool schemas and docstrings (the lint flags `target_language_code` today)

Done when: the generated SDK type-checks and passes tests against a local stub server, and the
generated MCP server scores at least as well as a hand-written one on the same tasks. The first
two hold; the comparison needs a live run with `ANTHROPIC_API_KEY`.

- [ ] Deriving a spec from an existing SDK, for APIs with no public spec (Sarvam publishes none).
  Models, enums, signatures and routes can be recovered; limits, error types and auth details
  may not be, so those get proposed and a person reviews them once.

## Phase 2 — the maintenance loop

- [ ] Generated code lives in `_gen/` and is never edited by hand. Everything outside `_gen/`
  is owned source that regeneration never touches. This replaces three-way merging; add
  `git merge-file` only if a customer must edit generated files.
- [ ] Regenerate on spec change
- [ ] Block breaking changes (`griffe check` for Python)
- [ ] GitHub Action that opens the regeneration PR

Done when: a spec change produces a clean PR and nothing outside `_gen/` changed.

## Phase 3 — TypeScript SDK + `stainless.yml` import

- [ ] Conformance suite: a stub API server, language-neutral check files under `conformance/`,
  and one small runner per target language, so every language is held to the same expected
  output. A change touches `conformance/` or implementation code, never both in one commit, so
  a test can't be weakened to make code pass; `scripts/verify.sh` enforces this once the
  directory exists.
- [ ] Differential check against the customer's existing SDK: send the same calls through the
  old SDK and the regenerated one against the stub server, and compare requests and results.
  The old SDK is the oracle.

Done when: one public ex-Stainless SDK regenerates with zero public API changes and the
differential check passes.

## Proof, for every phase

- Mutation check: inject bugs into generated code; the checks must catch at least 90%.
- Accuracy is measured on cases the builders never saw, and published.

## Known risks

- Nobody has shown they will pay yet. Talk to real API companies alongside the build.
- Customers' own coding agents are probably the biggest competitor.
- Plain generation is commoditising. The moat is measured proof and the edge cases collected
  over time, not the generator code.
- The hand-written layer (WebSockets, streaming, auth flows) is where spec generators stop.
  Sarvam's speech APIs need WebSockets, so streaming is first in line after Phase 2.

## Phase 4 — code mode, guardrails, hosted GitHub App

Start only once a design partner is live.
