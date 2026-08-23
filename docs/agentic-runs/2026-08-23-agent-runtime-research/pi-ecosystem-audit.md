# pi-agent: what it actually is, and how embeddable it is (83cc runtime fork)

**Verdict.** "pi-agent" is not a package; it is the `@earendil-works/*` npm scope of the **Pi coding agent** monorepo (`github.com/earendil-works/pi`, MIT, 95.8k stars, pushed 2026-08-23, latest release `0.84.2` published 2026-08-14). The ecosystem is cleanly layered, and the layer AudioGraph would actually want — **`@earendil-works/pi-agent-core` (1.7 MB, 6 deps)** — is a genuinely reusable, host-agnostic agent loop: `new Agent({ streamFn, tools, systemPrompt })` with event streaming, steering/follow-up queues, `beforeToolCall`/`afterToolCall` gates, parallel/sequential tool execution, pluggable session storage, and compaction. It is **CI-gated browser-bundle-safe** — upstream `scripts/browser-smoke-entry.ts` builds an actual `Agent` with `esbuild platform:"browser"`, and a second gate proves a selective-provider bundle drags in exactly one provider catalog and one vendor SDK. It ships `streamProxy`, a first-class "the server holds the auth" stream function. So the honest answer to "is it embeddable" is **yes, more so than the corpus's CLI framing suggests** — but embeddability is exactly what makes this dangerous rather than easy: the supported embedding shapes are (a) agent loop **in the webview** with LLM egress delegated to Rust, or (b) agent loop **in a Node process** with keys in Node. Shape (a) is the only one that preserves the SECURITY FACTS, and it moves *context assembly, tool dispatch, and system-prompt authorship* out of Rust and into the webview — i.e. outside `route.rs`'s ADR-0038 named-route gate unless Rust re-validates every delegated call. Meanwhile the *coding-agent* layer above `pi-agent-core` (`@earendil-works/pi-coding-agent`) is a poor fit: it is a terminal harness whose value is `read`/`write`/`edit`/`bash` + TUI + `~/.pi/agent/` config discovery + jiti-loaded third-party extensions that can read your API keys via `ctx.modelRegistry.getProviderAuth()`. For a gated meeting-answer flow over transcript/notes/graph, roughly 90% of what pi-coding-agent brings is irrelevant and the remaining 10% (agent loop + provider abstraction) is a ~1200-line Rust equivalent that AudioGraph already has 80% of. **Recommendation: do not adopt the coding-agent; treat `pi-agent-core`'s `Agent` as a *design spec* to copy, and treat `pi-ai`'s `Model{baseUrl, compat}` as confirmation that the local-LLM seam is a config knob either way.**

---

## 1. Naming: what exists and what does not

| Name | Real? | Evidence |
|---|---|---|
| `pi-agent` (npm) | **No** | `registry.npmjs.org/pi-agent` → 404 (probed 2026-08-23) |
| `pi-agent-core` (unscoped npm) | Squatted placeholder | `0.0.1`, 2026-04-07, description literally "Placeholder package name reservation for pi-agent-core." |
| `@earendil-works/pi-agent-core` | **Yes** — the agent loop | `0.84.2`, 2026-08-14, MIT |
| `@mariozechner/pi-agent` | Dead predecessor | last `0.9.0`, 2025-11-21 |
| `@mariozechner/pi-ai` | Dead predecessor | last `0.73.1`, 2026-05-07 |
| `badlogic/pi-mono`, `earendil-works/pi-mono` | Renamed | GitHub API returns `301 Moved Permanently` → `earendil-works/pi` |

Author: **Mario Zechner** (badlogic). The scope migrated `@mariozechner/*` → `@earendil-works/*` around May 2026 and the repo migrated `badlogic/pi-mono` → `earendil-works/pi`. Stale doc comments still reference the old names (`@mariozechner/agent` appears in a declaration-merging example in `pi-agent-core/dist/types.d.ts`). Any prior-art note or plan that says "pi-agent" should be corrected to name the concrete package.

## 2. Package boundaries — which piece is which

Verified from installed `0.83.0` manifests (`/home/codeseys/.bun/install/global/node_modules/@earendil-works/`) plus npm metadata for `0.84.2`.

