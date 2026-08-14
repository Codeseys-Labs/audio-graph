# audio-graph-98ef implementation report

Date: 2026-08-14

Seed: `audio-graph-98ef`

Parent: `audio-graph-ada2`

Branch: `work/98ef-stt-readiness-wave5`

Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/98ef-stt-readiness-wave5`

Exact base: `2dd2a02883df4b4e254913e3fe9eaf4473127dea`

## Outcome

Published separate static and effective STT fidelity contracts without changing
provider enablement. The provider registry now carries static declarations for
revision semantics, timing fidelity, confidence, turn, speaker, and channel
evidence. `ProviderReadiness` separately carries selected-configuration
effective fidelity plus typed degradation reasons and provider-neutral turn
signals.

Healthy final-only STT remains operationally `ready`; its reduced transcript
fidelity is represented only in `effective_stt_fidelity.degradations`.
Readiness status is never changed because a provider is final-only, uses
app-estimated timing, or lacks optional evidence.

No ASR adapter, speech worker, fixture, React/settings presentation, provider
selection, workflow, credential store, or Seed file was changed.

## Contract and invariants

- `ProviderDescriptor.stt_fidelity` is static registry-owned maximum
  capability metadata. Every ASR descriptor has it; every non-ASR descriptor
  omits it. Roadmap capabilities that are not proven remain typed `unverified`
  instead of being guessed.
- `ProviderReadiness.effective_stt_fidelity` exists only for the actively
  selected ASR provider. It is value-free capability metadata and never
  replaces authoritative Speech Span Revision v2 evidence.
- Fidelity vocabulary aligns with v2 origins: `unavailable`, `app`, and
  `provider`, plus `unverified` for capability metadata. Timing distinguishes
  unavailable, app-estimated, provider-coarse, provider-exact, and unverified.
- Degradation reasons are a closed enum. No free-form transcript, speaker
  label, provider response, endpoint body, credential, or captured content can
  enter the fidelity diagnostic payload.
- The OpenAI-compatible final-only path demonstrates ready-but-degraded:
  final-only revisions, app-estimated timing, and unavailable
  confidence/turn/speaker/channel evidence remain a `ready` health result.
- Deepgram Nova effective fidelity follows selected diarization, speaker-cap,
  VAD, endpointing, and utterance-end settings. It applies the authoritative
  global `settings.diarization` policy through the same resolver startup uses,
  so global off plus provider on reports speaker unavailable and degraded. A
  speaker cap reports app-owned remapping rather than provider-owned speaker
  fidelity.
- Deepgram Flux follows the actual selected model path: current turn-based
  output is final-only at the transcript seam, Flux turn signals are provider
  owned, eager-end and turn-resume are enabled only for a valid
  `0 < eager_eot_threshold <= eot_threshold` pair, and diarization is reported
  unavailable for the selected Flux path rather than inferred from transcript
  events.
- Deepgram static and effective channel fidelity is unavailable. The
  contradiction guard ties that declaration to the mono `channels=1`,
  `supports_multichannel=false`, and no-channel-label runtime contract instead
  of claiming attribution that the adapter cannot emit.
- Deepgram readiness cache fingerprints now include only non-secret selected
  fidelity controls, including the global diarization mode, speaker-count
  policy, and maximum-speaker input. Config changes cannot reuse a stale
  effective-fidelity result, and no API key enters the fingerprint or response.
- TypeScript has exact static/effective unions plus a compile guard proving a
  healthy final-only result can be ready and degraded while transcript-derived
  origins and arbitrary degradation strings are rejected.

## No-promotion proof

The exact source block for `MVP_SELECTABLE_PROVIDERS` was compared between the
base commit and the final working tree. Both blocks have SHA-256:

```text
2b720a614612e4aaaced522f88ba62b74290c3ee9a7d4d8e7c5bbaabf20edaa5
```

The sorted provider-id set also matches base/current exactly; both set hashes
are:

```text
146c2f405826cdd083a8f67268407d6a63c00421fe5c8eb02ae1073fdc3f359f
```

The single existing registry authority test derives the selectable set from
descriptor flags and compares it with `MVP_SELECTABLE_PROVIDERS`. It passed.
The review correction removed the redundant complete literal table rather than
creating a second code authority. No provider was promoted.

## TDD evidence

Provider-registry RED, before the static contract existed:

```text
error[E0609]: no field `stt_fidelity` on type `&'static ProviderDescriptor`
error[E0433]: use of undeclared type `SttRevisionSemantics`
error[E0433]: use of undeclared type `SttTimingFidelity`
error[E0433]: use of undeclared type `SttProviderEvidence`
error: could not compile `audio-graph-provider-registry` due to 15 previous errors
```

Generated TypeScript RED, before regeneration:

```text
FAIL  src/generated/providerRegistry.test.ts > GENERATED_PROVIDER_REGISTRY > publishes static STT fidelity without changing MVP selectability
AssertionError: expected undefined to deeply equal { ... }
Test Files  1 failed (1)
Tests  1 failed | 18 passed (19)
```

The readiness tracer was written before the resolver. A final mutation
sensitivity RED proved the public selected-config assertion detects a false
Nova turn-origin downgrade:

```text
assertion `left == right` failed
  left: Provider
 right: Unavailable
