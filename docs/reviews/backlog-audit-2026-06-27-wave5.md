# Backlog Audit - 2026-06-27 Wave 5

## Scope

This is the current backlog-zero Wave 5 queue audit. It supersedes the counts
in `docs/reviews/backlog-audit-2026-06-27-dynamic-roadmap.md`, which was
captured before later Seeds were added.

No provider keys, temporary secrets, raw provider responses, or secret-bearing
logs are copied here.

## Snapshot

- Branch: `master`
- HEAD: `831cc30101840db87bd2b502f2da749d65fe1c22`
- Total Seeds: 319
- Open Seeds: 104
- Closed Seeds: 215
- In progress Seeds: 0
- Ready Seeds: 62
- Blocked Seeds: 42
- Open priority split: P1 34, P2 49, P3 19, P4 2
- Open type split: 9 epics, 41 features, 53 tasks, 1 bug

## Current Open Inventory

`R` means open/unblocked. `B:<ids>` means blocked by the listed Seed suffixes.

| Seed | P | Lane | Deps | Type | Title |
| --- | ---: | --- | --- | --- | --- |
| `audio-graph-eee3` | P1 | S2S/LLM/TTS | B:1a8c,14e0,f53b | epic | Local/hybrid S2S pipeline: STT -> vLLM -> TTS turn orchestrator |
| `audio-graph-74b2` | P1 | CI/release | R | task | Blacksmith Tauri build smoke matrix |
| `audio-graph-14e0` | P1 | ASR/providers | B:0117,9279 | feature | Moonshine streaming STT provider (local) |
| `audio-graph-ad1d` | P1 | ASR/providers | B:d042,a805,e35f,f0a3,226e,eb2e,02da | epic | Provider registry and streaming STT expansion roadmap |
| `audio-graph-c395` | P1 | CI/release | B:0162,2586,74b2,8eeb,0d66,fbf6,fd9f | epic | Cross-platform CI and Blacksmith release-readiness matrix |
| `audio-graph-4673` | P1 | Transcript/graph | B:ad44,8e59,c395,4da5,9c89,9d93,0d1c | epic | Streaming transcript to notes and temporal-graph diff pipeline |
| `audio-graph-1c2f` | P1 | Credentials/settings | B:0162,cbde,0c08,a3d8,a6d4,ad98,d262 | epic | Configuration UX and credential health center |
| `audio-graph-e35f` | P1 | ASR/providers | B:0b93,be03 | feature | Implement Soniox realtime STT provider |
| `audio-graph-2044` | P1 | Audio/source | B:1e47 | epic | Source descriptor and audio consumer bus refactor |
| `audio-graph-cbde` | P1 | Credentials/settings | B:d262 | feature | Saved-credential health checks and model discovery on Settings open |
| `audio-graph-ad44` | P1 | Transcript/graph | R | feature | Event-sourced transcript/notes/graph synthesis data model |
| `audio-graph-afca` | P1 | ASR/providers | B:1d59 | feature | Dynamic processed-audio consumer registry |
| `audio-graph-2586` | P1 | CI/release | R | task | Move release workflow to Blacksmith and pinned actions |
| `audio-graph-a2ff` | P1 | ASR/providers | B:afca | feature | Provider audio policy registry: per-source vs mixed vs sessions |
| `audio-graph-f0a3` | P1 | ASR/providers | R | feature | Upgrade AssemblyAI streaming to Universal-3.5 Pro Realtime/v3 |
| `audio-graph-4da5` | P1 | Transcript/graph | B:ad44 | feature | Transcript revision ledger and canonical span projection |
| `audio-graph-9c89` | P1 | Transcript/graph | B:4da5,ad44 | feature | Session artifact migration for transcript and projection events |
| `audio-graph-3588` | P1 | ASR/providers | B:dbac | epic | Local streaming diarization and speaker timeline architecture |
| `audio-graph-5011` | P1 | Diarization | B:afca,eb6c,b05b | feature | Local streaming diarization worker with flexible speaker counts |
| `audio-graph-1fbd` | P1 | ASR/providers | B:ad44,eb6c,20f2 | feature | Normalize provider diarization into speaker-span revisions |
| `audio-graph-eb6c` | P1 | Diarization | B:ad44 | feature | Speaker timeline event schema and replay fixtures |
| `audio-graph-0117` | P1 | ASR/providers | R | feature | Moonshine streaming worker and span-revision adapter |
| `audio-graph-9279` | P1 | CI/release | B:0117,0d58 | feature | Moonshine model downloader readiness and cross-platform validation |
| `audio-graph-0d58` | P1 | CI/release | R | task | Blacksmith asr-moonshine feature compile matrix |
| `audio-graph-b05b` | P1 | CI/release | R | task | Diarization clustering feature compile and cross-platform smoke matrix |
| `audio-graph-f53b` | P1 | Audio/source | R | feature | Wire rubato output resampling into CPAL playback |
| `audio-graph-d042` | P1 | ASR/providers | R | feature | Reusable ASR provider transport and parser fixture harness |
| `audio-graph-fbf6` | P1 | CI/release | R | task | Cross-platform optional Rust feature compile matrix |
| `audio-graph-0c08` | P1 | Credentials/settings | R | task | OS keychain credential backend and non-destructive import |
| `audio-graph-a3d8` | P1 | Credentials/settings | B:0c08 | task | Settings credential source labels for keychain and fallback stores |
| `audio-graph-0b93` | P1 | ASR/providers | B:319c | task | Run Soniox env-gated live smoke and record redacted evidence |
| `audio-graph-be03` | P1 | Credentials/settings | B:0b93 | task | Promote Soniox to selectable ASR provider |
| `audio-graph-319c` | P1 | Credentials/settings | R | task | Provision Soniox live-smoke credential through safe local path |
| `audio-graph-d262` | P1 | Credentials/settings | R | task | First-class generic OpenAI-compatible LLM saved-key readiness/catalog |
| `audio-graph-396f` | P2 | S2S/LLM/TTS | R | epic | Implement OpenAI Realtime gpt-realtime-2 cloud-native S2S provider |
| `audio-graph-82b3` | P2 | S2S/LLM/TTS | B:eee3 | feature | Deepgram Flux EagerEndOfTurn support with TurnResumed rollback |
| `audio-graph-fd9f` | P2 | CI/release | R | task | Replace rsac sibling path dependency with published or pinned dependency |
| `audio-graph-b373` | P2 | S2S/LLM/TTS | B:919e,2f4a | task | Streaming chat for LocalLlama, MistralRs, AwsBedrock providers |
| `audio-graph-0d66` | P2 | CI/release | R | feature | Live rsac audio smoke tests on CI-capable runners |
| `audio-graph-09a7` | P2 | CI/release | B:0d66 | task | Cross-platform release usability smoke runbooks |
| `audio-graph-f166` | P2 | Audio/source | B:0d66 | task | Capture source round-trip tests for Windows/macOS/Linux |
| `audio-graph-e864` | P2 | Audio/source | B:afca | feature | Per-consumer audio backpressure telemetry in UI |
| `audio-graph-02da` | P2 | ASR/providers | B:e35f,f0a3 | feature | Reusable streaming WebSocket ASR session harness |
| `audio-graph-559d` | P2 | Credentials/settings | R | feature | Migrate user settings to config.yaml with settings.json import |
| `audio-graph-1e47` | P2 | Audio/source | R | feature | Capability and permission gated source selection states |
| `audio-graph-0d1c` | P2 | Transcript/graph | B:ad44 | task | Supersede ADR-0014 with event-sourced notes and graph projection architecture |
| `audio-graph-dbac` | P2 | Credentials/settings | B:eb6c,20f2 | feature | Diarization settings UX for local, provider, and hybrid modes |
| `audio-graph-eebf` | P2 | ASR/providers | B:3588,afca,eb6c,e864,bfcb | feature | Speaker timeline to channel-aware ASR projection |
| `audio-graph-b360` | P2 | Diarization | B:eb6c | task | Refresh diarization architecture docs after timeline design |
| `audio-graph-a6d4` | P2 | Credentials/settings | R | task | Settings accessibility pass for provider configuration |
| `audio-graph-a805` | P2 | ASR/providers | R | task | Split provider registry exporter into lightweight codegen path |
| `audio-graph-eb2e` | P2 | Credentials/settings | B:d042 | feature | Implement Speechmatics live realtime STT runtime and readiness |
| `audio-graph-226e` | P2 | Credentials/settings | B:d042 | feature | Implement Gladia Solaria live runtime and registry readiness |
| `audio-graph-8e59` | P2 | CI/release | R | task | Env-gated provider-backed projection smoke without secret/log leakage |
| `audio-graph-0162` | P2 | CI/release | R | task | Cross-platform provider setup UX validation |
| `audio-graph-919e` | P2 | S2S/LLM/TTS | R | feature | MistralRs streaming chat adapter |
| `audio-graph-2f4a` | P2 | S2S/LLM/TTS | R | feature | AwsBedrock ConverseStream adapter |
| `audio-graph-0bdc` | P2 | Audio/source | R | task | VAD and AEC crate bakeoff for local turn detection and barge-in |
| `audio-graph-b153` | P2 | Product/UX/trust | B:8181,53cf,392b,ceda,1971,8235,75a1,9284,5b2a,8055,058f,70a3,51e0,c282,a32f | epic | Competitive product roadmap: overtake Granola and Cluely |
| `audio-graph-8181` | P2 | Product/UX/trust | R | task | Competitive benchmark suite for notes, memory, and live assist quality |
| `audio-graph-53cf` | P2 | Transcript/graph | R | feature | Calendar and prior-context pre-briefs from the temporal graph |
| `audio-graph-392b` | P2 | Product/UX/trust | R | feature | Live assistance agent triggers, question detection, and key-note capture |
| `audio-graph-ceda` | P2 | Transcript/graph | B:48bb | task | Architecture session: cross-session meeting memory workspace and recall UX |
| `audio-graph-75a1` | P2 | Product/UX/trust | R | feature | Time-to-first-note onboarding and sample session UX |
| `audio-graph-ad98` | P2 | Credentials/settings | R | task | Redact provider HTTP and WebSocket error excerpts before UI/log surfacing |
| `audio-graph-48bb` | P2 | Storage/memory | B:2b2c | task | SurrealDB embedded local memory adapter spike |
| `audio-graph-dd19` | P2 | Audio/source | B:c237 | task | Source-separation bakeoff for experimental speaker PCM lanes |
| `audio-graph-bfcb` | P2 | Audio/source | R | feature | Source-channel provenance descriptors and guard fixtures |
| `audio-graph-b5f3` | P2 | ASR/providers | B:afca,bfcb | feature | Source-native multichannel processed-audio contract |
| `audio-graph-20f2` | P2 | ASR/providers | B:eb6c | task | Provider speaker and channel diarization parser fixtures |
| `audio-graph-c237` | P2 | Audio/source | R | task | Ground-truth overlapping speech fixture set for separation bakeoffs |
| `audio-graph-2b2c` | P2 | CI/release | R | task | Evaluate SurrealDB file-backed engines on Blacksmith before storage selectability |
| `audio-graph-70a3` | P2 | ASR/providers | R | task | Session data movement ledger and audit event schema |
| `audio-graph-51e0` | P2 | Credentials/settings | R | feature | Session data route UI and privacy report |
| `audio-graph-c282` | P2 | Product/UX/trust | R | task | Retention policy matrix for session artifacts and diagnostics |
| `audio-graph-3b9f` | P2 | ASR/providers | R | task | Provider-internal privacy guards for socket and request egress |
| `audio-graph-84f4` | P2 | Credentials/settings | B:61db,8772,76bd | feature | OpenRouter accelerator routing and API-surface compatibility |
| `audio-graph-61db` | P2 | Credentials/settings | R | task | OpenRouter accelerator catalog view model |
| `audio-graph-8772` | P2 | ASR/providers | R | task | OpenRouter routed smoke harness |
| `audio-graph-76bd` | P2 | ASR/providers | R | task | OpenRouter routed provider telemetry |
| `audio-graph-098b` | P2 | Audio/source | R | task | Playback-reference echo fixture harness for VAD/AEC |
| `audio-graph-bc1c` | P2 | Other | R | task | Map dirty worktree ownership before broad merges |
| `audio-graph-1d59` | P2 | Audio/source | R | task | Command-layer capture start/stop registry lifecycle tests |
| `audio-graph-1a8c` | P3 | S2S/LLM/TTS | R | feature | Local TTS providers: Kokoro, Piper, Coqui |
| `audio-graph-7fcc` | P3 | S2S/LLM/TTS | B:eee3,0bdc | feature | Barge-in / interruption support across S2S providers |
| `audio-graph-8eeb` | P3 | CI/release | R | task | Replace pinned pipewire-debian PPA with stock Ubuntu 24.04 packages or pin to PPA SHA |
| `audio-graph-d47b` | P3 | Other | R | bug | Debug build trips _CrtIsValidHeapPointer assertion on Windows (mixed CRT) |
| `audio-graph-9d93` | P3 | Transcript/graph | B:4da5 | feature | Frontend reducers for transcript, notes, and graph retcon events |
| `audio-graph-7e92` | P3 | ASR/providers | R | task | ElevenLabs Scribe v2 Realtime provider watch/spike |
| `audio-graph-14dc` | P3 | ASR/providers | R | task | Google Chirp 3 enterprise gRPC adapter spike |
| `audio-graph-8784` | P3 | ASR/providers | R | task | Azure Speech enterprise SDK adapter spike |
| `audio-graph-175e` | P3 | Audio/source | R | task | Decide codec/decode boundary for imported audio and provider Opus support |
| `audio-graph-1971` | P3 | Product/UX/trust | R | feature | Privacy-first sharing, redaction, ACLs, and export links |
| `audio-graph-8235` | P3 | Product/UX/trust | R | feature | Action item lifecycle and integration sync |
| `audio-graph-9284` | P3 | Transcript/graph | R | feature | Domain mode packs and customizable meeting playbooks |
| `audio-graph-5b2a` | P3 | Transcript/graph | R | task | Architecture session: team workspace and shared graph governance |
| `audio-graph-8055` | P3 | Product/UX/trust | R | task | Architecture session: mobile and in-person capture companion |
| `audio-graph-058f` | P3 | Product/UX/trust | R | task | Architecture session: screen-context assist with explicit capture controls |
| `audio-graph-0c55` | P3 | Product/UX/trust | R | task | Decide resumable actions for loaded historical live-assist pending cards |
| `audio-graph-a32f` | P3 | Product/UX/trust | R | task | SOC2 GDPR DPIA readiness checklist without certification claims |
| `audio-graph-fee1` | P3 | ASR/providers | R | task | Source-backed provider policy URL and processor matrix |
| `audio-graph-403d` | P3 | Credentials/settings | R | task | Use SID-native Windows ACL hardening for owner-only files |
| `audio-graph-b521` | P4 | CI/release | R | task | Migrate CI to Node.js 24 actions before September 2026 |
| `audio-graph-d760` | P4 | Other | R | task | Normalize duplicate-title handling for closed duplicate Seeds |

## Wave 5 Routing Notes

- CI/workflow, package-lock, Cargo manifest, and generated-registry items remain
  clean-worktree-only in this dirty checkout.
- `audio-graph-1d59` is the only current write-scope implementation candidate
  in this wave; it should stay limited to command-layer tests and any tiny
  `#[cfg(test)]` seam required to avoid live hardware.
- `audio-graph-d262` should start as a backend-contract clean-worktree slice;
  do not mix it with OpenRouter accelerator catalog UI (`61db` / `84f4`).
- Remote-evidence Seeds such as `0c08`, `f53b`, `0d58`, `b05b`, `74b2`, and
  `fbf6` need clean CI or OS/provider evidence, not more local narrow proof.