| Package | Role | Deps (latest) | Disk (0.83.0) |
|---|---|---|---|
| **`pi-agent-core`** | **The reusable agent loop.** `Agent` class, low-level `agentLoop`/`agentLoopContinue`, tool execution, event stream, steering/follow-up queues, session tree + storage interfaces, compaction, skills formatting, `streamProxy` | `pi-ai`, `pi-telemetry`, `diff`, `yaml`, `ignore`, `typebox` | **1.7 MB** |
| **`pi-ai`** | **Provider abstraction.** `Model{api, baseUrl, compat}`, `Models` store, ~40 providers, lazy per-API adapters, credential-store interface, OAuth | `openai`, `@anthropic-ai/sdk`, `@google/genai`, `@aws-sdk/client-bedrock-runtime`, `@mistralai/mistralai`, `@opentelemetry/api`, `http(s)-proxy-agent`, `partial-json`, `typebox` | 29 MB |
| **`pi-coding-agent`** | **The CLI/product.** TUI interactive mode, print/JSON mode, RPC mode, SDK façade (`createAgentSession`), `~/.pi/agent` resource discovery, extensions/skills/prompts/themes/packages, coding tools, `/model` `/login` `/llama` | the three above + `jiti`, `glob`, `undici`, `chalk`, `cross-spawn`, `proper-lockfile`, `grok-mermaid`, `pi-client`, `pi-protocol`, … | 15 MB |
| `pi-tui` | Terminal renderer. Irrelevant to a React panel | `marked`, `get-east-asian-width` | 2.1 MB |
| `pi-protocol` / `pi-client` / `pi-server` | **Experimental** remote-session layer: length-prefixed CBOR (`[uint32-be len][CBOR]`), `PiClient` over an abstract `ByteTransport`, `PiServer` behind unix socket / WS listeners | `pi-ai`, each other | new in 0.84.x |
| `pi-session-backend-sqlite-node` | Opt-out SQLite session store, kept out of core so core has no `node:sqlite` | `pi-agent-core`, `pi-ai` | new in 0.84.x |
| `pi-telemetry` | "Vendor-neutral telemetry contracts and typed schema utilities for pi" | — | new in 0.84.x |

The split is real, not aspirational. `pi-agent-core`'s public `Agent` (`dist/agent.d.ts`) takes only: `initialState{systemPrompt, model, thinkingLevel, tools, messages}`, a required `streamFn`, optional `convertToLlm` / `transformContext` / `getApiKey` / `beforeToolCall` / `afterToolCall` / `prepareNextTurn` / `shouldStopAfterTurn`, `sessionId`, `thinkingBudgets`, `transport`, `toolExecution`. It exposes `subscribe()`, `prompt()`, `continue()`, `steer()`, `followUp()`, `abort()`, `waitForIdle()`, `reset()`. Nothing in that surface knows about terminals, filesystems, or `~/.pi`.

Coding tools *do* live in `pi-agent-core/dist/harness/tools/` (`createBashTool`, `createReadTool`, `createEditTool`, `createWriteTool`) — but as **factories** parameterised by an `ExecutionEnv` (`FileSystem & Shell`, `harness/types.d.ts:238`). The only shipped implementation, `NodeExecutionEnv`, is reachable **only** through the separate `./node` subpath export. Nothing pulls it in by default.

## 3. Runtime requirements — Node? Bun? browser?

- **Declared:** `engines.node >= 22.19.0` on all four core packages. A `legacy-node20` dist-tag exists, frozen at `0.74.2`.
- **Bun:** the CLI binary is built with `bun build --compile`; `pi-ai` carries an explicit Bun-sandbox `process.env` workaround (`dist/utils/provider-env.js`). Bun is a supported host, not required.
- **Browser/webview: supported and CI-gated, for `pi-ai` + `pi-agent-core` only.**
  - `scripts/browser-smoke-entry.ts` (verified present upstream, 2275 bytes) constructs a live `Agent` and imports `streamProxy`, `PiClient`, `decodeCbor`, `InMemorySessionRepo`, skills formatters. Its header: *"Keep this entry browser-safe. It is bundled by scripts/check-browser-smoke.mjs to catch accidental Node-only runtime imports in browser-facing package exports."*
  - `scripts/check-browser-smoke.mjs` runs `esbuild` with `platform:"browser"` and then a **second** treeshake gate over `scripts/agent-treeshake-smoke-entry.ts`, which **throws** if the bundle includes `packages/ai/src/compat.ts`, `models.generated.ts`, or `providers/all.ts`, if more than one provider catalog JSON contributes bytes, or if more than one of `{@anthropic-ai/sdk, @aws-sdk/client-bedrock-runtime, @google/genai, @mistralai/mistralai, openai}` is bundled.
  - The idiom that satisfies it is three lines:
    ```ts
    import { Agent } from "@earendil-works/pi-agent-core";
    import { createModels } from "@earendil-works/pi-ai";
    import { anthropicProvider } from "@earendil-works/pi-ai/providers/anthropic";
    ```
  - Browser-tolerance is deliberate in the source, not accidental: `pi-ai/dist/auth/context.js` uses `const importNodeModule = (specifier) => import(specifier)` with the comment *"Variable specifier so browser bundlers do not try to resolve node builtins"*, and documents `fileExists` as *"always false in browsers"*. `provider-env.js` guards `typeof process === "undefined"`.
