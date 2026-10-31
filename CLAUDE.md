# oryva

An SDK generator built for agents. One OpenAPI spec goes in; SDKs in several languages, an MCP
server and an agent test report come out. The baseline is everything Stainless, Speakeasy and
Fern ship. On top of that:

- The agent surface (MCP server, tool descriptions) and the SDKs come from the same model of the
  API, so they can't drift apart.
- Every release is scored by agents: pass rate, tokens per task, tool errors, recoveries.

It is a general product. Individual companies (Sarvam and others) are outreach targets, not the
design centre: their material lives under `examples/`, never in engine fixtures or tests.

Phases, status and "done when" criteria live in `PLAN.md`. Update the checkboxes there when you
finish something.

## Decisions already made — don't re-ask

- Rust (edition 2024, rustc 1.90) for the generator. A single static binary is the
  distribution story.
- Pipeline: spec → `oryva_ir::Api` → emitters. Emitters never read the spec.
- Spec parsing walks a `serde_json::Value` (YAML via `serde-saphyr`), so OpenAPI 3.0 and 3.1
  share one code path. Local `$ref`s only.
- A `const` is a single-value enum in the IR; emitters render it as an exact literal.
- Targets: Python (`httpx` + `pydantic` v2, sync and async clients) and TypeScript (native
  `fetch`, zero runtime dependencies), plus a Python MCP server (official `mcp` package) that
  wraps the Python SDK. More languages follow the same behaviour contract below.
- Templates: `minijinja`. Python output is formatted and linted with `ruff`; TypeScript output
  is formatted with `dprint-plugin-typescript` in-process and type-checked with `tsc`.
- Agent tests call the Anthropic Messages API over raw HTTP (there is no official Rust SDK):
  `POST https://api.anthropic.com/v1/messages`, header `anthropic-version: 2026-10-21`,
  default model `claude-opus-5-5`, `fallbacks: "default"` with beta header
  `server-side-fallback-2026-07-01`. Never send `tool_choice` `any`/`tool`, `temperature`, or a
  thinking budget; those return 400 on current models.
- No license chosen yet; don't add one.
- Deferred until a later phase: WebSockets, code-mode MCP, Go/Java/Kotlin/Ruby/C#/PHP, a CLI
  target, Terraform, generated docs sites, the `_gen/` regeneration loop.

## Spec extensions

All optional. Names are the contract between the parser and the emitters.

| Where | Extension | Meaning |
|---|---|---|
| `info` | `x-oryva-name` | Package stem, e.g. `ledger`. Default: snake_case of the title's first word. |
| `info` | `x-oryva-idempotency-header` | Header for automatic idempotency keys, e.g. `Idempotency-Key`. |
| `info` | `x-oryva-webhook-signing: standard-webhooks` | Webhooks are signed per standardwebhooks.com. |
| server | `x-oryva-name` | Environment name. Default: first label of the host, then `environment_<n>` on collision. |
| operation | `x-oryva-resource`, `x-oryva-method` | Override the derived names. |
| operation | `x-oryva-agent` | `read_only`, `destructive`, `idempotent`, `hint`, `examples`. |
| operation | `x-oryva-pagination` | `style: cursor` (`param`, `next_cursor`, `items`), `style: offset` (`offset_param`, `limit_param`, `items`) or `style: page` (`page_param`, `items`). Paths are lists of field names. |

## Naming rules

- Resource: the operation's first tag in snake_case; with no tag, the first static path segment.
- Method: `operationId` in snake_case with a leading `<resource>_` stripped
  (`customers_list` under tag `customers` → `list`). With no `operationId`: GET on a path ending
  in a parameter → `get`, other GET → `list`, POST → `create`, PUT/PATCH → `update`, DELETE →
  `delete`. Two operations mapping to the same resource+method is a parse error naming both.
- Hoisted inline objects: request bodies → `<Resource><Method>Request`, 2xx responses →
  `<Resource><Method>Response`, nested inline objects → `<Parent><Field>` (all PascalCase).
- Agent defaults: GET → `read_only` + `idempotent`; PUT → `idempotent`; DELETE → `destructive` +
  `idempotent`; POST/PATCH → none. `x-oryva-agent` keys override individually.
- Env vars, with `<NAME>` the upper snake API name: API key and bearer token `<NAME>_API_KEY`;
  basic auth `<NAME>_USERNAME` / `<NAME>_PASSWORD`; OAuth2 `<NAME>_CLIENT_ID` /
  `<NAME>_CLIENT_SECRET`; webhooks `<NAME>_WEBHOOK_SECRET`; base URL `<NAME>_BASE_URL`.
