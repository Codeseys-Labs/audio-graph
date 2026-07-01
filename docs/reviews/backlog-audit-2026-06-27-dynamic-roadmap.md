# Backlog Audit - 2026-06-27 Dynamic Roadmap

## Scope

This audit is for backlog-zero continuation planning. It is intentionally
read-only against Seeds: `.seeds/issues.jsonl` was parsed directly but not
modified. `sd ready --format json` and `sd blocked --format json` were also
used, but the full inventory below comes from direct JSONL parsing because the
`sd ready` envelope only surfaced 50 ready rows while the JSONL contains 59
open unblocked rows.

No provider keys, temporary secrets, raw provider responses, or secret-bearing
logs are copied here.

## Repository State

- Branch: `master`
- HEAD: `831cc30101840db87bd2b502f2da749d65fe1c22`
- Seeds CLI: `sd 0.4.5`
- Dirty-tree caveat: the checkout was already broadly dirty before this audit.
  `git status --short` showed 191 rows, with 102 unstaged tracked paths, 75
  staged tracked paths, and a large untracked tree collapsed by status output.
  `.seeds/issues.jsonl` was already modified. Do not run `sd sync`, create a
  broad commit, or edit workflow/release files from this mixed checkout without
  a clean worktree/branch plan or explicit approval.

## Queue Reconciliation

Direct `.seeds/issues.jsonl` parse:

- Total Seeds: 307
- Open Seeds: 100
- Closed Seeds: 207
- Open priorities: P1 = 34, P2 = 46, P3 = 18, P4 = 2
- Open types: 9 epics, 41 features, 49 tasks, 1 bug

`sd` envelope check:

- `sd ready --format json`: 50 rows
- `sd blocked --format json`: 41 rows
- Direct JSONL unblocked rows absent from `sd ready`: 9 rows

Dependency notation in the table:

- `R`: surfaced by `sd ready`
- `B: ...`: surfaced by `sd blocked`; listed IDs are direct `blockedBy`
  dependencies with the `audio-graph-` prefix omitted
- `R*`: open and unblocked in direct JSONL, but not surfaced by `sd ready`

Primary bucket definitions:

- `Alpha-blocking`: local implementation, architecture, UX, privacy, or data
  contract work that blocks a credible alpha of the desktop speech-to-notes /
  transcript / temporal graph product.
- `Remote-evidence`: closure is mostly gated by external runners, OS matrix,
  provider smoke credentials, live-service evidence, or release/supply-chain
  validation. These can still be alpha-critical, but the next step is evidence
  collection from a clean path.
- `Product-expansion`: competitive or later product surface, realtime voice
  agent work, extra providers, integrations, or non-MVP UX depth.
- `Research-only/out-of-scope`: watchlists, architecture sessions, spikes, or
  deferred hygiene that should not displace alpha or remote-evidence work.

Complexity notation:

- `S`: single doc/test/policy decision or small hygiene item
- `M`: bounded implementation/evidence slice
- `L`: cross-module feature or contract work
- `XL`: epic or multi-lane integration surface

Summary by primary bucket:

- Alpha-blocking: 37
- Remote-evidence: 20
- Product-expansion: 30
- Research-only/out-of-scope: 13

Summary by lane:

- ASR/providers: 19
- Product/UX/trust: 17
- CI/release: 13
- S2S/LLM/TTS: 12
- Audio/source: 11
- Diarization/speaker: 10
- Credentials/settings: 8
- Transcript/graph: 7
- Storage/memory: 2
- Queue/hygiene: 1

## Top Execution Waves

1. Remote evidence and clean-worktree gates: run or record safe evidence for
   Blacksmith, OS keychain, Soniox live smoke, OpenRouter routed smoke, rsac
   live audio, optional Rust features, SurrealDB file-backed engines, and
   release workflow readiness. Candidate Seeds: `audio-graph-319c`,
   `audio-graph-0b93`, `audio-graph-0c08`, `audio-graph-0d58`,
   `audio-graph-b05b`, `audio-graph-74b2`, `audio-graph-fbf6`,
   `audio-graph-2586`, `audio-graph-0d66`, `audio-graph-fd9f`,
   `audio-graph-2b2c`, `audio-graph-8772`, `audio-graph-8e59`.