- **Not browser-safe:** OAuth login flows (`node:http` callback servers), Amazon Bedrock (`@smithy/node-http-handler`, proxy agents), and all of `pi-coding-agent` (jiti, `node:fs`, spawn, TUI).

So the 29 MB `pi-ai` figure is a red herring for a webview embed: that number is the full monorepo-published tree including all model catalogs, sourcemaps, and five vendor SDKs. The gated selective-import path is one catalog + one SDK.

## 4. Provider abstraction and the local-LLM seam

`Model<TApi>` (`pi-ai/dist/types.d.ts:647-666`) carries **`baseUrl: string` as a required per-model field**, plus `headers?` and a per-API `compat?` block. `api` is `KnownApi | (string & {})` — extensible. There is no hardcoded provider host anywhere in the type.

Native APIs shipped: `anthropic-messages`, `openai-completions`, `openai-responses`, `azure-openai-responses`, `openai-codex-responses`, `google-generative-ai`, `google-vertex`, `bedrock-converse-stream`, `mistral-conversations`, `pi-messages`, `cloudflare`, plus ~40 provider presets (openai, anthropic, google, xai, groq, cerebras, deepseek, fireworks, together, openrouter, huggingface, nvidia, minimax, moonshot, zai, qwen, xiaomi, ant-ling, github-copilot, opencode, vercel-ai-gateway, cloudflare-ai-gateway, radius, …).

**Custom OpenAI-compatible endpoints are a documented first-class feature**, not a hack (`pi-coding-agent/docs/models.md`, titled "Custom Models", opening line: *"Add custom providers and models (Ollama, vLLM, LM Studio, proxies) via `~/.pi/agent/models.json`"*):

```json
{"providers":{"ollama":{"baseUrl":"http://localhost:11434/v1","api":"openai-completions",
  "apiKey":"ollama","compat":{"supportsDeveloperRole":false,"supportsReasoningEffort":false},
  "models":[{"id":"gpt-oss:20b","reasoning":true}]}}}
```

The `compat` block is exactly the per-endpoint divergence knob the sibling `local-llm-seam.md` note argues every runtime will need — pi already has it, and `docs/models.md` names Ollama, vLLM, and SGLang as the concrete cases. There is also a dedicated `docs/llama-cpp.md` with a `/llama` command that drives the llama.cpp **router** server (load/unload/download GGUFs, `LLAMA_BASE_URL` env). Two further seams matter:

- `StreamOptions.fetch?: FetchFunction` (`pi-ai/dist/types.d.ts:41,53-57`) — per-request fetch injection, added in 0.83.0. Caveat in the docstring: *"Provider adapters that cannot inject a custom implementation may reject it. This does not affect WebSocket transports."*
- `CredentialStore` is an **interface**; the default is `InMemoryCredentialStore` and the docstring says *"Apps inject persistent stores."*

**Conclusion for the forward constraint:** pi's provider layer does not foreclose Rust-side local inference, and neither does anything else — because AudioGraph *already ships* Rust-native local inference (`src-tauri/src/llm/mod.rs:5-11`: `engine.rs` = llama-cpp-2 GGUF in-process, `mistralrs_engine.rs` = mistral.rs/Candle with JSON-Schema-constrained generation, both ON by default via `Cargo.toml:45-46` `default = ["local-ml"]`) plus a generic OpenAI-compatible client (`api_client.rs`, documented for "Ollama, LM Studio, vLLM, etc."). The premise "someday local LLM inference may land in Rust (not before a rust-native vllm-class server exists)" is already false in this repo. See `local-llm-seam.md` for the server-side side of that.