- Webhook ident: the event name in snake_case with dots as underscores (`invoice.paid` →
  `invoice_paid`).
- Client class: PascalCase API name (`Ledger`, async `AsyncLedger`). Resources are attributes
  (`client.customers.list(...)`). Python keywords get a trailing underscore (`from_`).
- MCP tool names: `<resource>_<method>` (`customers_list`).

## SDK behaviour contract (every language)

Each language uses its own idioms, but every SDK does all of this:

- **Auth:** constructor arguments for each scheme the API offers, falling back to the env vars
  above. OAuth2 client credentials: fetch a token, cache it, refresh 60 s before expiry; the token URL
  can be overridden (`<NAME>_OAUTH_TOKEN_URL`).
- **Environments:** pick a named server (`environment="sandbox"`) or pass a base URL; the env var
  overrides the default.
- **Retries:** 408, 409, 429, 5xx and connection errors, exponential backoff with jitter, honour
  `Retry-After` (seconds or HTTP date, capped at 60 s). Default 2 retries, configurable per client
  and per request.
- **Idempotency:** when the API declares an idempotency header, every non-GET request gets a fresh
  key, reused across its retries; callers can pass their own.
- **Per-request options:** timeout, max retries, extra headers, extra query, extra body.
- **Omitted vs null:** omitting an optional argument sends nothing; passing null sends `null`.
- **Forward compatibility:** enums and discriminated unions accept values the SDK doesn't know,
  without raising; unknown object fields are kept.
- **Pagination:** list methods return a page that exposes its items and the next page, and can be
  iterated to walk every item across pages.
- **Streaming:** event-stream methods return an iterable of typed events that can be closed early.
- **Files:** uploads accept bytes, paths and file objects; downloads return bytes and can stream
  to a file.
- **Raw responses:** a way to get status, headers and the parsed body together.
- **Errors:** one class per status family plus connection and timeout errors, carrying status,
  body, request id, and a message that says how to fix the problem.
- **Webhooks:** `client.webhooks.unwrap(body, headers)` verifies the signature (5-minute
  tolerance) and returns the typed event; a bad signature raises a dedicated error.
- **Telemetry:** a `User-Agent` of `<name>-<language>/<version>`. Debug logging of requests with
  credentials redacted, turned on by an env var `<NAME>_LOG=debug`.
- **Deprecation:** deprecated methods still work and warn once.

## Layout

| Path | What | Owner |
|---|---|---|
| `crates/oryva-ir` | The API model | Main agent only. Changes need an `IR_VERSION` bump. |
| `crates/oryva-spec` | OpenAPI 3.0/3.1 → IR | |
| `crates/oryva-emit-python` | IR → Python SDK + MCP server | |
| `crates/oryva-emit-typescript` | IR → TypeScript SDK | |
| `crates/oryva-eval` | Agent tests against any MCP server | |
| `crates/oryva` | CLI: `oryva generate`, `oryva eval` | |
| `fixtures/specs` | Engine fixtures. `ledger.yaml` (fictional billing API) exercises every feature; extend it when you add one. | |
| `examples/<company>` | Material for one outreach target. Never used by tests. | |

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
  `ANTHROPIC_API_KEY` (from `.env`, never printed). Smoke scripts under `scripts/` may install
  packages.
- Stay inside the paths you were given. If the IR needs a change, stop and report it instead of
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
  `emit-python`, `emit-typescript`, `eval`, `cli`).
- The repo is public at github.com/Hopline-AI/oryva. Only the main agent pushes, and only
  after `scripts/verify.sh` passes on the merged result. Subagents never push.

## Agents

The main agent (a larger model) owns design, the IR, reviews and merges. Implementation work
goes to subagents, one area each, each in its own git worktree branched from local `HEAD`.
Usage is capped: at most 3 agents per session without the user's approval (a hook asks), and
big spends get a cost estimate first.

| Work | Model |
|---|---|
| Tiny and mechanical, where a wrong result is obvious from the checks | Haiku |
| Clear work in one area, or following an existing pattern | Sonnet |
| New architecture or hard judgement | Opus |

Whoever builds doesn't verify. Before merging a branch, the main agent (or a fresh read-only
agent) reruns `scripts/verify.sh` on it and does a mutation check: inject one small bug into
code the new tests cover, confirm a test fails, restore. A branch whose tests don't catch the
bug doesn't merge. Restore from a copy of the file made before the mutation, never with
`git checkout`, which also throws away uncommitted work. Judge "caught" by the test command's
own exit code, and make sure the mutated code still compiles. Templates are embedded with
`include_str!`, so `touch` a restored template or cargo keeps the mutated build.

When an agent gets something wrong twice, add a line to this file.