2. Provider harness and alpha ASR backbone: land the production ASR transport
   boundary, then unblock the reusable ASR fixture/session harness and provider
   roadmap. Candidate path: `audio-graph-b841` -> `audio-graph-d042` ->
   `audio-graph-02da`, `audio-graph-f0a3`, `audio-graph-e35f`,
   `audio-graph-ad1d`.
3. Credentials, settings, and provider readiness: finish saved-key health,
   config YAML migration, credential source labels, settings accessibility, and
   diagnostic redaction before closing the settings health epic. Candidate path:
   `audio-graph-559d`, `audio-graph-cbde`, `audio-graph-a6d4`,
   `audio-graph-ad98`, `audio-graph-a3d8` -> `audio-graph-1c2f`.
4. Audio/source foundation: advance processed-audio consumers, source selection
   states, ProcessTree contract tests, and channel provenance before higher
   level diarization or source-native multichannel work. Candidate path:
   `audio-graph-afca`, `audio-graph-1e47`, `audio-graph-7ee6`,
   `audio-graph-bfcb` -> `audio-graph-2044`, `audio-graph-a2ff`,
   `audio-graph-e864`.
5. Transcript, notes, graph, and speaker timeline: build the event-sourced data
   model and replay fixtures first, then unblock transcript revisions, session
   artifact migration, frontend reducers, diarization schema, and projection.
   Candidate path: `audio-graph-ad44` -> `audio-graph-4da5`,
   `audio-graph-eb6c`, `audio-graph-0d1c`, `audio-graph-20f2`,
   `audio-graph-1fbd`, `audio-graph-9c89`, `audio-graph-9d93`,
   `audio-graph-eebf`, `audio-graph-4673`.
6. Local model and voice-agent follow-through: after core alpha foundations and
   evidence gates, continue Moonshine, local TTS, playback resampling, local
   S2S, OpenAI Realtime, barge-in, and extra streaming LLM providers. Candidate
   Seeds: `audio-graph-0117`, `audio-graph-9279`, `audio-graph-14e0`,
   `audio-graph-f53b`, `audio-graph-1a8c`, `audio-graph-eee3`,
   `audio-graph-396f`, `audio-graph-b373`, `audio-graph-82b3`,
   `audio-graph-7fcc`.
7. Competitive product expansion: keep `audio-graph-b153` and children visible,
   but do not let later roadmap items preempt alpha blockers or evidence gates.
   Pull forward only tightly scoped alpha-usability items when they support
   time-to-first-note, privacy state, or live-assist basics.

## Open Seed Inventory