## 5. Embedding seams, ranked by trust-boundary cost

| # | Shape | Where the key lives | CSP / bundle impact | Rust work |
|---|---|---|---|---|
| **A** | `pi-agent-core` in the **webview**, custom `streamFn` that `invoke()`s a Tauri command and pushes `AssistantMessageEventStream` events | **Rust only** ✅ | none — no HTTP from webview | one streaming Tauri command emitting pi-ai event shapes |
| B | `pi-agent-core` in the webview + `streamProxy` | **Rust only** ✅ | needs `connect-src http://127.0.0.1:<port>`; opens a local HTTP listener | implement `POST /api/stream`, `Authorization: Bearer`, SSE `data: <ProxyAssistantMessageEvent>` |
| C | `pi-agent-core` in the webview + `pi-ai` providers directly | **webview** ❌ | needs `connect-src` to every provider host | none |
| D | `pi-coding-agent` as a **Node sidecar** (`--mode rpc`, JSONL over stdio) | **Node process** ❌ | new `externalBin` + bundled Node runtime | IPC bridge + process supervision |
| E | `pi-server` + `pi-client` over CBOR/unix socket | Node process ❌ | — | would need a **Rust CBOR protocol impl** that does not exist |

`streamFn`'s whole contract is three lines (`pi-agent-core/dist/types.d.ts`): `(model, context, options) => AssistantMessageEventStream | Promise<...>`, "must not throw", encode failures as a terminal `AssistantMessage` with `stopReason: "error"|"aborted"`. `AssistantMessageEventStream` is exported from `pi-ai` and is push-based (`pi-ai/dist/api/lazy.js` constructs and pushes into one). **Shape A is small and it is the only shape that keeps the SECURITY FACTS intact.**

`streamProxy` (`pi-agent-core/dist/proxy.js:83-135`) is worth naming precisely because it proves upstream *intends* this split — its docstring is *"Proxy stream function for apps that route LLM calls through a server. The server manages auth and proxies requests to LLM providers."* Wire shape: `POST ${proxyUrl}/api/stream`, `Authorization: Bearer ${authToken}`, body `{model, context, options}`, response `data: <json>\n` lines of a bandwidth-stripped `ProxyAssistantMessageEvent` union.

**But note what shapes A and B do not solve.** In both, the *webview* owns: system-prompt authorship, context assembly (which transcript segments, which notes, which graph nodes), tool schemas, and tool execution. AudioGraph's ADR-0038 gate is Rust-side by construction (`src-tauri/src/llm/route.rs:1-25`: "Routes are **named entities**, not provider strings assembled at the call site… resolves exactly one route, gates it against ADR-0033's product-enablement boundary, and stamps the resolved route id into provenance **from trusted code**" — and it deliberately has "no `authorized_fallbacks` field and no walker"). A JS-side agent loop that hands Rust a fully-formed `{model, context}` inverts that: Rust would be stamping provenance on a context it did not assemble. Preserving the invariant means the Rust command must re-derive the route from a named agent-route id and re-run `enforce_session_content_policy` on the delegated payload — i.e. the gate has to move from "Rust assembles, therefore Rust knows" to "Rust validates an untrusted assembly." That is a real, nameable security-design cost, not a wiring detail.

## 6. Extension model — could AudioGraph register domain tools cleanly?

**At the `pi-agent-core` layer: yes, trivially and safely.** `AgentTool` = `{name, label, description, parameters: TSchema, execute(toolCallId, params, signal, onUpdate), executionMode?}`. A `query_knowledge_graph` / `read_live_transcript` / `read_notes` tool is a plain object; `onUpdate` gives streaming partial results; `beforeToolCall` returning `{block:true, reason}` is a per-call gate (the gated auto-answer flow maps onto it directly); `AgentToolResult.terminate` and `shouldStopAfterTurn` give bounded loops. `AgentState.tools` is assignable mid-session. This part of the API is genuinely good and is the strongest argument for copying its *design*.

