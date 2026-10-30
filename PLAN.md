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

- [ ] `oryva-eval`: connect to any MCP server over stdio, list its tools, lint them (no model needed)
- [ ] Run task files against Claude; report pass rate, tokens, tool calls, tool errors, recoveries
- [ ] Task file for `sarvam-mcp==0.2.13`

Done when: run against `sarvam-mcp==0.2.13` it flags the "Max 10 pages" description (wrong
since the limits were raised) and the retired `text_analytics` tool. Needs `SARVAM_API_KEY`
and `ANTHROPIC_API_KEY`.

## Phase 1 — spec → Python SDK + MCP server

- [ ] `oryva-spec`: OpenAPI 3.0/3.1 → IR, snapshot-tested on `fixtures/specs`
- [ ] `oryva-emit-python`: IR → Python SDK
- [ ] `oryva-emit-python`: IR → MCP server wrapping the SDK
- [ ] `oryva` CLI: `generate` and `eval`
- [ ] End to end: `sarvam-lite.yaml` → SDK + MCP server → lint passes, smoke test passes

Done when: the generated SDK type-checks and passes tests against a local stub server, and the
generated MCP server scores at least as well as a hand-written one on the same tasks.

## Phase 2 — the maintenance loop

- [ ] Regenerate on spec change and re-apply hand edits (`git merge-file`)
- [ ] Block breaking changes (`griffe check` for Python)
- [ ] GitHub Action that opens the regeneration PR

Done when: a spec change produces a clean PR with hand edits intact.

## Phase 3 — TypeScript SDK + `stainless.yml` import

Done when: one public ex-Stainless SDK regenerates with zero public API changes.

## Phase 4 — code mode, guardrails, hosted GitHub App

Start only once a design partner is live.
