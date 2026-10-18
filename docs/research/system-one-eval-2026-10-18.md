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

- `--session ID --allow-private-session`: final canonical transcript spans, falling back to legacy transcript JSONL only when the canonical stream is absent. The consent flag is required before reading private session content, including local replay. Cloud replay additionally requires `--allow-cloud`. LiveAssistCardRecord heads are loaded and counted. Reference labels are recomputed with the shipped Rust heuristics, not copied from potentially stale cards.
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
| `AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1 cargo +1.95.0 test --locked --no-default-features --features cloud` | Latest parallel run: 2,153 passed, 7 failed, 11 ignored, all failures in untouched modules |
| `AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1 cargo +1.95.0 test --locked --no-default-features --features cloud -- --test-threads=1` | Passed, 2,160 tests plus 1 doctest, 0 failures, 11 ignored |
| `cargo +1.95.0 check --locked` | Passed with the existing Windows debug local-ML CRT advisory |
| `cargo +1.95.0 check --locked --no-default-features --features cloud` | Passed after wire, stdin credential, and consent changes |
| `cargo clippy --locked --all-targets -- -D warnings` | Failed with 41 diagnostics in untouched persistence test code |
| `cargo +1.95.0 clippy --locked --no-default-features --features cloud --lib --example save_typesafe_credential --example system_one_eval -- -D warnings` | Passed |
| `cargo +1.95.0 clippy --locked --no-default-features --features cloud --all-targets -- -D warnings` | Failed with the same 41 diagnostics in untouched persistence tests |
| `cargo +1.95.0 clippy --locked -p audio-graph-decision-contract --all-targets -- -D warnings` | Passed |
| `AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1 cargo +1.95.0 test --locked --no-default-features --features cloud decision::` | Passed, 9 tests including Cloudflare envelope/auth, typed HTTP failures, and cardinality maxima |
| `AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1 cargo +1.95.0 test --locked --no-default-features --features cloud stdin_credential` | Passed, 2 tests; invalid input preserves the saved value and keychain failure creates no plaintext fallback |
| `AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1 cargo +1.95.0 test --locked --no-default-features --features cloud decision_credentials_survive_keychain_save_and_delete_without_plaintext_presence` | Passed, 1 test on final code |
| `cargo +1.95.0 build --locked --no-default-features --features cloud --example save_typesafe_credential --example system_one_eval` | Passed; both actual executables exercised |
| `cargo +1.95.0 run --locked --no-default-features --features cloud --example system_one_eval -- --help` | Passed; actual executable printed usage |
| `bun run typecheck`, repository root | Could not execute `tsc`; frontend dependencies are absent |
| `bun scripts/check-docs-secret-hygiene.mjs`, repository root | Passed, 0 findings |

The Windows test environment variable is the existing repository-prescribed launcher fix, documented in [Windows Rust test CRT skew](../ops/windows-rust-test-crt-skew.md). It changes the embedded test manifest, not test assertions.

A replay regression was reproduced before the fix and passed afterward. A speech span that naturally starts with a canned proposal prefix was being stripped one extra time when computing the reference grade. Replay now calls the existing proposal-body formatter before the existing grade function, exactly matching production minting. Only crate visibility changed in those speech helpers; their behavior is unchanged.

The initial parallel cloud run failed the Deepgram speaker-ledger test at `speech/tests_integration.rs:2505`, observing zero rows instead of two. The latest parallel run also failed analytics hub, sessions artifact path, usage seed/migration, shutdown lifecycle, and projection fold-counter tests. Running the full suite serially passed all 2,160 tests and the doctest, including all seven previously failing tests. This is evidence of parallel test-isolation sensitivity, not a verified production regression or a repaired parallel suite. No code in those modules was changed for this work.

The persistence lint baseline is tracked separately as `audio-graph-9f65`. Per the orchestrator's decision, ad35 clippy acceptance requires zero new diagnostics against `origin/master` and zero diagnostics in files added or changed by this task. The measured comparison below passes that criterion. Persistence modules were not edited.

The stdin example's actual empty, command-argument, and oversized-input probes each printed only `missing`, exited 1, and emitted no stderr. A console/PTY refusal probe could not execute because the host PTY tool failed; no console-refusal execution result is claimed. Actual Jev and local Ollama private-session probes, without `--allow-private-session`, both exited 1 before loading content. Only the committed public corpus was used for the real Jev attempt, which still stopped on the missing credential.

Live evaluation remains `live eval pending credential` until the human saves the key through the stdin-only command. The clippy baseline-delta criterion and serial full-cloud verification now pass. The raw parallel-suite and baseline lint findings remain separately recorded. Exact provider token preflight still needs the provider tokenizer and prompt accounting; this implementation does not claim that byte length is a token count.

### Clippy baseline acceptance comparison

A detached, stash-free scratch checkout pinned `origin/master` at `a6ea3f8e51a6e93c0ebcc6812c60eb335595046c`. The implementation head was `c68b3ba1462493d1499541985c5791a074aca55c`. Both runs used the package's Rust 1.95.0, Cargo 1.95.0, and Clippy 0.1.95 with default features on Windows. No persistence source was changed. The scratch checkout was removed after comparison.

The requested all-target lint command was executed on both revisions. The only added option selected machine-readable output, not different lint or feature behavior:

```sh
cargo clippy --locked --all-targets --message-format=json -- -D warnings
```

| Metric | Baseline | Implementation |
| --- | ---: | ---: |
| Compiler diagnostics | 41 | 41 |
| Raw clippy exit status | 101 | 101 |