test commands::tests::deepgram_effective_fidelity_uses_selected_model_diarization_and_turn_controls ... FAILED
test result: FAILED. 0 passed; 1 failed; 1575 filtered out
```

Review correction RED for authoritative global diarization policy:

```text
assertion `left == right` failed: global=Off provider_enabled=true
  left: Provider
 right: Unavailable
test commands::tests::deepgram_effective_speaker_fidelity_follows_global_and_provider_diarization_policy ... FAILED
test result: FAILED. 0 passed; 1 failed; 1576 filtered out
```

Review correction RED for readiness-cache invalidation:

```text
assertion `left != right` failed
  left: "model=nova-3|diarization=true|...|max_speakers=0"
 right: "model=nova-3|diarization=true|...|max_speakers=0"
test commands::tests::deepgram_readiness_fingerprint_tracks_global_diarization_policy ... FAILED
test result: FAILED. 0 passed; 1 failed; 1577 filtered out
```

Review correction RED/GREEN for the mono-channel contradiction:

```text
RED registry guard: left Provider, right Unavailable
RED effective guard: ChannelUnavailable degradation missing
GREEN registry contradiction guard: 1 passed; 0 failed
GREEN Deepgram effective/fingerprint group: 3 passed; 0 failed
```

Final focused GREEN:

```text
provider registry: 23 passed; 0 failed
healthy_final_only_stt_readiness_is_ready_but_typed_degraded: 1 passed; 0 failed
Deepgram selected model/global policy effective fidelity: 2 passed; 0 failed
Deepgram global-policy fingerprint invalidation: 1 passed; 0 failed
generated provider registry: 19 passed; 0 failed
```

## Files

- `src-tauri/crates/provider-registry/src/lib.rs`
- `src-tauri/src/commands.rs`
- `src/generated/providerRegistry.ts`
- `src/generated/providerRegistry.test.ts`
- `src/types/index.ts`
- `src/types/providerReadiness.typecheck.ts`
- this report

## Gates and real results

- `cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml -p audio-graph-provider-registry -- --nocapture`
  - PASS: 23 passed, 0 failed; doc-tests passed.
- Focused locked cloud readiness tests
  - PASS: final-only 1 passed, 0 failed; Deepgram 1 passed, 0 failed.
- `bun run check:provider-registry`
  - PASS: generated provider registry current.
- `bun run test -- src/generated/providerRegistry.test.ts`
  - PASS: 19 passed, 0 failed.
- `bun run typecheck`
  - PASS, including the readiness contract compile guard.
- `bun run check`
  - PASS: 174 files checked, no fixes.
- `bun run verify:contracts`
  - PASS: audio-source, provider-registry, session-data-movement,
    endpoint-credential-routing, and speech-span contracts current.
- `cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`
  - PASS in 2 minutes 3 seconds.
- Full direct locked cloud library suite, serialized exactly once after the
  final production implementation:
  - PASS: 1,570 passed, 0 failed, 8 ignored in 38.79 seconds.
- `cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud -- -D warnings`
  - PASS in 38.26 seconds.
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check`
  - PASS with no diagnostics.
- Exact base/current `MVP_SELECTABLE_PROVIDERS` byte and sorted-set comparison
  plus SHA-256
  - PASS: byte-identical and set-identical; hashes are the values above.
- `git diff --check 2dd2a02883df4b4e254913e3fe9eaf4473127dea --`
  - PASS.
- `bun scripts/check-docs-secret-hygiene.mjs`
  - PASS: 0 findings.
- `betterleaks dir --no-banner --redact <touched files and report>`
  - PASS: approximately 1.11 MB scanned, no leaks found.

## Findings and open questions

- No in-scope implementation blocker remains.
- The parallel adapter child `audio-graph-48de` reported a design blocker to
  the conductor. This readiness contract is intentionally independent, but
  adapter activation and per-span v2 production evidence still require that
  child or a revised follow-up. This work does not claim adapter activation.
- Static roadmap-provider attributes remain `unverified` when this repository
  has no implementation evidence. Provider promotion should replace those
  values only with cited provider/runtime proof; it must not infer them from a
  successful readiness probe.

## Rollback

The change is metadata-only and reader/additive at IPC. Rollback removes the
new registry/readiness fields and generated TypeScript declarations; it does
not rewrite session artifacts, settings, credentials, or the selectable
provider set.
