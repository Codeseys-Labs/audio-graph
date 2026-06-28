# ADR-0022: Codec/Decode Boundary — Keep Realtime PCM Codec-Free; Adopt symphonia Only at the Fixture/Import Edge

## Status

accepted (gated) — recorded 2026-06-28

This ADR records a **boundary decision plus a gated adoption**, not an
unconditional dependency add. Today the repository carries **zero** audio-codec
crates and every realtime and fixture path operates on linear PCM
(`src-tauri/Cargo.toml` has no `symphonia`, `audiopus`, `opus`, or `hound`
entry; the only `symphonia` string in the whole tree is a logging allowlist at
`src-tauri/src/logging/mod.rs:118`). The honest verdict is to ratify the
codec-free realtime posture as a hard invariant now, and to pre-commit the
narrow conditions under which a *single* decode dependency (`symphonia`) may
enter — at the offline fixture/import edge only — rather than to add a codec
crate speculatively or to forbid one that two named fixture seeds will need.

This decision was triggered by the Rust-ecosystem review follow-up
(seed `audio-graph-175e`): *"Realtime pipeline should stay linear PCM unless a
provider or replay/import feature requires codecs."* It is advisory-sequenced
ahead of the WAV-import path in seed `audio-graph-098b` and consumed by the
fixture set in seed `audio-graph-c237` (see *More Information* for the lane seam
doc that orders these).

## Context and Problem Statement

AudioGraph is a local-first Tauri desktop app (Windows/macOS/Linux) whose audio
spine is uniformly **uncompressed linear PCM**, end to end:

- **Capture** delivers interleaved `f32` samples
  (`src-tauri/src/audio/capture.rs:48` — `pub data: Vec<f32>` "Interleaved f32
  sample data"; the device path requests an `f32` sample type at
  `capture.rs:112`).
- **The processing pipeline** downmixes/resamples to a single canonical bus —
  "the processed 16 kHz mono bus" (`src-tauri/src/audio/pipeline.rs:4`) carrying
  `pub data: Vec<f32>` (`pipeline.rs:29`) at
  `PROCESSED_AUDIO_SAMPLE_RATE_HZ = 16_000` (`pipeline.rs:36`). This shape is the
  ratified product contract of ADR-0020 (Processed PCM And Timing Contract):
  normalized mono `f32`, finite samples only.
- **The mixer** sums "16 kHz mono f32" streams and explicitly does "no
  resampling, no channel logic" (`src-tauri/src/audio/mixer.rs:6`), operating on
  `VecDeque<f32>` ring buffers (`mixer.rs:38`).
- **Playback** drains a lock-free `ringbuf::HeapRb<i16>`
  (`src-tauri/src/playback/mod.rs:21`) and converts to the host sample format in
  the cpal callback (`playback/mod.rs:25`) — no codec decode is in the audible
  path.
- **TTS** deliberately restricts its wire encodings to the streaming-compatible
  PCM set. `TtsEncoding` exposes only `Linear16`, `Mulaw`, `Alaw`
  (`src-tauri/src/tts/mod.rs:56`), and the doc comment is explicit: REST-only
  "formats (mp3, opus, flac, aac) are intentionally absent — they don't fit the
  streaming AudioChunk model and would invite confused configurations"
  (`src-tauri/src/tts/mod.rs:47`).

The current audio dependency surface is small and codec-free: `rubato = "3.0"`
(`src-tauri/Cargo.toml:133`, resampling), `cpal = "0.17"` (`Cargo.toml:141`,
device I/O), `ringbuf = "0.5"` (`Cargo.toml:142`, SPSC buffer), and the `rsac`
capture library (`Cargo.toml:82`). None of these decode a compressed container.

Two pressures push on this codec-free spine:

1. **Imported / replay audio fixtures.** The repo already checks in WAV fixtures
   for source-separation bakeoffs (`src-tauri/fixtures/source_separation/`,
   seed `audio-graph-c237`) and validates them with a **hand-rolled WAV header
   parser** — `fn parse_wav_info(path: &Path) -> WavInfo`
   (`src-tauri/src/source_separation_fixtures.rs:327`) walks RIFF/`fmt `/`data`
   chunks by hand (`source_separation_fixtures.rs:330`–`391`). Two more fixture
   consumers are coming: seed `audio-graph-098b` (playback-reference echo
   harness) will need a WAV-import path to load render references, and
   `audio-graph-c237`'s fixtures must "run offline on macOS/Windows/Linux CI
   without private meeting audio"
   (`src-tauri/fixtures/source_separation/README.md:5`). The open question is
   whether to keep growing the hand-rolled parser or adopt a real decode crate
   (`symphonia`, MPL-2.0) at this edge.

2. **Provider transport.** Some ASR/TTS/realtime providers *can* speak
   compressed formats (Opus, mp3). The question is whether AudioGraph must
   *decode* any of them today — which would require `audiopus`/`opus` (libopus,
   BSD/3-clause) bindings.

The decision must answer four coupled sub-questions while keeping the realtime
spine sacred: **(a)** decode crate vs. hand-rolled parsing for fixtures;
**(b)** Opus/libopus bindings for provider transport; **(c)** the realtime-PCM
guardrail; **(d)** the fixture plan if `symphonia` is adopted.

## Decision Drivers

- **Realtime audio quality is non-negotiable.** No decode/encode work may enter
  the capture→pipeline→mixer→playback hot path; it risks dropouts and breaks the
  ADR-0020 `f32`/16 kHz contract.
- **Licensing must stay audit-clean for a shipped desktop binary.** MPL-2.0
  (`symphonia`) is file-level (weak) copyleft; libopus is BSD-3. Both are
  redistribution-friendly, but the *scope* of any copyleft obligation must be
  stated, not assumed.
- **Minimal dependency surface.** Every crate added to a cross-platform Tauri
  binary is a supply-chain, build-time, and binary-size cost.
- **Offline, three-OS CI.** Fixtures must decode deterministically with no
  network and no system codecs (`fixtures/source_separation/README.md:5`).
- **Real use case before dependency.** The 175e acceptance criterion is explicit:
  "no codec dependency enters realtime capture/playback without a real provider
  or fixture use case."
- **Correctness of fixture decoding.** A hand-rolled parser silently accepts a
  narrow WAV subset; a real decoder handles malformed/edge-case inputs and
  non-WAV containers a fixture author might check in.

## Considered Options

This ADR makes one boundary decision with four sub-decisions. Each sub-decision
lists ≥2 genuinely considered options.

### (a) Fixture/import decode strategy

- **A1. Adopt `symphonia` (MPL-2.0) at the fixture/import edge only** — a
  dev/test- or import-scoped decode crate behind the existing seam, replacing the
  hand-rolled parser as fixtures grow.
- **A2. Keep hand-rolled WAV parsing** (`source_separation_fixtures.rs:327`) and
  extend it as needed.
- **A3. Add `hound`** (a small WAV-only crate, ISC/Apache-2.0) instead of the
  broader `symphonia`.

### (b) Opus/libopus for provider transport

- **B1. Add `audiopus`/`opus` (libopus, BSD-3) now** to be ready for a
  compressed provider transport.
- **B2. Do not add any Opus dependency** until a concrete provider requires
  decode/encode of a compressed frame.

### (c) Realtime-PCM guardrail

- **C1. Codify "no codec dependency in realtime capture/playback" as a hard
  invariant** that a new dependency may not cross without a named provider/fixture
  use case.
- **C2. Leave it as informal convention** (status quo — enforced only by the
  `TtsEncoding` comment and reviewer vigilance).

### (d) Fixture plan if `symphonia` is adopted

- **D1. Document a concrete edge-scoped fixture/import plan** (where the decode
  call lives, what 098b/c237 consume, what stays codec-free).
