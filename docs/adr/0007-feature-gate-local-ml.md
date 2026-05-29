# ADR-0007: Gate local ML inference behind cargo feature flags

## Status

Accepted + implemented 2026-05-29 (proposed 2026-05-28). Implemented as
**Option B** (opt-out): `default = ["local-ml"]`, with a cloud-only build via
`cargo build --no-default-features --features cloud`.

### Implementation outcome

- `whisper-rs`, `llama-cpp-2`, `mistralrs` are now `optional = true`; features
  `local-ml` (default) = `asr-whisper` + `llm-llama` + `llm-mistralrs`. `cuda`/
  `vulkan` use weak refs (`whisper-rs?/cuda`). macOS metal deps kept optional.
- The engines (`LlmEngine`, `MistralRsEngine`, whisper `AsrWorker` + the speech
  `run_asr_worker`) keep their public types/APIs via `#[cfg]` real-vs-stub
  pairs, so all `Option<Engine>` state plumbing and `LlmProvider`/`AsrProvider`
  match arms compile unchanged; selecting a local provider in a cloud build
  returns a clear "not included in this build" error / logs and degrades
  gracefully (Local Whisper drains its queue and logs instead of building).
- Verified on Windows: **both** `cargo check` (default, ML on) and
  `cargo check --no-default-features --features cloud` (ML off) compile
  warning-clean. The cloud build omits whisper.cpp/llama.cpp/mistral.rs (no
  cmake/ggml compile) → substantially faster build + smaller binary.

### Correction: this does NOT fix the Windows test harness

The earlier hypothesis (this ADR + the deep-critique review) that the ML libs
caused the `cargo test` `STATUS_ENTRYPOINT_NOT_FOUND` was **wrong**. Verified:
`cargo test --no-default-features --features cloud` (zero ML libs linked) still
aborts with `0xC0000139`. So the test-harness failure comes from other native
deps (aws-lc-sys / ring / OpenMP cmake builds), not whisper/llama/mistralrs.
The standalone runner (`scripts/run-core-tests.ps1`) remains the way to run
ML-free logic tests on Windows.

## Context

The operative goal of the 2026-05-28 loop is: a user on Windows can build,
launch, enter cloud API keys (Deepgram STT + OpenRouter LLM + Deepgram Aura
TTS), and start working. That path requires **no local ML models** at runtime
(verified — Deepgram STT pre-flight only checks the key; OpenRouter chat and
Aura TTS are network calls).

But the *build* still compiles the full native ML stack unconditionally.
`src-tauri/Cargo.toml` declares these as non-optional dependencies:

- `whisper-rs = "0.16.0"` (line 116) — whisper.cpp, C++/CMake
- `llama-cpp-2 = "0.1.139"` (line 128) — llama.cpp, C++/CMake
- `mistralrs = "0.8"` (line 136) — Candle + gemm + a very large dep tree

On this machine a clean `cargo check` is ~5 min and a debug `tauri build` is
~11.5 min, dominated by compiling these C++ / heavy-Rust crates. They also
impose the full toolchain requirement (CMake + MSVC C++ + LLVM) on every
contributor, including those who only ever use cloud providers.

This is the single largest friction point for the stated goal: "start getting
stuff working" should not require a ~12-minute native build of inference
engines the cloud user never invokes.

## Decision Drivers

- Cloud-only users (the primary near-term persona) should get a fast, light
  build with a minimal toolchain.
- Local-stack users (Whisper / llama.cpp / mistral.rs) must keep full
  functionality with no behavior change.
- The green build, the 148-test suite, and the existing provider-selection UX
  must not regress.
- Settings already enumerate providers; selecting a local provider when its
  feature is not compiled in must fail gracefully with a clear message, not a
  panic or a silent no-op.
- CI must continue to build the full-feature matrix so local paths stay tested.

## Considered Options