| Seed | P | Lane | Deps | Complexity | Bucket | Title |
| --- | ---: | --- | --- | --- | --- | --- |
| `audio-graph-0117` | P1 | ASR/providers | R | M | Alpha-blocking | Moonshine streaming worker and span-revision adapter |
| `audio-graph-0b93` | P1 | ASR/providers | B: 319c | M | Remote-evidence | Run Soniox env-gated live smoke and record redacted evidence |
| `audio-graph-0c08` | P1 | Credentials/settings | R | M | Remote-evidence | OS keychain credential backend and non-destructive import |
| `audio-graph-0d58` | P1 | CI/release | R | M | Remote-evidence | Blacksmith asr-moonshine feature compile matrix |
| `audio-graph-14e0` | P1 | ASR/providers | B: 0117, 9279 | L | Alpha-blocking | Moonshine streaming STT provider (local) |
| `audio-graph-1c2f` | P1 | Credentials/settings | B: 0162, cbde, 0c08, a3d8, a6d4, ad98 | XL | Alpha-blocking | Configuration UX and credential health center |
| `audio-graph-1fbd` | P1 | Diarization/speaker | B: ad44, eb6c, 20f2 | M | Alpha-blocking | Normalize provider diarization into speaker-span revisions |
| `audio-graph-2044` | P1 | Audio/source | B: 1e47, 7ee6 | XL | Alpha-blocking | Source descriptor and audio consumer bus refactor |
| `audio-graph-2586` | P1 | CI/release | R | M | Remote-evidence | Move release workflow to Blacksmith and pinned actions |
| `audio-graph-319c` | P1 | ASR/providers | R | S | Remote-evidence | Provision Soniox live-smoke credential through safe local path |
| `audio-graph-3588` | P1 | Diarization/speaker | B: dbac | XL | Alpha-blocking | Local streaming diarization and speaker timeline architecture |
| `audio-graph-4673` | P1 | Transcript/graph | B: ad44, 8e59, c395, 4da5, 9c89, 9d93, 0d1c | XL | Alpha-blocking | Streaming transcript to notes and temporal-graph diff pipeline |
| `audio-graph-4da5` | P1 | Transcript/graph | B: ad44 | L | Alpha-blocking | Transcript revision ledger and canonical span projection |
| `audio-graph-5011` | P1 | Diarization/speaker | B: afca, eb6c, b05b | L | Alpha-blocking | Local streaming diarization worker with flexible speaker counts |
| `audio-graph-74b2` | P1 | CI/release | R | M | Remote-evidence | Blacksmith Tauri build smoke matrix |
| `audio-graph-9279` | P1 | ASR/providers | B: 0117, 0d58 | M | Remote-evidence | Moonshine model downloader readiness and cross-platform validation |
| `audio-graph-9c89` | P1 | Transcript/graph | B: 4da5, ad44 | L | Alpha-blocking | Session artifact migration for transcript and projection events |
| `audio-graph-a2ff` | P1 | Audio/source | B: afca | M | Alpha-blocking | Provider audio policy registry: per-source vs mixed vs sessions |
| `audio-graph-a3d8` | P1 | Credentials/settings | B: 0c08 | M | Alpha-blocking | Settings credential source labels for keychain and fallback stores |
| `audio-graph-ad1d` | P1 | ASR/providers | B: d042, a805, e35f, f0a3, 226e, eb2e, 02da | XL | Alpha-blocking | Provider registry and streaming STT expansion roadmap |
| `audio-graph-ad44` | P1 | Transcript/graph | R | L | Alpha-blocking | Event-sourced transcript/notes/graph synthesis data model |
| `audio-graph-afca` | P1 | Audio/source | R | L | Alpha-blocking | Dynamic processed-audio consumer registry |
| `audio-graph-b05b` | P1 | CI/release | R | M | Remote-evidence | Diarization clustering feature compile and cross-platform smoke matrix |
| `audio-graph-b841` | P1 | ASR/providers | R | L | Alpha-blocking | Extract production ASR WebSocket transport/session boundary |
| `audio-graph-be03` | P1 | ASR/providers | B: 0b93 | M | Alpha-blocking | Promote Soniox to selectable ASR provider |
| `audio-graph-c395` | P1 | CI/release | B: 0162, 2586, 74b2, 8eeb, 0d66, fbf6, fd9f | XL | Remote-evidence | Cross-platform CI and Blacksmith release-readiness matrix |
| `audio-graph-cbde` | P1 | Credentials/settings | R | M | Alpha-blocking | Saved-credential health checks and model discovery on Settings open |
| `audio-graph-d042` | P1 | ASR/providers | B: b841 | L | Alpha-blocking | Reusable ASR provider transport and parser fixture harness |
| `audio-graph-e35f` | P1 | ASR/providers | B: 0b93, be03 | L | Alpha-blocking | Implement Soniox realtime STT provider |
| `audio-graph-eb6c` | P1 | Diarization/speaker | B: ad44 | M | Alpha-blocking | Speaker timeline event schema and replay fixtures |
| `audio-graph-eee3` | P1 | S2S/LLM/TTS | B: 1a8c, 14e0, f53b | XL | Product-expansion | Local/hybrid S2S pipeline: STT -> vLLM -> TTS turn orchestrator |
| `audio-graph-f0a3` | P1 | ASR/providers | R | M | Alpha-blocking | Upgrade AssemblyAI streaming to Universal-3.5 Pro Realtime/v3 |
| `audio-graph-f53b` | P1 | Audio/source | R | M | Product-expansion | Wire rubato output resampling into CPAL playback |
| `audio-graph-fbf6` | P1 | CI/release | R | M | Remote-evidence | Cross-platform optional Rust feature compile matrix |
| `audio-graph-0162` | P2 | Credentials/settings | R | M | Remote-evidence | Cross-platform provider setup UX validation |
| `audio-graph-02da` | P2 | ASR/providers | B: e35f, f0a3 | M | Alpha-blocking | Reusable streaming WebSocket ASR session harness |
| `audio-graph-09a7` | P2 | CI/release | B: 0d66 | S | Remote-evidence | Cross-platform release usability smoke runbooks |
| `audio-graph-0bdc` | P2 | Audio/source | R | M | Research-only/out-of-scope | VAD and AEC crate bakeoff for local turn detection and barge-in |
| `audio-graph-0d1c` | P2 | Transcript/graph | B: ad44 | M | Alpha-blocking | Supersede ADR-0014 with event-sourced notes and graph projection architecture |
| `audio-graph-0d66` | P2 | CI/release | R | M | Remote-evidence | Live rsac audio smoke tests on CI-capable runners |
| `audio-graph-1e47` | P2 | Audio/source | R | M | Alpha-blocking | Capability and permission gated source selection states |
| `audio-graph-20f2` | P2 | Diarization/speaker | B: eb6c | M | Alpha-blocking | Provider speaker and channel diarization parser fixtures |
| `audio-graph-226e` | P2 | ASR/providers | B: d042 | M | Product-expansion | Implement Gladia Solaria live runtime and registry readiness |
| `audio-graph-2b2c` | P2 | Storage/memory | R | M | Remote-evidence | Evaluate SurrealDB file-backed engines on Blacksmith before storage selectability |
| `audio-graph-2f4a` | P2 | S2S/LLM/TTS | R | M | Product-expansion | AwsBedrock ConverseStream adapter |
| `audio-graph-392b` | P2 | Product/UX/trust | R | L | Product-expansion | Live assistance agent triggers, question detection, and key-note capture |
| `audio-graph-396f` | P2 | S2S/LLM/TTS | R | XL | Product-expansion | Implement OpenAI Realtime gpt-realtime-2 cloud-native S2S provider |
| `audio-graph-48bb` | P2 | Storage/memory | B: 2b2c | M | Product-expansion | SurrealDB embedded local memory adapter spike |
| `audio-graph-51e0` | P2 | Product/UX/trust | R | M | Product-expansion | Session data route UI and privacy report |
| `audio-graph-53cf` | P2 | Product/UX/trust | R | L | Product-expansion | Calendar and prior-context pre-briefs from the temporal graph |
| `audio-graph-559d` | P2 | Credentials/settings | R | M | Alpha-blocking | Migrate user settings to config.yaml with settings.json import |
| `audio-graph-61db` | P2 | S2S/LLM/TTS | R | M | Product-expansion | OpenRouter accelerator catalog view model |
| `audio-graph-70a3` | P2 | Product/UX/trust | R | M | Product-expansion | Session data movement ledger and audit event schema |
| `audio-graph-75a1` | P2 | Product/UX/trust | R | M | Product-expansion | Time-to-first-note onboarding and sample session UX |
| `audio-graph-76bd` | P2 | S2S/LLM/TTS | R | M | Product-expansion | OpenRouter routed provider telemetry |
| `audio-graph-7ee6` | P2 | Audio/source | R | M | Alpha-blocking | Process vs ProcessTree contract tests across TS and Rust |
| `audio-graph-8181` | P2 | Product/UX/trust | R | M | Product-expansion | Competitive benchmark suite for notes, memory, and live assist quality |
| `audio-graph-82b3` | P2 | S2S/LLM/TTS | B: eee3 | M | Product-expansion | Deepgram Flux EagerEndOfTurn support with TurnResumed rollback |
| `audio-graph-84f4` | P2 | S2S/LLM/TTS | B: 61db, 8772, 76bd | M | Product-expansion | OpenRouter accelerator routing and API-surface compatibility |
| `audio-graph-8772` | P2 | S2S/LLM/TTS | R | M | Remote-evidence | OpenRouter routed smoke harness |
| `audio-graph-8e59` | P2 | Transcript/graph | R | M | Remote-evidence | Env-gated provider-backed projection smoke without secret/log leakage |
| `audio-graph-919e` | P2 | S2S/LLM/TTS | R | M | Product-expansion | MistralRs streaming chat adapter |
| `audio-graph-a6d4` | P2 | Credentials/settings | R | S | Alpha-blocking | Settings accessibility pass for provider configuration |
| `audio-graph-a805` | P2 | ASR/providers | R | M | Alpha-blocking | Split provider registry exporter into lightweight codegen path |
| `audio-graph-ad98` | P2 | Credentials/settings | R | M | Alpha-blocking | Redact provider HTTP and WebSocket error excerpts before UI/log surfacing |
| `audio-graph-b153` | P2 | Product/UX/trust | B: 8181, 53cf, 392b, ceda, 1971, 8235, 75a1, 9284, 5b2a, 8055, 058f, 70a3, 51e0, c282, a32f | XL | Product-expansion | Competitive product roadmap: overtake Granola and Cluely |
| `audio-graph-b360` | P2 | Diarization/speaker | B: eb6c | S | Alpha-blocking | Refresh diarization architecture docs after timeline design |
| `audio-graph-b373` | P2 | S2S/LLM/TTS | B: 919e, 2f4a | L | Product-expansion | Streaming chat for LocalLlama, MistralRs, AwsBedrock providers |
| `audio-graph-b5f3` | P2 | Audio/source | B: afca, bfcb | L | Product-expansion | Source-native multichannel processed-audio contract |
| `audio-graph-bfcb` | P2 | Audio/source | R | M | Alpha-blocking | Source-channel provenance descriptors and guard fixtures |
| `audio-graph-c237` | P2 | Diarization/speaker | R | M | Research-only/out-of-scope | Ground-truth overlapping speech fixture set for separation bakeoffs |
| `audio-graph-c282` | P2 | Product/UX/trust | R | M | Product-expansion | Retention policy matrix for session artifacts and diagnostics |
| `audio-graph-ceda` | P2 | Product/UX/trust | B: 48bb | M | Product-expansion | Architecture session: cross-session meeting memory workspace and recall UX |
| `audio-graph-dbac` | P2 | Diarization/speaker | B: eb6c, 20f2 | M | Alpha-blocking | Diarization settings UX for local, provider, and hybrid modes |
| `audio-graph-dd19` | P2 | Diarization/speaker | B: c237 | M | Research-only/out-of-scope | Source-separation bakeoff for experimental speaker PCM lanes |
| `audio-graph-e864` | P2 | Audio/source | B: afca | M | Alpha-blocking | Per-consumer audio backpressure telemetry in UI |
| `audio-graph-eb2e` | P2 | ASR/providers | B: d042 | M | Product-expansion | Implement Speechmatics live realtime STT runtime and readiness |
| `audio-graph-eebf` | P2 | Diarization/speaker | B: 3588, afca, eb6c, e864, bfcb | L | Alpha-blocking | Speaker timeline to channel-aware ASR projection |
| `audio-graph-f166` | P2 | CI/release | B: 0d66 | M | Remote-evidence | Capture source round-trip tests for Windows/macOS/Linux |
| `audio-graph-fd9f` | P2 | CI/release | R | M | Remote-evidence | Replace rsac sibling path dependency with published or pinned dependency |
| `audio-graph-058f` | P3 | Product/UX/trust | R | M | Research-only/out-of-scope | Architecture session: screen-context assist with explicit capture controls |
| `audio-graph-0c55` | P3 | Product/UX/trust | R | S | Product-expansion | Decide resumable actions for loaded historical live-assist pending cards |
| `audio-graph-14dc` | P3 | ASR/providers | R* | S | Research-only/out-of-scope | Google Chirp 3 enterprise gRPC adapter spike |
| `audio-graph-175e` | P3 | Audio/source | R* | M | Research-only/out-of-scope | Decide codec/decode boundary for imported audio and provider Opus support |
| `audio-graph-1971` | P3 | Product/UX/trust | R | L | Product-expansion | Privacy-first sharing, redaction, ACLs, and export links |
| `audio-graph-1a8c` | P3 | S2S/LLM/TTS | R* | L | Product-expansion | Local TTS providers: Kokoro, Piper, Coqui |
| `audio-graph-5b2a` | P3 | Product/UX/trust | R | M | Research-only/out-of-scope | Architecture session: team workspace and shared graph governance |
| `audio-graph-7e92` | P3 | ASR/providers | R* | S | Research-only/out-of-scope | ElevenLabs Scribe v2 Realtime provider watch/spike |
| `audio-graph-7fcc` | P3 | S2S/LLM/TTS | B: eee3, 0bdc | L | Product-expansion | Barge-in / interruption support across S2S providers |
| `audio-graph-8055` | P3 | Product/UX/trust | R | M | Research-only/out-of-scope | Architecture session: mobile and in-person capture companion |
| `audio-graph-8235` | P3 | Product/UX/trust | R | L | Product-expansion | Action item lifecycle and integration sync |
| `audio-graph-8784` | P3 | ASR/providers | R* | S | Research-only/out-of-scope | Azure Speech enterprise SDK adapter spike |
| `audio-graph-8eeb` | P3 | CI/release | R* | M | Remote-evidence | Replace pinned pipewire-debian PPA with stock Ubuntu 24.04 packages or pin to PPA SHA |
| `audio-graph-9284` | P3 | Product/UX/trust | R | L | Product-expansion | Domain mode packs and customizable meeting playbooks |
| `audio-graph-9d93` | P3 | Transcript/graph | B: 4da5 | M | Alpha-blocking | Frontend reducers for transcript, notes, and graph retcon events |
| `audio-graph-a32f` | P3 | Product/UX/trust | R | M | Product-expansion | SOC2 GDPR DPIA readiness checklist without certification claims |
| `audio-graph-d47b` | P3 | CI/release | R* | M | Remote-evidence | Debug build trips _CrtIsValidHeapPointer assertion on Windows (mixed CRT) |
| `audio-graph-fee1` | P3 | ASR/providers | R | S | Research-only/out-of-scope | Source-backed provider policy URL and processor matrix |
| `audio-graph-b521` | P4 | CI/release | R* | S | Research-only/out-of-scope | Migrate CI to Node.js 24 actions before September 2026 |
| `audio-graph-d760` | P4 | Queue/hygiene | R* | S | Research-only/out-of-scope | Normalize duplicate-title handling for closed duplicate Seeds |