**At the `pi-coding-agent` layer: yes, but the extension host is a liability.**
- `pi.registerTool()` works at load and at runtime, refreshed in-session (`docs/extensions.md:1337-1341`).
- Extensions are TypeScript modules loaded via **jiti**, run with full user permissions, and *"Node.js built-ins (`node:fs`, `node:path`, etc.) are also available"* (`docs/extensions.md:149`).
- **`ctx.modelRegistry.getProviderAuth(id)` "resolves its current API key, headers, base URL, and provider-scoped environment"** (`docs/extensions.md:985`). This is not theoretical: the maintainer's own `pi-side-chat` calls `ctx.modelRegistry.getApiKeyAndHeaders(model)` at `/home/codeseys/DevBox/custom-pi-setup/pi-side-chat/src/index.ts:60`. **Any loaded extension can read every configured API key.**
- `pi.registerProvider(name, config)` accepts `apiKey` as a literal, `$ENV_VAR`, or **`!command`** (shell-executed) (`docs/extensions.md:1795`).
- `docs/security.md` is explicit: *"Pi does not include a built-in sandbox… Extensions are TypeScript modules that run with the same permissions."* Project trust is *"only an input-loading guard."*
- Default session persistence writes **full transcripts** as JSONL to `~/.pi/agent/sessions/` (`docs/sdk.md`, "Directories"). AudioGraph's credential module deliberately keeps secrets out of the app-data tree (`src-tauri/src/credentials/mod.rs:9-14`); a pi session store would be a *third* location, outside both, holding meeting content. Avoidable — `SessionManager.inMemory()` / `InMemorySessionStorage` / a custom `SessionStorage` — but it is the default and defaults leak.

**MCP is not in pi core.** `docs/usage.md:296`: *"It intentionally does not include built-in MCP, sub-agents, permission popups, plan mode, to-dos, or background bash."* Confirmed independently via deepwiki over `earendil-works/pi`. MCP means adopting `pi-mcp-adapter` — third party (Nico Bailon / `nicopreme`, `2.27.0`, 2026-08-20, MIT, 57 versions), pulling `@modelcontextprotocol/{client,core,ext-apps}`, **`@napi-rs/keyring`** (native), `zod`, `open`, `cross-spawn`, and peer-pinned to `@earendil-works/pi-ai: ^0.84.1`. For AudioGraph that is a native-module supply-chain addition to reach a capability the epic does not ask for.

## 7. License, maintenance pulse, API stability

- **License:** MIT across every `@earendil-works/*` package and the repo. No CLA/attribution constraint. Clean.
- **Pulse:** `earendil-works/pi` — 95,855 stars, 137 open issues, last push **2026-08-23** (today), not archived. `0.84.2` published 2026-08-14. The bundled `CHANGELOG.md` has **267 releases**; the 0.83.0 entry alone credits 14 distinct outside contributors by PR. This is a fast-moving, heavily-contributed project. Maintenance risk is not abandonment.
- **API stability: this is the material risk.** `0.73.1 → 0.84.2` in ~3.5 months. **43 of 267 releases carry a `### Breaking Changes` section**, and they land in *minor* bumps: 0.83.0, 0.80.8, 0.80.7, 0.75.0, 0.73.0, 0.72.0, 0.71.0, 0.70.0, 0.69.0, 0.68.0, 0.65.0 … 0.83.0's breaking change was a TypeBox 1.3.7 upgrade that removed `Type.Base`, `Type.Awaited`, `Type.Promise`, `Type.Options`, `Value.Mutate` — i.e. **it broke tool-parameter schemas**, the exact surface AudioGraph's domain tools would live on. `pi-client`/`pi-server`/`pi-protocol` are labelled experimental and "subject to breaking changes."
- **The maintainer's own harness already documents the cost:** `custom-pi-setup/capability-map.md:100-101` pins the host prerequisite at **Pi `>=0.81.1 <0.83.0`** ("locked baseline `0.81.1`, exercised host `0.82.1`"), and *"a host outside that window refuses to"* proceed. npm latest is `0.84.2`. That is an upper-bounded pin three minors behind current, in the maintainer's own tooling, maintained by the person who wrote it. Whatever AudioGraph pins would age the same way — except AudioGraph ships to end users on a release cadence, not to one operator's box.

## 8. What the maintainer's own packages prove about extension cost

All four are peer-dep-only against `@earendil-works/pi-*`, zero-runtime-dep (except one `proper-lockfile`), MIT, with a `pi.extensions` manifest entry:

| Package | src LOC | Capability |
|---|---|---|
| `pi-memory-layers` (ON HOLD) | 8,332 | auditable project/session memory |
| `pi-compaction-router` (v0.4.2) | 5,783 | compaction summariser model routing + resume |
| `pi-adaptive-thinking` (ARCHIVED) | 996 | reasoning-effort control |
| `pi-side-chat` (v0.1.4) | 267 | `/btw` transcript-isolated side answer |

**~15,400 lines of pi-extension TypeScript, authored by the maintainer**, plus `pi-dynamic-fractal-workflows`, `pi-agentic-sdlc-skills`, `pi-bedrock-mantle`, `pi-lab`, and a 5,387-package catalog survey with 19 feature indexes (`pi-ecosystem-research/`). Fluency is not in question — this is probably top-percentile familiarity with the pi extension API outside the maintainer of pi itself.

Two things that fluency *proves*, which cut against adoption rather than for it:
1. **The cheap thing is cheap and the useful thing is not.** `pi-side-chat` — "snapshot stable messages, one tool-free model call, show the answer outside the transcript" — is 267 lines, and it is nearly the *simplest possible* pi extension. The moment a capability touches compaction or memory it costs 6–8k lines and lands in "ON HOLD"/"ARCHIVED". A gated meeting-answer agent with graph/transcript/notes tools is in the second bucket, not the first.
2. **The prior art already wrote down the governing rules, and they argue against this.** `capability-map.md:348-407`: *"New extensions carry **no credentials, no permissions, no lifecycle authority** unless that is the package's explicit, reviewed, fail-closed purpose (only pi-bedrock-mantle holds credential material, and it never writes it to disk/session)"*; *"**In-band authority is not a boundary** (the pi-memory-layers lesson): a self-declared trust field in an agent-writable substrate is forgeable by the actor it constrains"*; *"**MCP servers are excluded** from the distributable pattern by operator instruction"*; *"do not re-implement a tool the ecosystem already has worse"*; *"Audit what is already loaded in the profile before adding a package for the same capability (the pi-lab-telemetry lesson)."* Applied to 83cc, that last rule reads: AudioGraph already owns the provider-routing, streaming, retry, schema-enforcement, and content-egress-gating slots in Rust (see `audiograph-agent-seams.md`); adding a TypeScript runtime for the same slots is the pi-lab-telemetry mistake at product scale.
3. Also relevant: `pi-adaptive-thinking` was **retired over a `/effort` command-slot collision** (`capability-map.md:136-138`, `OWNERSHIP_CONFLICT`). Slot collision between independently-versioned extensions is a lived failure mode here, not a hypothetical.

## 9. Where pi is honestly off-label for 83cc

pi describes itself as *"a minimal terminal coding harness"* (pi.dev/docs/latest). The mismatch is not fatal at the `pi-agent-core` layer and is severe above it:

| 83cc needs | pi provides | Fit |
|---|---|---|
| Multi-turn loop with domain tools, bounded | `Agent` + `AgentTool` + `beforeToolCall` + `shouldStopAfterTurn` + `terminate` | **Good** — best-in-class shape |
| Streaming into a React chat panel | `subscribe()` + `AgentEvent` union (`message_update` carries `text_delta`) | **Good** |
| Provider abstraction incl. future local endpoint | `Model{baseUrl, compat}`, `api: "openai-completions"` | **Good** — but AudioGraph has this in Rust already |
| Keys never in webview/Node | `streamFn` / `streamProxy` / injectable `CredentialStore` | **Possible** — requires the custom-`streamFn` discipline, and the extension host must be *excluded*, not configured |
| Transcript/notes never persisted outside sanctioned store | `SessionStorage` interface + `InMemorySessionStorage` | **Possible** — but JSONL-to-`~/.pi/agent/sessions/` is the default |
| Detected-question → gated auto-answer | nothing; pi has no notion of an externally-triggered turn under approval. Closest primitives are `steer()`/`followUp()` queues and `pi.sendUserMessage()` | **Build it yourself either way** |
| Meeting/audio/temporal-graph domain | nothing | **N/A** |
| File edit, shell, git, LSP, worktrees, sub-agents, skills, themes, TUI, `/model` `/login` `/llama`, package manager, project trust | the bulk of `pi-coding-agent` | **Dead weight; several items are attack surface** |
| MCP | not in core; third-party + native deps | **Not free** |
| Rust host integration | no Rust anything. `pi-protocol` is a TS CBOR spec; a Rust peer would be net-new work | **Absent** |