- **Option A — Feature-flag each engine, default to cloud-only.**
  Add `asr-whisper`, `llm-llama`, `llm-mistralrs` features (off by default).
  Wrap the `dep:` entries as `optional = true` and `#[cfg(feature = ...)]` the
  modules that call them. A new `local-ml` umbrella feature turns them all on.
  Default `cargo build` becomes cloud-only and fast.

- **Option B — Feature-flag each engine, default to ALL on (opt-out).**
  Same mechanics, but `default = ["local-ml"]`. Cloud users opt out with
  `--no-default-features`. Preserves today's behavior for the default build.

- **Option C — Status quo.** Keep all three non-optional. Document the long
  build as expected. No code change.

- **Option D — Split a separate `audio-graph-lite` binary/crate** that excludes
  local ML entirely. Two build targets.

## Decision Outcome

Proposed: **Option B (opt-out, `default = ["local-ml"]`) as the first step,
with a documented `--no-default-features --features cloud` fast path.**

Rationale: Option B is the lowest-regression-risk way to introduce the cfg
seams. The default build and CI behavior are unchanged (everything still
compiles, all tests still run), so we de-risk the refactor itself. Once the
cfg seams are proven and the graceful-degradation messaging is in place, a
follow-up can flip the default to cloud-only (Option A) as a deliberate,
separately-reviewed change.

Option A is the better *end state* for the stated goal but flipping the default
in the same change that introduces the seams couples two risks (mechanical
cfg-gating bugs + behavior change) into one hard-to-review diff. Sequence them.

Option C leaves the core friction unaddressed. Option D doubles the maintenance
and bundling surface for little gain over feature flags.

### Consequences

- **Positive**: `cargo build --no-default-features --features cloud` skips
  whisper.cpp + llama.cpp + mistral.rs entirely — minutes faster, and CMake /
  MSVC-C++ become optional for cloud-only contributors.
- **Positive**: The cfg seams document exactly which code paths are local-only.
- **Negative**: Every call site touching the three engines needs a
  `#[cfg(feature = ...)]` gate plus a compiled-out fallback that returns a
  structured "provider not available in this build" error. This touches
  `asr/`, `llm/` (engine.rs, mistralrs_engine.rs), `models/`, `speech/`, and
  the command pre-flight checks.
- **Negative**: CI must add a cloud-only build job (verify it compiles without
  the engines) in addition to the full-feature job.
- **Neutral**: Bundle/runtime behavior of the default build is unchanged until
  a later Option-A flip.

## Implementation outline (informational)

```toml
# Cargo.toml
[features]
default = ["local-ml"]
cloud = []                         # explicit cloud-only marker (no-op deps)
local-ml = ["asr-whisper", "llm-llama", "llm-mistralrs"]
asr-whisper = ["dep:whisper-rs"]
llm-llama   = ["dep:llama-cpp-2"]
llm-mistralrs = ["dep:mistralrs", "dep:schemars"]

# deps become optional:
whisper-rs  = { version = "0.16.0", optional = true }
llama-cpp-2 = { version = "0.1.139", optional = true }
mistralrs   = { version = "0.8", optional = true }
```

```rust
// provider pre-flight (commands.rs), graceful degradation:
match settings.asr_provider {
    #[cfg(feature = "asr-whisper")]
    AsrProvider::LocalWhisper => { /* existing model-file gate */ }
    #[cfg(not(feature = "asr-whisper"))]
    AsrProvider::LocalWhisper => return Err(AppError::provider_unavailable(
        "Local Whisper is not included in this build. Use a cloud ASR \
         provider, or rebuild with --features local-ml.")),
    /* cloud arms unchanged */
}
```

CI matrix:
- job `rust (full)`: `--features local-ml` (today's behavior; keeps local paths
  tested on Linux/Windows/macOS).
- job `rust (cloud)`: `--no-default-features --features cloud` (proves the
  cloud-only build compiles and tests pass without the engines).

## References

- `docs/commit-state-2026-05-28-runnable-windows.md`
- `docs/WINDOWS_QUICKSTART.md`
- seeds: AG-WIN-001 (filed this loop)
- `src-tauri/Cargo.toml` lines 116 / 128 / 136
