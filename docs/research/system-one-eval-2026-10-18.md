# System One replay evaluation, 2026-10-18

Seed `audio-graph-ad35`. Evaluation-only implementation. No UI, product route, heuristic replacement, or native inference.

## Contract and endpoints

`src-tauri/crates/decision-contract` contains the shared typed engine contract. Questions and option maps retain insertion order through `indexmap`. Native score answers use a vector indexed by rubric level. HTTP score objects with numeric keys are normalized by index, not object insertion order. Noncontiguous or noncanonical indices are rejected.

`WireRequest` and `WireResponse` are immutable typed projections plus original JSON. Their Serde serialization preserves original bytes, including whitespace, field order, numeric spelling, `legend`, `diagnostics`, `id`, `object`, and `created`. Fixtures come from the TypeSafe quickstart, vLLM's decision example, and Ollama's decision guide.

Native engines still return ordinary `DecisionResponse` values with `Answer::Score.probabilities: Vec<f64>`. `WireResponse::from_typed(response)` constructs canonical wire JSON, including index-keyed score probabilities. This conversion belongs to `decision-contract`; native engines and the registry need no wire types.

Lossless replay through an owned typed value retains an opaque `WireFormat` token:

```rust
let wire = WireResponse::from_json(vendor_json)?;
let (typed, format) = wire.into_parts();
// Read the shared typed value without vendor-specific fields.
let restored = WireResponse::from_parts(typed, format)?;
assert_eq!(restored.json(), vendor_json);
```

The same API exists on `WireRequest`. Reconstruction checks that the typed projection, including map order, is unchanged and returns the original bytes. Modified projections return `WireConversionError::TypedValueChanged` rather than stale JSON. Use `from_typed` when canonical new output is wanted. `into_typed` intentionally discards wire metadata for engine-neutral processing; bare typed values cannot retain unknown vendor fields. The vLLM and Ollama fixtures test vendor JSON to Wire to owned typed value plus format token to Wire to byte-identical JSON. A compile assertion pins `Arc<dyn DecisionEngine + Send + Sync>` usability. No registry or factory types were added to this crate.

A standalone Rust 1.95 binary also exercised the public conversion API with out-of-order score index keys, a legend, diagnostics, and preserved numeric spelling. Compilation and execution both exited 0. It verified byte-identical reconstruction and canonical native score encoding. Source was passed on stdin and temporary output files were removed.

| Endpoint | Model | Questions | Choice options | Score levels | Context |
| --- | --- | ---: | --- | --- | --- |
| TypeSafe `/v1/systemone` | `jev-latest` | No separate count limit | 2 to 255 | 2 to 10 | 64k request, 32k per question |
| Cloudflare `/accounts/{id}/ai/run` | `typesafe/jev` | No separate published count limit | 2 to 255 | 2 to 10 | Published 32,000 tokens |
| Cloudflare `/accounts/{id}/ai/run/@cf/cloudflare/clef-flash` | `clef-flash` | 64 | 2 to 255 | 2 to 10 | 24,576 tokens |
| Local Ollama `/v1/systemone` | `nimble` or `clef-flash` | 64 | 2 to 26, conservative common backend limit | 2 to 10 | Loaded model context, 64 KiB text/schema |
| Local vLLM `/v1/systemone` | Caller-selected | 64 | 1 to 26, common label-reader limit | 2 to 10 | Server model context |

Cardinality checks run before HTTP dispatch. Token limits remain authoritative at the provider because this client does not ship the provider tokenizer or prompt formatter. `EngineLimits.max_state_tokens` records the known context ceiling, not an exact tokenizer-backed preflight. TypeSafe `max_tokens_exceeded` and context-length failures become `DecisionError::StateTooLong`; no byte-to-token estimate silently truncates input. vLLM D3-specific 255-option support is not assumed for the common vLLM endpoint.

The initial brief's Cloudflare Jev route is outdated. Current Cloudflare documentation specifies `typesafe/jev` through `/ai/run` with `{model,input:{state,questions}}`. It does not document `@cf/typesafe/jev`. Clef-flash still uses its account-scoped `@cf` route. Cloudflare REST result envelopes are unwrapped before the shared wire response is parsed.

Cloud credentials use the existing keychain-default backend through `typesafe_api_key` and `cloudflare_workers_ai_token`. The existing presence IPC exposes only saved/missing status and source metadata. The frontend credential type and key allowlist mirror both slot names; no IPC returns their saved plaintext values. The standalone example additionally accepts `TYPESAFE_API_KEY` or `CLOUDFLARE_API_TOKEN`. Missing/rejected credentials are typed errors. HTTP bodies and underlying connection errors are never echoed to logs or reports; fixed diagnostics pass through the existing provider redactor.

Local endpoints require loopback IP literals, disable proxies and redirects, and reject Ollama `:cloud` relay names. Cloud requests require the CLI's explicit `--allow-cloud` switch. This is an evaluation guard, not the ADR-0033 product content-egress checklist.

## Replay method

`cargo example system_one_eval` reads either:

- `--session ID`: final canonical transcript spans, falling back to legacy transcript JSONL only when the canonical stream is absent. LiveAssistCardRecord heads are loaded and counted. Reference labels are recomputed with the shipped Rust heuristics, not copied from potentially stale cards.
- `--fixture PATH`: the existing `OfflineProjectionReplayFixture` format. Transcript events replay through `TranscriptLedger`; superseded partials do not become extra samples. The default is the existing `fixtures/projection_eval/two_span_repair.json`.

The additional public corpus, `fixtures/decision/public_replay.json`, contains 12 synthetic spans. Seven question examples reuse the existing `speech/mod.rs` signal-grade fixture sentences and confidence values. Added graph, note, and none examples exercise the other intent labels. This is a small diagnostic corpus, not a representative meeting benchmark or independently human-labeled accuracy set.

Each span sends one request with two questions. The state includes text and ASR confidence, but excludes speaker identifiers, cards, graph content, and other session data. `agent_proposal_kind` chooses `question`, `graph_suggestion`, `note`, or `none`. `agent_signal_grade` scores `fragment`, `weak`, or `strong`. The grade question is speculative and its result applies only when the model selects question.

Kind confusion rows are the Rust heuristic and columns are the model. Grade confusion uses only the intersection of heuristic-question and model-question spans. Both question counts and the conditional denominator are printed. Other kind disagreements remain visible in the kind matrix. Grade classification uses the highest-probability level, with lower index winning a tie, rather than rounding the expected score.

ECE uses ten equal-width probability bins. It compares the selected option's probability with agreement against the heuristic. Grade ECE uses the argmax level probability on the conditional question subset. Provider confidence is not substituted for class probability. These are heuristic-agreement calibration diagnostics, not proof of calibration against ground truth. Empty grade subsets print null, never a fabricated zero error or perfect agreement.

Requests are sequential with no warmup removal or retries. Latency includes transport, parsing, and response validation. Percentiles use nearest rank. Usage sums provider-reported input/output tokens. Estimated API cost uses $0.042 per million input tokens for Jev, $0.038 for Cloudflare Clef-flash, zero output-token charge, and zero API charge for local inference. Local energy/hardware cost and cached-token discounts are not estimated.

## Executed results

The actual `system_one_eval` executable completed a transport-only smoke using the existing two-span fixture and a temporary loopback HTTP server. The server returned fixed, explicitly synthetic responses. These numbers verify replay, serialization, HTTP dispatch, conditional grade handling, metrics, and CLI output. They are not Jev, vLLM, or other model-quality measurements.

| Transport-only smoke metric | Observed |
| --- | ---: |
| Process exit | 0 |
| HTTP requests / final spans | 2 / 2 |
| Kind confusion, heuristic none to synthetic none | 2 |
| Kind agreement / ECE | 1.0 / 0.15 |
| Grade samples / agreement / ECE | 0 / null / null |
| Latency p50 / p95 | 2.8971 / 65.1515 ms |
| Synthetic reported input / output tokens | 200 / 4 |
| Local API cost estimate | $0 |

The rebuilt final executable also replayed the 12-span public corpus through the temporary server. It exited 0 after 12 requests. Heuristic row counts were question 7, graph suggestion 2, note 1, none 2. With the fixed synthetic none answer, kind agreement was 2/12 and ECE was 0.6833333; grade count was zero. Transport p50/p95 were 0.5114/4.3477 ms and synthetic usage was 1,200 input/24 output tokens. This verifies the diagnostic corpus path, not model inference.

The real Jev invocation against `fixtures/decision/public_replay.json` exited 1 with `decision credential missing or rejected`, before sending content. No model inference numbers are available from that attempt. Endpoint evaluation remains required before this Seed can close.

Ollama was not found on PATH or in the current-user and machine-wide 32/64-bit uninstall registries. The local API on port 11434 refused a connection. A `winget list` inventory attempt timed out. No Ollama installation or model download was performed.

## Verification and remaining gates

Commands ran from `src-tauri` unless stated otherwise.

| Command | Result |
| --- | --- |
| `cargo +1.95.0 test --locked -p audio-graph-decision-contract` | Passed, 5 integration tests, including vLLM/Ollama owned typed round trips and canonical native score encoding |
| `cargo +1.95.0 test --locked --no-default-features --features cloud` | Compiled; Windows loader aborted before tests with `0xc0000139` |
| `AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1 cargo +1.95.0 test --locked --no-default-features --features cloud` | 2,154 passed, 1 failed, 11 ignored; all 7 new decision/credential tests passed |
| `cargo +1.95.0 check --locked` | Passed with the existing Windows debug local-ML CRT advisory |
| `cargo +1.95.0 check --locked --no-default-features --features cloud` | Passed after wire reconstruction changes |
| `cargo clippy --locked --all-targets -- -D warnings` | Failed with 41 diagnostics in untouched persistence test code |
| `cargo +1.95.0 clippy --locked --no-default-features --features cloud --lib --example system_one_eval -- -D warnings` | Passed |
| `cargo +1.95.0 clippy --locked -p audio-graph-decision-contract --all-targets -- -D warnings` | Passed |
| `AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1 cargo +1.95.0 test --locked --no-default-features --features cloud decision::` | Passed, 6 tests on final code |
| `AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1 cargo +1.95.0 test --locked --no-default-features --features cloud decision_credentials_survive_keychain_save_and_delete_without_plaintext_presence` | Passed, 1 test on final code |
| `cargo +1.95.0 build --locked --no-default-features --features cloud --example system_one_eval` | Passed; rebuilt executable used in the 12-request smoke |
| `cargo +1.95.0 run --locked --no-default-features --features cloud --example system_one_eval -- --help` | Passed; actual executable printed usage |
| `bun run typecheck`, repository root | Could not execute `tsc`; frontend dependencies are absent |
| `bun scripts/check-docs-secret-hygiene.mjs`, repository root | Passed, 0 findings |