And two structural facts about *this* app that make the sidecar shape (D) especially costly: AudioGraph today has **no JS runtime in the shipped bundle** (no `externalBin`/`sidecar` in `src-tauri/tauri.conf.json`; frontend has 11 runtime deps), and its webview CSP is locked to IPC only — `src-tauri/tauri.conf.json:23`: `default-src 'self'; script-src 'self'; …; connect-src ipc: http://ipc.localhost`. There is no outbound-HTTP capability in the webview at all today. Shape C would require adding provider hosts to `connect-src`; shape B would require adding a loopback origin; shape A requires **no CSP change at all**.

---

## Implications for the 83cc runtime decision

1. **Correct the framing before deciding.** There is no "pi-agent". The fork is not "adopt an ecosystem vs. write Rust"; it is a three-way: (i) embed `@earendil-works/pi-agent-core` (1.7 MB, agent loop only) in the webview behind a Rust-delegated `streamFn`; (ii) run `@earendil-works/pi-coding-agent` as a Node sidecar; (iii) write the loop in Rust. Option (ii) is the only one that changes the trust boundary irrecoverably, and it is the one the word "adopt pi-agent" most naturally suggests. Kill it explicitly in the ADR so nobody re-proposes it.

2. **If pi is adopted, it must be `pi-agent-core` + a hand-written `streamFn`, and `pi-coding-agent` must be excluded by policy, not by configuration.** The exclusion is load-bearing for the SECURITY FACTS: `ctx.modelRegistry.getProviderAuth()` hands API keys to any loaded extension (`docs/extensions.md:985`, exercised at `pi-side-chat/src/index.ts:60`), extensions execute arbitrary TS with `node:fs` via jiti, `apiKey: "!command"` shells out, and `docs/security.md` states there is no sandbox. Shipping that extension host inside a desktop app that holds user LLM keys in the OS keychain (`src-tauri/src/credentials/mod.rs:1-14`) would convert "keys live in the Rust process" into "keys are readable by any pi package the user installs." A one-line ADR statement — *the pi extension host, resource loader, and package manager are never linked* — is the difference.

3. **Even the safe shape relocates the ADR-0038 gate, and that must be designed, not discovered.** With the loop in JS, Rust receives a pre-assembled `{model, context, tools}` and can no longer claim it stamped provenance "from trusted code" (`src-tauri/src/llm/route.rs:1-25`). The delegated Tauri command must accept a *named agent route id* (never a provider/baseUrl from the webview), re-resolve it through `route.rs`, and re-run the content-egress policy on the payload. Write that as the acceptance criterion for the seam. The same requirement is what makes option (iii) simpler: in Rust the gate stays where it is.

4. **Session persistence is a silent transcript-egress risk.** pi's default `SessionManager.create(cwd)` writes full transcripts as JSONL under `~/.pi/agent/sessions/` — a third data location outside both AudioGraph's app-data dir and its credential store. If pi is adopted, `SessionStorage` must be a custom implementation backed by AudioGraph's own persistence (or `InMemorySessionStorage`), and no pi artifact may be produced from a test fixture containing real transcript content.

5. **The local-LLM forward constraint does not discriminate between the options — and the constraint's premise is stale.** `pi-ai`'s `Model{baseUrl, compat}` and its documented Ollama/vLLM/LM Studio/llama.cpp support mean pi would not block a local endpoint. But AudioGraph already ships llama-cpp-2 and mistral.rs in-process by default (`src-tauri/src/llm/mod.rs:5-11`, `Cargo.toml:45-46`) plus a generic OpenAI-compatible client for local servers. Local inference in Rust is not a future dependency; it is a present capability. This factor should carry **zero weight** in the fork, and the ADR should say so rather than leaving it as an open risk.

6. **Weigh version churn as a shipping cost, not a dev-tooling annoyance.** 43 breaking-change releases in 267; breakage lands in minors; 0.83.0's breaking change hit TypeBox tool schemas specifically; `pi-client`/`pi-server`/`pi-protocol` are self-declared experimental; and the maintainer's own harness is pinned `>=0.81.1 <0.83.0` while npm is at `0.84.2`. A desktop app on a release cadence would either pin and drift (inheriting unpatched provider bugs) or track and absorb quarterly breakage in the agent panel. AudioGraph's Rust LLM layer has no such upstream clock.