- **D2. Defer the plan** until the first import feature is actually built.

## Decision Outcome

**(a) Chosen: A1 (gated) — adopt `symphonia` (MPL-2.0) at the fixture/import edge
only, when the first non-trivial import/replay consumer lands.** The hand-rolled
parser (`source_separation_fixtures.rs:327`) is correct for the current
single-format manifest check (PCM s16le, 16 kHz mono —
`source_separation_fixtures.rs:131`–`134`) and stays until then; but 098b's
render-reference import and a growing fixture set will outgrow a 60-line RIFF
walker, and a real decoder is the right tool at that point. Rejected A2 as the
*permanent* answer (it does not scale to varied containers and silently mis-reads
malformed input — it asserts a 44-byte minimum and a literal chunk walk,
`source_separation_fixtures.rs:330`); rejected A3 because `hound` is WAV-only and
unmaintained relative to `symphonia`, so it would have to be replaced the moment
a fixture is checked in as FLAC/Ogg, whereas `symphonia` covers WAV/FLAC/Ogg/MP3
behind one API and one license review.

**(b) Chosen: B2 — do not add any Opus/libopus dependency now.** No shipping or
planned provider requires AudioGraph to decode or encode a compressed frame: the
TTS surface intentionally excludes mp3/opus/flac/aac (`tts/mod.rs:47`), the
processed bus is `f32` PCM (`pipeline.rs:29`), and playback is `i16` PCM
(`playback/mod.rs:21`). Rejected B1: adding `audiopus`/`opus` now would violate
the "real use case first" driver and the 175e acceptance criterion, buying a
native-build/link cost and a supply-chain entry for a capability nothing calls.

**(c) Chosen: C1 — codify the realtime-PCM guardrail as a hard invariant.** No
codec/decode/encode dependency may be linked into the capture
(`capture.rs:48`), pipeline (`pipeline.rs:29`,`:36`), mixer (`mixer.rs:6`), or
playback (`playback/mod.rs:21`) modules without a named provider or fixture use
case recorded in a superseding ADR. `symphonia`, if adopted per (a), lives at the
fixture/import edge and its decoded output enters the system **only** as PCM that
already satisfies the ADR-0020 contract — it never appears in a realtime module's
dependency graph. Rejected C2: an informal convention is exactly what let the
question drift until a review had to raise it; an invariant is cheap to state and
gives future reviewers a bright line.

**(d) Chosen: D1 — document the edge-scoped fixture/import plan now** (below), so
098b and c237 build against a known boundary rather than inventing ad-hoc parsing.

### Consequences

- **Positive**: The realtime audio path stays provably codec-free and keeps the
  ADR-0020 `f32`/16 kHz contract; no decode latency or failure mode enters
  capture/pipeline/mixer/playback.
- **Positive**: Zero new dependencies land *today* — the codec-free `Cargo.toml`
  surface (`rubato`/`cpal`/`ringbuf`/`rsac`) is unchanged until a real import
  consumer exists.
- **Positive**: A single, license-reviewed decode crate (`symphonia`) is
  pre-blessed for the fixture/import edge, so 098b/c237 do not each hand-roll a
  parser or independently re-litigate the licensing question.
- **Positive**: The libopus question is settled with a clear re-open trigger,
  preventing speculative native-binding churn.
- **Negative**: When `symphonia` is adopted it introduces an **MPL-2.0** crate.
  MPL-2.0 is *file-level* copyleft: modifications to `symphonia`'s own source
  files must be released under MPL, but it imposes **no obligation on the rest of
  AudioGraph's source** and is compatible with redistributing a closed/own-license
  binary that merely links it. The cost is a standing obligation to publish any
  patches we make to `symphonia` files and to keep the dependency auditable in the
  supply-chain review. (This is a real, if modest, new constraint.)