| Persistence file | Baseline | Implementation |
| --- | ---: | ---: |
| `canonical_crash_harness.rs` | 16 | 16 |
| `session_semantics.rs` | 11 | 11 |
| `canonical_durability.rs` | 8 | 8 |
| `session_artifact_manifest.rs` | 6 | 6 |
| Total | 41 | 41 |

The comparison used diagnostic code, severity, message, and primary source span, with paths normalized relative to each repository root. This was a set comparison, not just a comparison of totals:

- New diagnostics: **0**.
- Removed diagnostics: **0**.
- Clippy diagnostics in any of the 17 added/changed files from `git diff --name-only` between those revisions: **0**.
- The separate Windows debug local-ML CRT build-script advisory was identical on both revisions.

Both raw clippy commands still fail on the same baseline. The result is acceptance under the approved zero-delta rule, not a claim that all-target clippy is globally clean. The subsequent cloud-suite rerun with the Windows manifest and `--test-threads=1` passed 2,160 tests and one doctest, with 11 ignored.


## Reproduction

Run in `src-tauri` after setting credentials through the existing backend or the harness-only environment variable. Never pass a secret on the command line.

### Human-only stdin credential entry

Do not send the key through chat, command-line arguments, files, or logs. Run this in the human's own PowerShell terminal. `Read-Host -AsSecureString` hides the input, and the value is piped only to the child process's stdin:

```powershell
$credential = Read-Host 'TypeSafe API key' -AsSecureString
$buffer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($credential)
try {
  [Runtime.InteropServices.Marshal]::PtrToStringBSTR($buffer) |
    cargo +1.95.0 run --quiet --locked --no-default-features --features cloud --example save_typesafe_credential
} finally {
  [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($buffer)
  $credential.Dispose()
  Remove-Variable credential, buffer
}
```

The example prints only `saved` or `missing`. It rejects command-line values and terminal stdin so the key cannot accidentally be typed into an echoing stdin prompt. The reader accepts at most 16 KiB, rejects malformed or multiline input, and zeroizes its fixed buffer. The existing credential facade is forced to OS keychain with file fallback disabled, even if a development environment override selects a file backend. It preserves migration/tombstone handling, clears a stale legacy entry through the existing facade, and verifies the saved value without displaying it. `missing` exits 1 and does not mean an existing saved key was deleted.

On Bash, a hidden shell prompt can feed the same stdin-only example without putting the value into history or an environment variable:

```bash
IFS= read -r -s -p 'TypeSafe API key: ' credential
printf '\n'
printf '%s\n' "$credential" |
  cargo +1.95.0 run --quiet --locked --no-default-features --features cloud --example save_typesafe_credential
unset credential
```

### Credential and consent threat boundary

The API key is restricted data. Private session text is confidential. Scope is this evaluation CLI and its existing credential facade, not product routing or protection against a compromised OS user account.

```text
[Human hidden prompt] -- stdin --|process boundary|--> [credential CLI]
[credential CLI] --|OS credential boundary|--> [keychain]
[Explicit private consent] --> [replay CLI] <-- [local session store]
[replay CLI] -- TLS --|provider boundary|--> [Jev]
```

| Component | STRIDE | Threat | Risk | Mitigation and status |
| --- | --- | --- | --- | --- |
| Human terminal and CLI | Information disclosure, spoofing | Key appears in chat, history, argv, or terminal echo | High | Hidden terminal prompt, piped stdin only, no accepted arguments, fixed saved/missing output. Mitigated. |
| Stdin reader | Tampering, denial of service | Malformed or oversized input overwrites a saved value or allocates unbounded memory | Medium | Bounded zeroizing buffer; validation completes before a write. Mitigated. |
| Credential facade | Information disclosure, elevation of privilege | Key silently lands in plaintext fallback or a stale entry masks the saved value | High | Explicit OS-only mode, existing migration/tombstone cleanup, saved-value verification. Mitigated. |
| Replay CLI | Spoofing, elevation of privilege | Private content is read or exported without an explicit approval step | High | `--allow-private-session` before loading a session; `--allow-cloud` for cloud transport. Private runs are not authorized in this evaluation. Mitigated at the CLI boundary. |
| Local credential changes | Repudiation | No detailed audit identifies which local process supplied a key | Low | OS-user identity and presence-only output. Accepted; secret-bearing audit logs are out of scope. |
| Terminal/OS process memory | Information disclosure | A same-user attacker reads temporary plaintext while the key is passed to stdin | High | Rust buffer and unmanaged BSTR are zeroized. Remaining runtime/OS copies are accepted under the trusted-host assumption. |

Assumptions: the human's terminal and OS account are trusted, the existing platform keychain enforces its access controls, TLS verification stays enabled, and the selected evaluation fixtures contain reviewed public text. Arbitrary fixture files must not be assumed public. Revisit these boundaries if the CLI input, storage backend, or session-export workflow changes.

### Public fixture evaluation

```sh
cargo +1.95.0 run --locked --no-default-features --features cloud --example system_one_eval -- --help
cargo +1.95.0 run --locked --no-default-features --features cloud --example system_one_eval -- --endpoint jev --allow-cloud
cargo +1.95.0 run --locked --no-default-features --features cloud --example system_one_eval -- --endpoint jev --allow-cloud --fixture fixtures/decision/public_replay.json
cargo +1.95.0 run --locked --no-default-features --features cloud --example system_one_eval -- --endpoint ollama --model clef-flash --fixture fixtures/decision/public_replay.json
```

A real session's text is not loaded unless the caller supplies `--allow-private-session` after explicit user approval. Sending it to a cloud provider also requires `--allow-cloud`. No private-session evaluation is authorized or run here. Initial evaluation uses the committed public fixtures only.

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