7. **The highest-value use of the pi corpus here is as a specification, not a dependency.** `pi-agent-core`'s API is the cheapest available design review for a Rust agent loop: required `streamFn` with a no-throw/encode-errors-in-stream contract; `AgentEvent` union covering agent/turn/message/tool lifecycle with `message_update` deltas; `beforeToolCall`→block / `afterToolCall`→override / `shouldStopAfterTurn` / `prepareNextTurn` as the four extension points; steering vs. follow-up as *separate* queues with `all` | `one-at-a-time` drain modes; `terminate` requiring unanimity across a tool batch; `parallel` default with per-tool `sequential` override that escalates the whole batch; and `AgentToolResult.addedToolNames` for progressive tool disclosure. Those are hard-won details a from-scratch Rust loop will otherwise get wrong on the first pass. Port the semantics; do not port the runtime.

---

### Verification log

- npm registry probed directly 2026-08-23: `@earendil-works/pi-{coding-agent,agent-core,ai,tui,client,protocol,server,telemetry,session-backend-sqlite-node}` all `0.84.2` / `2026-08-14`, MIT, `engines.node >=22.19.0`, `repository: git+https://github.com/earendil-works/pi.git`. `pi-agent` → 404. unscoped `pi-agent-core` → `0.0.1` placeholder. `@mariozechner/pi-agent` → `0.9.0` (2025-11-21). `@mariozechner/pi-ai` → `0.73.1` (2026-05-07). `pi-mcp-adapter` → `2.27.0` (2026-08-20), maintainer `nicopreme`. `pi-web-ui` → `0.30.0` (2026-08-23), `express`+`node-pty`+React over the pi SDK.
- GitHub API 2026-08-23: `earendil-works/pi` = 95,855 stars, 137 open issues, pushed `2026-08-23T18:48:02Z`, MIT, not archived. `earendil-works/pi-mono` and `badlogic/pi-mono` → `301 Moved Permanently`.
- Raw upstream files fetched: `scripts/browser-smoke-entry.ts`, `scripts/check-browser-smoke.mjs`, `scripts/agent-treeshake-smoke-entry.ts`.
- Docs: <https://pi.dev/docs/latest>, <https://pi.dev/docs/latest/sdk>, <https://pi.dev/packages>; `github.com/earendil-works/pi/blob/main/packages/agent/README.md`.
- deepwiki `earendil-works/pi`: browser compatibility + `ExecutionEnv`; MCP absent from core; `pi-client`/`pi-server`/`pi-protocol` roles and "transport-specific authentication is completed by the listener before passing a connection to `PiServer`".
- Local installed tree read at `/home/codeseys/.bun/install/global/node_modules/@earendil-works/` (`0.83.0`, installed 2026-07-30): all four `package.json`, `pi-agent-core/dist/{agent,types,proxy,node,index}.d.ts` + `proxy.js` + `harness/**`, `pi-ai/dist/{index,types}.d.ts` + `api/lazy.js` + `auth/{context.js,credential-store.d.ts}` + `utils/provider-env.js`, and `pi-coding-agent/{CHANGELOG.md,docs/*}`.
- Local prior art read: `pi-ecosystem-research/{README.md,feature-landscape.md,cli-agent-parity.md}`, `custom-pi-setup/{README.md,capability-map.md}`, `custom-pi-setup/pi-side-chat/src/index.ts`, and `package.json` of `pi-{side-chat,compaction-router,memory-layers,adaptive-thinking}`.
- **UNVERIFIED / not checked:** whether `pi-coding-agent`'s `0.84.x` docs changed any statement quoted here from the installed `0.83.0` docs (the 0.83.0 tree is what is on disk); the exact behaviour of `StreamOptions.fetch` under the `anthropic-messages` adapter (the docstring warns some adapters may reject it); whether the `pi-ai` selective-import treeshake gate covers `openai-completions` (the gate asserts `@anthropic-ai/sdk` specifically, so the OpenAI-compatible path's browser bundle size is inferred, not measured); no `cargo`/`rustc` was run, so all Rust claims are source-read only.