- **Negative**: Until adoption, the hand-rolled parser remains a maintenance
  liability — it accepts only canonical PCM WAV and will reject or mis-handle any
  fixture authored as FLAC/Ogg or with unusual chunk ordering
  (`source_separation_fixtures.rs:341`–`383`), and it is not shared with any
  runtime import path.
- **Negative**: The guardrail adds a process step — adding a codec crate now
  requires a superseding ADR with a named use case, slowing any future provider
  that genuinely needs Opus.
- **Neutral**: `symphonia` already appears in the logging noisy-target allowlist
  (`src-tauri/src/logging/mod.rs:118`); that string is forward-looking and does
  **not** imply the crate is linked today (it is not — `Cargo.toml` has no
  `symphonia` dep). Adoption per (a) would make that allowlist entry live.

## Pros and Cons of the Options

### A1. Adopt `symphonia` at the fixture/import edge (gated)

- Good, because one crate decodes WAV/FLAC/Ogg/MP3 behind a single API, so the
  fixture set can grow beyond canonical PCM WAV without a new dependency each
  time.
- Good, because it is a pure-Rust decoder (no system codec, no network), which
  satisfies offline three-OS CI (`fixtures/source_separation/README.md:5`).
- Good, because gating adoption to "first real import consumer" honors the 175e
  "real use case first" criterion while still pre-blessing the choice.
- Bad, because MPL-2.0 file-level copyleft adds a standing obligation to publish
  patches to `symphonia`'s own files and a supply-chain audit entry.
- Bad, because it is a larger crate than the problem needs *today* (the current
  manifest check is single-format).

### A2. Keep hand-rolled WAV parsing

- Good, because it adds zero dependencies and is sufficient for the current
  single-format manifest assertion (`source_separation_fixtures.rs:131`–`134`).
- Good, because the code is fully under our control and trivially auditable.
- Bad, because it is WAV-only and silently fragile: it assumes ≥44 bytes
  (`source_separation_fixtures.rs:330`) and a literal `fmt `/`data` chunk walk
  (`:356`–`:380`); a FLAC/Ogg fixture or unusual layout would panic or mis-read.
- Bad, because it is test-only and not reusable by 098b's runtime WAV-import
  path, so that seed would duplicate parsing logic.

### A3. Add `hound` (WAV-only)

- Good, because it is tiny and battle-tested for canonical WAV I/O.
- Bad, because it is WAV-only — the first FLAC/Ogg fixture forces a second
  decode crate and a second license review, defeating the point.
- Bad, because it is comparatively low-activity; adopting it now risks adopting
  `symphonia` later anyway (two migrations instead of one).

### B1. Add `audiopus`/`opus` (libopus, BSD-3) now

- Good, because BSD-3 is permissive (no copyleft) and would be ready if a
  compressed provider transport ever lands.
- Bad, because nothing decodes/encodes Opus today (`tts/mod.rs:47`;
  `pipeline.rs:29`; `playback/mod.rs:21`), so it is a speculative dependency.
- Bad, because libopus is a C library — it adds a native build/link burden across
  all three OS targets for an unused capability.

### B2. No Opus dependency until a provider requires it

- Good, because it keeps the dependency surface minimal and honors "real use case
  first."
- Good, because the re-open trigger is explicit and cheap (a provider PR that
  needs Opus writes a superseding ADR).
- Bad, because a future Opus-only provider transport would be blocked on that ADR
  + the binding work, adding latency to that one feature.

### C1. Hard realtime-PCM guardrail

- Good, because it gives reviewers a bright line: a codec crate in
  capture/pipeline/mixer/playback is rejected absent a named use case + ADR.
- Good, because it protects the ADR-0020 contract structurally, not by habit.
- Bad, because it is process overhead for any future realtime codec need.

### C2. Informal convention

- Good, because it has zero process cost.
- Bad, because it is exactly what let this question drift until review had to
  raise it; conventions are invisible to new contributors and bots.

### D1. Document the fixture/import plan now

