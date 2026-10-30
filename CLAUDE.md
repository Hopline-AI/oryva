# oryva

An SDK generator built for agents. One OpenAPI spec goes in; SDKs, an MCP server and an agent
test report come out. Two things set it apart:

- The agent surface (MCP server, tool descriptions) and the SDK come from the same model of
  the API, so they can't drift apart.
- Every release is scored by agents: pass rate, tokens per task, tool errors, recoveries.

Phases, status and "done when" criteria live in `PLAN.md`. Update the checkboxes there when you
finish something.

## Decisions already made — don't re-ask

- Rust (edition 2024, rustc 1.90) for the generator. A single static binary is the
  distribution story.
- Pipeline: spec → `oryva_ir::Api` → emitters. Emitters never read the spec.
- Spec parsing walks a `serde_json::Value` (YAML via `serde-saphyr`), so OpenAPI 3.0 and 3.1
  share one code path. Local `$ref`s only.
- First targets: a Python SDK (`httpx` + `pydantic` v2, sync client only) and a Python MCP
  server (the official `mcp` package) that wraps the generated SDK. TypeScript SDK in Phase 3.
- Templates: `minijinja`. Python output is formatted and linted with `ruff`.
- Agent tests call the Anthropic Messages API over raw HTTP (there is no official Rust SDK):
  `POST https://api.anthropic.com/v1/messages`, header `anthropic-version: 2026-10-21`,
  default model `claude-opus-5-5`, `fallbacks: "default"` with beta header
  `server-side-fallback-2026-07-01`. Never send `tool_choice` `any`/`tool`, `temperature`, or a
  thinking budget; those return 400 on current models.
- Agent-facing metadata comes from the `x-oryva-agent` operation extension (`read_only`,
  `destructive`, `idempotent`, `hint`, `examples`). Names can be overridden per operation with
  `x-oryva-resource` / `x-oryva-method`, and the package name with `info.x-oryva-name`.
- No license chosen yet; don't add one.
- Deferred until a real spec needs them: pagination, streaming/WebSockets, async Python client,
  nested resources, idempotency keys, code mode.

## Naming rules (the contract between parser and emitters)

- Resource: the operation's first tag in snake_case; with no tag, the first static path segment.
  `x-oryva-resource` overrides.
- Method: `operationId` in snake_case with a leading `<resource>_` stripped
  (`text_identify_language` under tag `text` → `identify_language`). With no `operationId`:
  GET on a path ending in a parameter → `get`, other GET → `list`, POST → `create`,
  PUT/PATCH → `update`, DELETE → `delete`. `x-oryva-method` overrides. Two operations mapping
  to the same resource+method is a parse error naming both.
- Hoisted inline objects: request bodies → `<Resource><Method>Request`, 2xx responses →
  `<Resource><Method>Response`, nested inline objects → `<Parent><Field>` (all PascalCase).
- Agent defaults: GET → `read_only` + `idempotent`; PUT → `idempotent`; DELETE → `destructive` +
  `idempotent`; POST/PATCH → none. `x-oryva-agent` keys override individually.
- Auth env var: `<NAME>_API_KEY` in upper snake case (`SARVAM_API_KEY`).
- Python: client class is the PascalCase API name (`Sarvam`), resources are attributes
  (`client.text.translate(...)`), all method arguments are keyword-only, body object fields
  are flattened into arguments, Python keywords get a trailing underscore (`from_`).
- MCP tool names: `<resource>_<method>` (`text_translate`).

## Layout

| Path | What | Owner |
|---|---|---|
| `crates/oryva-ir` | The API model | Main agent only. Changes need an `IR_VERSION` bump. |
| `crates/oryva-spec` | OpenAPI 3.0/3.1 → IR | |
| `crates/oryva-emit-python` | IR → Python SDK + MCP server | |
| `crates/oryva-eval` | Agent tests against any MCP server | |
| `crates/oryva` | CLI: `oryva generate`, `oryva eval` | |
| `fixtures/specs` | Input specs. `sarvam-lite.yaml` is a hand-written approximation, not Sarvam's spec. | |
| `fixtures/evals` | Agent-test task files | |

## Testing

Tests prove the end goal, not the code that happened to be written.

- Start from what a caller sees: an SDK user, an agent, a CLI user. Take expected values from
  the spec or the original SDK, never from the code under test.
- Every feature gets at least one end-to-end check through its real entry point: the CLI, a
  generated client against a stub server, a real MCP session.
- Mock only external boundaries (the network, the model API). Never mock the thing under test.
- Watch a new test fail before trusting it: break the code on purpose and confirm the test
  catches it, then restore.
- Delete tests that restate the code, only check that something exists, or pin formatting
  nobody relies on.

## Working rules

- `scripts/verify.sh` is the definition of done: it must exit 0 before you report anything as
  finished. While iterating, scope it: `scripts/verify.sh -p <crate>`.
- When a STOP condition in your brief matches, stop and report. Don't improvise around it.
- Snapshot tests use `insta`. Read every new snapshot before accepting it.
- `cargo test` never touches the network. Agent tests use a scripted fake model; live runs need
  `ANTHROPIC_API_KEY`. Smoke scripts under `scripts/` may install packages.
- Stay inside the crate you were given. If the IR needs a change, stop and report it instead of
  editing `oryva-ir`.
- Don't ask the user about anything decided above or about routine engineering calls. Decide,
  note the decision in your report, and keep going. Ask only for what only the user can provide:
  API keys, money, accounts, a license.
- Keep it small: no abstraction with one implementation, no config for a value that never
  changes. Generated code must read as if a person wrote it.
- Git: commits are authored by `Khushiyant <khushiyant2002@gmail.com>` from the global config;
  never set a repo-local identity. No `Co-Authored-By` or other AI-attribution lines.
- Commit messages follow Conventional Commits: `type(scope): imperative summary`, lowercase
  after the colon. Scopes are crate names without the `oryva-` prefix (`ir`, `spec`,
  `emit-python`, `eval`, `cli`).
- The repo is public at github.com/Hopline-AI/oryva. Only the main agent pushes, and only
  after `scripts/verify.sh` passes on the merged result. Subagents never push.

## Agents

The main agent (a larger model) owns design, the IR, reviews and merges. Implementation work
goes to subagents, one crate each, each in its own git worktree.

| Work | Model |
|---|---|
| Tiny and mechanical, where a wrong result is obvious from the checks | Haiku |
| Clear work in one area, or following an existing pattern | Sonnet |
| New architecture or hard judgement | Opus |

Whoever builds doesn't verify. Before merging a branch, the main agent (or a fresh read-only
agent) reruns `scripts/verify.sh` on it and does a mutation check: inject one small bug into
code the new tests cover, confirm a test fails, restore. A branch whose tests don't catch the
bug doesn't merge.

When an agent gets something wrong twice, add a line to this file.