The Windows test environment variable is the existing repository-prescribed launcher fix, documented in [Windows Rust test CRT skew](../ops/windows-rust-test-crt-skew.md). It changes the embedded test manifest, not test assertions.

A replay regression was reproduced before the fix and passed afterward. A speech span that naturally starts with a canned proposal prefix was being stripped one extra time when computing the reference grade. Replay now calls the existing proposal-body formatter before the existing grade function, exactly matching production minting. Only crate visibility changed in those speech helpers; their behavior is unchanged.

The full cloud suite's failure is `speech::tests_integration::deepgram_final_after_writer_cleared_reaches_ledger_but_is_counted_as_display_miss` at `speech/tests_integration.rs:2505`. It observed zero speaker-ledger rows instead of two. The decision module is not called by that path. No Deepgram integration code or persistence implementation was changed for this task.

All-target clippy reports unused/dead platform-specific test fixtures in `persistence/canonical_durability.rs`, `canonical_crash_harness.rs`, `session_artifact_manifest.rs`, and `session_semantics.rs`, plus `explicit_auto_deref` in the crash fixture. Those files remain unchanged. Their cleanup and the Deepgram test failure were reported to the orchestrator rather than folded into this decision-layer change.

This Seed remains open until a real endpoint evaluation produces model metrics and the full-suite/clippy acceptance blockers are resolved. Exact provider token preflight also needs the provider tokenizer and prompt accounting; this implementation does not claim that byte length is a token count.

## Reproduction

Run in `src-tauri` after setting credentials through the existing backend or the harness-only environment variable. Never pass a secret on the command line.

```sh
cargo +1.95.0 run --locked --no-default-features --features cloud --example system_one_eval -- --help
cargo +1.95.0 run --locked --no-default-features --features cloud --example system_one_eval -- --endpoint jev --allow-cloud
cargo +1.95.0 run --locked --no-default-features --features cloud --example system_one_eval -- --endpoint jev --allow-cloud --fixture fixtures/decision/public_replay.json
cargo +1.95.0 run --locked --no-default-features --features cloud --example system_one_eval -- --endpoint ollama --model clef-flash --fixture fixtures/decision/public_replay.json
```

A real session's text leaves the device only if the caller explicitly selects that session and allows cloud evaluation. Initial evaluation uses public fixtures only.

## Sources

Read 2026-10-18.

- [TypeSafe API](https://docs.typesafe.ai/api.md), base URL, bearer authentication, choice/score bounds, wire shapes, context error code.
- [TypeSafe quickstart](https://docs.typesafe.ai/introduction/quickstart.md), fixture request and `jev-latest`.
- [TypeSafe models](https://docs.typesafe.ai/models.md), context limits, no separate question limit, model aliases, $0.042/M input-token pricing. The page lists Jev 1.14 as preview and `jev-latest` as Jev 1.13; reports retain the actual returned model.
- [vLLM structured decisions](https://docs.vllm.ai/en/latest/models/structured_decisions_models/), three answer types, numeric score keys, legend/diagnostics, 64/26/10 common limits, D3 option-limit exception.
- [Ollama decision guide](https://docs.ollama.com/capabilities/decision), no local key, supported models, minimal Noul response, 2 to 26 options and 2 to 10 levels.
- [Ollama System One API](https://docs.ollama.com/api/systemone), 64-question limit, request/body bounds, no truncation, model-specific choice bounds.
- [Cloudflare Jev](https://developers.cloudflare.com/ai/models/typesafe/jev/), current `typesafe/jev` route/envelope, pricing and context. [Input schema](https://developers.cloudflare.com/ai/models/typesafe/jev/schema-input.json).
- [Cloudflare Clef-flash](https://developers.cloudflare.com/workers-ai/models/clef-flash/), 9B model, account-scoped endpoint, context and $0.038/M input-token pricing. [Input schema](https://developers.cloudflare.com/workers-ai/models/clef-flash/schema-input.json).
- [Cloudflare REST API](https://developers.cloudflare.com/workers-ai/get-started/rest-api/), REST result/success envelope.