## Audit Findings

1. Direct JSONL is the only complete open backlog view in this checkout. `sd
   ready` plus `sd blocked` accounts for 91 rows, while direct parsing finds
   100 open rows. The nine `R*` rows are unblocked in JSONL and should not be
   lost during planning.
2. The actual alpha cutline is smaller than the P1/P2 count suggests. Several
   P1 items are voice-agent or remote-evidence lanes; they matter, but they
   should not preempt the durable transcript, notes, graph, credential,
   provider-harness, and source/audio foundations.
3. CI/release and provider live-smoke work should be isolated. This checkout is
   too dirty for workflow edits, broad `sd sync`, or live credential handling in
   shell-visible commands.
4. `audio-graph-ad1d` is still carrying both alpha provider-platform work and
   product-expansion providers. Without a cutline extension, closure pressure on
   Speechmatics/Gladia can blur the alpha readiness definition.
5. `audio-graph-eee3` is P1 but categorized here as product-expansion because it
   is the local/hybrid realtime voice-agent path, not the durable speech-to-
   notes/temporal-graph alpha path. Its dependency on `audio-graph-1a8c` should
   be revisited if closed Deepgram Aura TTS is already enough for a first
   non-fully-local voice-agent slice.

## Seed Proposals Not Applied

No Seeds were created, updated, closed, or synced during this audit. Proposed
queue updates for a future Seed hygiene turn:

- Add an extension to `audio-graph-ad1d` defining the alpha provider-platform
  cutline separately from post-alpha Speechmatics/Gladia expansion.
- Add an extension to `audio-graph-eee3` or split a child Seed so fully local TTS
  providers (`audio-graph-1a8c`) do not block a cloud-TTS-backed first local/
  hybrid orchestrator slice if that is now the intended product path.
- Use `audio-graph-d760` or a new narrow queue-hygiene Seed to document why the
  nine direct-only unblocked rows are absent from `sd ready`, and whether the
  repo-pinned Seeds CLI should expose an uncapped ready listing.
- Add a clean-worktree execution plan extension to `audio-graph-c395` for
  workflow/release edits and Blacksmith evidence gathering, rather than doing
  those edits from this dirty checkout.