- Good, because 098b/c237 build against a defined edge instead of ad-hoc parsing.
- Bad, because the plan may need minor revision once the first importer is real
  (acceptable: it is advisory, not a frozen API).

### D2. Defer the plan

- Good, because it avoids writing a plan that might change.
- Bad, because it leaves 098b free to commit to a hand-rolled WAV-import path the
  seam doc explicitly wants this ADR to land ahead of.

## Imported-Audio Fixture/Import Plan (sub-decision d)

If and when `symphonia` is adopted (trigger: the first import/replay consumer —
expected to be seed `audio-graph-098b`'s render-reference loader):

1. **Where it lives.** Decode happens in a dedicated fixture/import module
   (e.g. `src-tauri/src/audio/import.rs` or a `fixtures` test-support module),
   **never** inside `capture.rs`, `pipeline.rs`, `mixer.rs`, or `playback/`. The
   `symphonia` crate appears in that module's dependency graph only.
2. **Output shape.** The decoder's only job is to produce PCM that already
   satisfies the ADR-0020 contract before it reaches any consumer: mono `f32`,
   16 kHz, finite samples (`pipeline.rs:29`,`:36`). Resampling, if needed, reuses
   `rubato` (`Cargo.toml:133`) — the same path the live pipeline uses — so there
   is one resampling implementation, not two.
3. **What 098b consumes.** The playback-reference echo harness loads a render
   reference (assistant playback audio) from a checked-in WAV/compressed file via
   this module, timestamps it, and aligns it with mic/system capture — keeping it
   "before the canonical 16 kHz ASR bus rather than mutating ASR chunks directly"
   (seed `audio-graph-098b` acceptance).
4. **What c237 consumes.** The source-separation fixture validator
   (`source_separation_fixtures.rs:258`) migrates from the hand-rolled
   `parse_wav_info` (`:327`) to the shared decode module, so a fixture author may
   check in FLAC/Ogg ground-truth clips while CI still runs offline on all three
   OSes (`fixtures/source_separation/README.md:5`).
5. **What stays codec-free.** The realtime spine. The decode module's output is
   PCM; nothing compressed crosses into `capture`/`pipeline`/`mixer`/`playback`.
6. **Feature scoping (optional).** Per ADR-0007's precedent of gating heavy
   capability behind Cargo features, `symphonia` may sit behind a non-default
   feature if it proves to add meaningful binary size, so default builds and
   realtime paths never link it.

## More Information

- **Triggering seed:** `audio-graph-175e` — "Decide codec/decode boundary for
  imported audio and provider Opus support."
- **Consuming seeds:** `audio-graph-098b` (playback-reference echo fixture
  harness; this ADR is advisory-sequenced to land before 098b commits to a
  WAV-import path) and `audio-graph-c237` (ground-truth overlapping-speech
  fixture set; child of `audio-graph-dd19`).
- **Lane seam doc:** `docs/reviews/_triage-2026-06-27/lane-audio-diarization-fixtures.md`
  orders this ADR ahead of 098b's WAV-import path (advisory, not a hard
  dependency).
- **Related ADRs:** ADR-0020 (Processed PCM And Timing Contract) defines the
  `f32`/16 kHz bus this ADR protects; ADR-0004 (TtsProvider trait) owns the
  `TtsEncoding` PCM-only surface (`tts/mod.rs:47`,`:56`); ADR-0007 (feature-gate
  local ML) sets the precedent for the optional-feature scoping in plan step 6.
- **Re-open triggers:** a provider transport that genuinely requires compressed
  decode/encode (re-opens sub-decision **b** via a superseding ADR), or a
  realtime feature that genuinely requires in-path codec work (re-opens
  sub-decision **c**).
- **License notes:** `symphonia` is MPL-2.0 (file-level copyleft — obliges
  releasing modifications to its own files, imposes nothing on the rest of the
  binary or its source); libopus (`audiopus`/`opus`) is BSD-3 (permissive, no
  copyleft) — recorded here so a future Opus adopter does not re-research it.
