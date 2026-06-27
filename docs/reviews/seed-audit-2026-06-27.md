# AudioGraph Seed Backlog Audit — 2026-06-27

Source: 8 cluster digests under `docs/reviews/_audit-2026-06-27/digest-*.json`
(data-architecture, asr-streaming, providers-llm, security-privacy, audio, ci-testing, ux-settings, misc).

---

## 1. Executive Summary

### Totals

| Cluster | Total | Closed | Open | Blocked |
|---|---|---|---|---|
| data-architecture | 72 | 46 | 26 | 13 |
| asr-streaming | 59 | 35 | 24 | 10 |
| providers-llm | 58 | 40 | 18 | 4 |
| security-privacy | 40 | 33 | 7 | 2 |
| misc | 33 | 26 | 7 | 2 |
| ci-testing | 24 | 15 | 9 | 2 |
| ux-settings | 21 | 14 | 7 | 2 |
| audio | 12 | 4 | 8 | 1 |
| **TOTAL** | **319** | **213** | **106** | **37** |

(`buckets.json` reports `total_assigned: 319`. "Blocked" is a subset of "Open": of 106 open seeds, 37 are blocked by an unresolved dependency, leaving ~69 actionable now. ~67% of the backlog has shipped.)

### The 3–5 biggest in-flight arcs

1. **Event-sourced transcript/notes/graph data model (data-architecture).** `audio-graph-ad44` is the single most load-bearing unresolved seed in the repo. It defines the canonical `TranscriptEvent` JSONL schema, `ProjectionJob` basis, and artifact paths. It directly gates 5 P1/P2 seeds (`4da5`, `eb6c`, `9c89`, `0d1c`, `4673`) and transitively gates the entire diarization sub-tree and the frontend reducer work. The projection/materializer engine *around* it has largely shipped (retcon ops, replay harness, telemetry, schemas) — the canonical event contract itself is the hole.

2. **Diarization / speaker-timeline normalization (asr-streaming + data-architecture).** The largest cross-cluster open arc: `eb6c` (SpeakerTimeline schema) → `20f2`, `1fbd`, `dbac`, `b360`, `eebf`, `5011`, plus the architecture seed `3588`. All blocked behind `eb6c`, which is itself blocked behind `ad44`. Soniox diarization fixtures (`5e61`) shipped, but the schema that everything else hangs off has not landed.

3. **Cross-platform CI / release evidence (ci-testing + audio + providers-llm).** A pervasive pattern, not a single arc: many seeds are *locally complete and tested* but cannot close without Blacksmith Linux/macOS/Windows run-URL evidence from a clean pushed ref. The dirty 198-file checkout is the structural blocker. Keystones here: `0d66` (live rsac audio smoke), `fbf6` (optional-feature matrix), `74b2`/`2586` (Tauri smoke / release publish), feeding the epic `c395`.

4. **Provider expansion + reusable transport harness (asr-streaming + data-architecture).** Parser-first spikes shipped for Soniox, Speechmatics, Gladia, Rev AI, AssemblyAI v3; live runtimes are gated on the shared harness (`d042`/`02da`) and live-smoke credentials. The roadmap epic `ad1d` waits on both harnesses.

5. **Local/hybrid S2S voice pipeline (providers-llm + misc + audio).** `eee3` (STT→vLLM→TTS turn orchestrator) is blocked on local TTS (`1a8c`), playback resampling (`f53b`), and the audio subsystem; it in turn gates the S2S wave-3 features (`82b3`, `7fcc`). TTS/playback fundamentals (`3132`/`8d75`/`92c7`) already shipped.

### What "backlog to zero" realistically requires

- **Land one root schema.** `ad44` must land before ~10 downstream seeds can even start. This is the highest-leverage single move in the backlog.
- **Set up a clean-ref CI evidence pipeline.** A large fraction of "open" seeds are code-complete and blocked only on multi-OS Blacksmith run URLs. Establishing a clean evidence branch (the `bc1c` ownership-map work is the prerequisite) converts a dozen seeds from "open" to "closed" with no new code.
- **Provision live-smoke credentials safely** (`319c`) to unblock the Soniox promotion chain and the AssemblyAI v3 smoke (`f0a3`).
- **Make a SurrealDB-vs-file storage decision** (`2b2c`). It is gating the CI default-feature matrix decisions and the cross-session memory UX, even though FileMemoryRepository is shipped and default.
- The competitive product roadmap (`b153`, 15 blockers) is intentionally deferred behind the P1 platform cutline and should *not* be pulled forward.

---

## 2. DONE — What Has Shipped (213 seeds)

Clustered by domain; load-bearing items called out with the "how".

### Projection / event-sourcing engine (data-architecture, ~25 seeds)
The materialization side of the event-sourced architecture is substantially built even though the canonical event-source schema (`ad44`) is still open:
- **Temporal graph patch + retcon engine** (`6008`, `44bb`, `b57a`): `GraphOp` with stable ids, confidence, `valid_from`/`valid_until`, provenance; replay applies add/update/remove/merge/split/invalidate/strengthen/weaken deterministically without duplicating entities.
- **Notes as versioned diffs** (`d5a4`, `6347`, `e9b6`): `NoteOp` insert/update/delete/reorder with span provenance; structured JSON schemas + validators + repair prompts, provider-agnostic.
- **TTFT-aware projection scheduler** (`d524`, `3f24`, `f66a`, `c01c`): coalesces partial deltas, one in-flight job, stale→repair conversion, adaptive coalescing, full latency/backpressure telemetry.
- **Deterministic offline replay harness** (`24dc`, `d83c`, `3886`, `b1ac`, `ff70`, `7548`, `0e3a`, `6f39`): no-network replay from checked-in fixtures with retcon/duplicate/stale-basis coverage; replay validates against transcript history, not just artifacts.
- **Crash/replay session restore** (`60ca`, `93fc`): rebuilds notes+graph from JSONL when artifacts are stale; fails closed before ledger advance on poisoned writer.

### Storage / repository abstraction (data-architecture, ~8 seeds)
- **LocalMemoryRepository trait + FileMemoryRepository** (`5679`, `f2b6`, `ff32`): backend-owned repository seam; ASR ingestion + projection writers append through it; typed artifact descriptors (not `Vec<PathBuf>`) so DB-backed repos have a real export/delete story.
- **Repository replay parity conformance suite** (`965b`): the bar any SurrealDB adapter must clear before becoming default.
- **Bounded event-writer queues + backpressure** (`3a09`, `24dc`): replaced unbounded mpsc with bounded sync channels, try_send, per-queue metrics, UI diagnostics — applies to file and DB paths.
- **SurrealDB 3.x embedding spike** (`5dde`): evaluated kv-mem/kv-surrealkv/kv-rocksdb; established the repository trait as the gating abstraction.

### Privacy / credential security (security-privacy + data-architecture, ~30 seeds)
A rigorous inside-out remediation arc — the most thoroughly-executed cluster:
- **No-plaintext-credential-in-React invariant** (`c906`, `9d0e`, `dba3`, `b266`, `c309`, `e634`): `load_credential_cmd` deleted from IPC; presence/readiness-only API; test mocks *throw* on reintroduction; regression test proves IPC absence.
- **CredentialBackend facade + keychain-first** (`799a`, `1322`/ADR-0019, `0c08` code-complete, `e78e`, `403d`): OS keychain primary, YAML import/fallback, owner-only temp-file writes with Windows ACLs.
- **Default-block content-egress policy** (`7db3`, `d598`, `e604`, `bf74`, `8c0d`, `131a`, `a4b6`, `25aa`): Rust-owned `PrivacyMode`; provider-client construction requires explicit policy; session content stripped from all logs/parse-errors; fail-closed settings reads; endpoint-aware loopback exemption.
- **Redaction-safe Debug** (`9338`, `1bd7`): manual Debug on transcript/projection/settings/provider structs; secrets render `<present>`/`<missing>`.
- **Org-knowledge promotion redaction built before any cloud sync** (`053f`, `6165`, `0586`, `e793`, `d115`, `ac71`): promotion/redaction schemas, fixtures, and no-cloud-sync IPC guards exist before transport.
- **Secret hygiene scanner** (`de28`) treats docs/Seeds as attack surface; `secrecy` wrapper explicitly deferred (`6cce`).

### Provider registry + Settings UX (providers-llm + ux-settings, ~35 seeds)
- **Backend-owned provider registry as single source of truth** (`257a`, `80ed`, `f8e0`, `baa6`, `b6a6`, `b701`): typed `ProviderDescriptor` for ASR/LLM/TTS/Gemini/RealtimeAgent; roadmap status (watch/enterprise_watch/rejected), auth schema, data-boundary, transport/packaging metadata; generated TS schema.
- **Settings redesign around product modes + capability cards** (`c323`, `925b`, `f729`, `903d`, `e362`, `cf72`, `df7e`, `9882`, `b638`, `c502`): readiness-derived cards, searchable model catalog picker, progressive advanced disclosure, source-capability blockers.
- **OpenRouter as default cloud LLM** (`c847`, `a641`, `b652`, `d157`, `bd8c`, `8650`, `bb32`): provider variant, routing-policy presets/serializer parity, provider/endpoint catalog commands, draft-URL-aware test/discovery.
- **Cerebras first-class provider** (`94fc`, `590f`); **local-ML feature gating** (ADR-0007: `5fe7`, `5b75`) confirmed cloud build excludes whisper-rs/llama-cpp-2/mistralrs.
- **Typed SourceDescriptor from rsac→React** (`3251`, `0dba`, `8b94`, `7ee6`, `f3ff`, `cc78`): generated `ipc-contract` crate; eliminated WASAPI id-prefix heuristics; backend-driven device direction.

### ASR streaming + reconnect correctness (asr-streaming, ~35 seeds)
- **Normalized ASR span-revision schema** (`3709`, `bf51`, `4168`): span_id/provider_item_id/speaker_id/stability/revision_number/supersedes/turn_id; final-only providers get deterministic stable span metadata; projection lifecycle scoped by session.
- **Reconnect lifecycle dedup** (`ee7d`, `5633`, `10ec`, `379f`): one-shot Disconnected guards, stale-socket run_io re-entry fixes for Deepgram + OpenAI Realtime + AssemblyAI.
- **Production WebSocket transport boundary** (`b841`): `asr/transport.rs` with policy enforced at the write primitive; Soniox first consumer.
- **AssemblyAI v3 migration** (`c9ec`): binary pcm_s16le, speaker_labels, SpeakerRevision sidebands (live smoke `f0a3` still open).
- **Parser-first provider spikes** (`0557` Rev AI, `1476` Speechmatics, `228b` Gladia, `a2dc` Moonshine skeleton): fixtures + saved-key readiness before any live socket or Settings exposure.
- **LocalLlama streaming** (`e2b6`, `5958`): per-token deltas via persistent-context actor with interruptible cancellation.
- **SSE decoder buffer cap** (`3344`), finish_reason propagation (`0e34`), timing persistence for replay latency split (`c731`).

### TTS / playback (misc, ~7 seeds — fully closed arc)
- `3132` TtsProvider trait + Deepgram Aura streaming; `8d75` CPAL playback + barge-in ~50ms; `92c7` speak-aloud loop (LLM token deltas → TTS); Aura review fixes (`7107`, `d875`, `0e19`).

### CI / tooling (ci-testing + misc, ~25 seeds)
- **Blacksmith cloud-only matrix** (`150f`, `5b75`, `f98e`, `2b06`): Linux/macOS/Windows cargo check/test --features cloud + Tauri no-bundle smoke verified (PR 21 run 28126263189).
- **Windows test-harness fixes** (`e5f8`, `9f6e`, `2b06`): CRT-skew mitigations documented + scoped.
- **Seeds CLI JSON durability** (8+ seeds: `2a14`, `3926`, `103f`, `2743`, `8c46`, `a844`, `c0cb`, `2e71`, `f18c`, `1e4b`): the os-eco/seeds-cli 64KB stdout-truncation root cause patched and pinned; dependency integrity repaired; queue visibility uncapped.
- **Public settings schema** (`667e`): schemars derive with credential fields skipped.

---

## 3. OPEN & READY — The Actionable Queue

~69 actionable seeds. Listed by priority with acceptance criteria. **Bold = highest leverage** (unblocks downstream work or is a keystone).

### P1 — Ready now

| ID | Title | Acceptance (condensed) | Effort |
|---|---|---|---|
| **`ad44`** | Event-sourced transcript/notes/graph data model | TranscriptEvent JSONL persisted with full span fields; ProjectionJob basis + ProjectionPatch schema; canonical artifact paths; replay produces identical state. **Root blocker for 10+ seeds.** | L |
| **`afca`** | Dynamic processed-audio consumer registry | Speech/Gemini/converse/OpenAI-Realtime/local-S2S coexist or are policy-rejected without new AppState fields; per-consumer drop counters + queue health to UI. **Blocks 5 seeds.** | M |
| **`d042`** | Reusable ASR transport + parser-fixture harness | connect/init/audio/write/read/reconnect/keepalive/terminal over tungstenite; ≥2 providers share it without losing provider-specific parsing. **Blocks `ad1d`,`226e`,`eb2e`.** | L |
| `3588` | Local streaming diarization + speaker-timeline architecture | DiarizationSpan/SpeakerTimeline events, stable speaker IDs, rolling revisions, source/channel metadata, auto/max policy, local-only/provider-join modes. | L |
| `2044` | Source descriptor + audio consumer bus refactor | Backend emits typed SourceDescriptor; frontend stops inferring direction from IDs; consumers register with bounded queues + telemetry. | L |
| `d262` | Generic OpenAI-compatible LLM saved-key readiness/catalog | llm.api advertises remote health + catalog; auto-probe active endpoint on blank draft key; readiness cache fingerprint includes endpoint/model/cred-epoch; redacted errors. **Unblocks `cbde`,`1c2f`.** | M |
| `fbf6` | Cross-platform optional-feature compile matrix | Blacksmith Linux/macOS/Windows evidence for cloud + {asr-moonshine, diarization-clustering, sherpa-streaming, llm-llama, llm-mistralrs}. | M |
| **`f53b`** | Wire rubato output resampling into CPAL playback | 24k/16k PCM plays on 48k devices without blocking callback; frame/cancellation tests; **code-complete, needs clean-ref CI evidence.** Unblocks `eee3`. | S |
| **`0d58`** | Blacksmith asr-moonshine feature compile matrix | 3-OS passing run URLs for cloud,asr-moonshine; moonshine + provider_readiness filters pass remotely. | S |
| `0117` | Moonshine streaming worker + span-revision adapter | Native Moonshine C API bindings + production speech runtime branch + 3-OS CI evidence (scaffold exists). | M |
| `74b2` | Blacksmith Tauri build smoke matrix | Clean branch pushes ci.yml; default no-bundle smoke passes 3-OS; artifacts inspected. | S |
| `2586` | Move release workflow to Blacksmith + pinned actions | Clean-ref dry-run URL; then approved disposable tag produces/inspects DMG/NSIS/AppImage/deb + rsac manifests. | M |
| `f0a3` | Upgrade AssemblyAI streaming to Universal-3.5 Pro/v3 | Run env-gated v3 WS live smoke with real audio; update readiness copy (backend already implemented). | S |
| `eee3` | Local/hybrid S2S: STT→vLLM→TTS turn orchestrator | (Also blocked — see §4; ready only after `1a8c`/`f53b`.) | L |

### P2 — Ready now (selected high-value)

| ID | Title | Acceptance (condensed) | Effort |
|---|---|---|---|
| **`bc1c`** | Map dirty-worktree ownership before broad merges | Ownership map tying each dirty file group to an owning Seed; identify clean-worktree-required files; define merge order. **Prerequisite for the whole CI-evidence unlock.** | M |
| **`2b2c`** | Evaluate SurrealDB file engines on Blacksmith | kv-surrealkv vs kv-rocksdb on build/link/binary-size/native-deps/corruption/Tauri packaging 3-OS; recommendation documented. **Sole gate before `48bb`→`ceda`.** | M |
| `02da` | Reusable streaming WebSocket ASR session harness | ≥1 existing + 1 new provider share it across Deepgram/AssemblyAI/OpenAI/Soniox/Speechmatics/Gladia. | M |
| `c282` | Retention policy matrix for session artifacts | Each data class has storage boundary, retention default, delete/export behavior, audit requirement. | M |
| `70a3` | Session data-movement ledger + audit event schema | Redacted session-scoped ledger for all data movements; no raw audio/transcript/keys. | M |
| `c237` | Ground-truth overlapping-speech fixture set | Record mono-ASR + diarization baselines for overlap + turn-taking fixtures. Unblocks `dd19`. | S |
| `76bd`/`8772`/`61db` | OpenRouter telemetry / smoke / accelerator catalog | Sanitized routing telemetry; env-gated routed smoke; non-secret accelerator view model. **All three unblock `84f4`.** | M each |
| `ad98` | Redact provider HTTP/WS error excerpts | All error paths through centralized redaction helper; covers streaming.rs, tts, ASR readiness. | M |
| `51e0` | Session data route UI + privacy report | Loaded sessions show local-vs-left-device routing without secrets. | M |
| `a6d4` | Settings accessibility pass | Listbox labels localized; disabled-action reasons accessible; live-region tests green. | M |
| `559d` | Migrate user settings to config.yaml + import | Fresh installs write config.yaml; existing import settings.json once; corrupt-YAML fallback. | S |
| `098b` / `0bdc` | Playback-reference echo fixtures / VAD+AEC bakeoff | AEC fixture harness then dated bakeoff across earshot/silero/webrtc/sonora. | M / L |
| `bfcb` | Source-channel provenance descriptors + guard fixtures | Misleading stereo→mono+timeline fallback; true multichannel preserved; derived lanes rejected for source-native. | M |
| `0d66` | Live rsac audio smoke on CI runners | workflow_dispatch live smokes on PipeWire/VB-CABLE/BlackHole. **Blocks `f166`,`09a7`,`c395`.** | L |
| `1d59` | Capture start/stop registry lifecycle tests | No stale worker/consumer slots across stop/start; deterministic health/drop without hardware. | S |
| `0162` | Cross-platform provider setup UX validation | Blacksmith 3-OS remote evidence from clean branch (code complete). | M |
| `a805` | Split provider registry exporter into lightweight codegen | generate/check:provider-registry validated on macOS+Windows runners. | S |
| `396f` | OpenAI Realtime gpt-realtime-2 cloud-native S2S | New openai_realtime/ module mirroring gemini/mod.rs; session resume/reconnect/usage; commands + tests. | L |
| `8e59` | Env-gated provider-backed projection smoke | Skipped without env; sanitized telemetry-only output; runs on clean CI. | M |
| `8181` | Competitive benchmark suite for notes/memory/assist | Benchmark plan with datasets/metrics/scoring/cadence; can gate roadmap + expose regressions. | L |

### P3 / P4 — Ready (lower urgency)
`7e92` (ElevenLabs watch spike, S), `8784`/`14dc` (Azure/Chirp enterprise adapter spikes, L), `1a8c` (local TTS Kokoro/Piper/Coqui, L — unblocks `eee3`), `d47b` (Windows debug CRT, M), `fee1` (provider policy/processor matrix, L), `175e` (codec/decode boundary decision, S), `8055`/`058f` (mobile/screen-context arch sessions, M), `5b2a` (team-workspace arch, M), `0c55` (resumable historical cards, S), `9284` (domain mode packs, L), `8235` (action-item lifecycle, L), `a32f` (SOC2/GDPR DPIA checklist, M), `1971` (privacy-first sharing, L), `403d` (SID-native Windows ACLs, M), `8eeb` (PipeWire PPA removal, S), `fd9f` (rsac published/pinned dep, M), `b521` (Node 24 actions, S — hard 2026-09-16 deadline), `d760` (duplicate-title handling, S), `53cf`/`67f9` (calendar pre-briefs, L), `75a1` (onboarding UX, L), `392b` (live assist agent, L), `1e47` (capability-gated source states, M).

---

## 4. OPEN & BLOCKED — Blocker Chains and Keystones

37 blocked seeds. The dependency graph collapses to a small number of **keystone blockers**.

### Keystone blockers (ranked by total downstream unblock count)

**1. `audio-graph-ad44` — Event-sourced transcript/notes/graph data model (OPEN, ready, unblocked).**
Unblocks (direct + transitive): `4da5`, `eb6c`, `9c89`, `0d1c`, `9d93`, `4673` and — via `eb6c` — `20f2`, `1fbd`, `dbac`, `b360`, `eebf`, `5011`. **~12 seeds.** This is the deepest fan-out in the backlog and the single highest-leverage action. Note: `ad44` is itself *ready* (no blockers) — it is simply not yet done.

**2. `audio-graph-eb6c` — Speaker timeline event schema + replay fixtures (BLOCKED by `ad44`).**
Unblocks `20f2`, `1fbd`, `dbac`, `b360`, `eebf`, `5011` (`20f2` further gates `1fbd`/`dbac`). **~6 seeds, the entire diarization sub-tree.** Resolving the `ad44`→`eb6c` pair clears nearly the whole diarization arc.

**3. `audio-graph-afca` — Dynamic processed-audio consumer registry (OPEN, ready, unblocked).**
Unblocks `a2ff`, `b5f3`, `eebf`, `e864`, `5011`. **5 seeds** spanning audio policy registry, multichannel contract, channel-aware ASR projection, backpressure UI, and local diarization worker. Also ready now.

### Secondary blockers

- **`audio-graph-0d66`** (live rsac audio CI smoke, ready) → `f166`, `09a7`, and the release epic `c395`. **3+ seeds.**
- **`audio-graph-d042`** (ASR transport/parser harness, ready) → `ad1d`, `226e`, `eb2e`. **3 seeds.**
- **`audio-graph-2b2c`** (SurrealDB file-engine eval, ready) → `48bb` → `ceda`. **2 seeds + the storage decision.**
- **`audio-graph-eee3`** (S2S orchestrator; itself blocked on `1a8c`/`f53b`/`14e0`) → `82b3`, `7fcc`. **2 seeds.**
- **`audio-graph-0c08`** (OS keychain, code-complete, blocked only on 3-OS keychain CI evidence) → `a3d8`, epic `1c2f`.
- **`audio-graph-d262`** (generic llm.api readiness, ready) → `cbde` → epic `1c2f`.
- **`audio-graph-c237`** (overlap fixtures, ready) → `dd19`.
- **`audio-graph-319c`** (Soniox smoke credential, ready) → `0b93` → `be03` → `e35f` (Soniox promotion chain).

### Pure epic umbrellas (held intentionally, do not pull forward)
- `b153` (overtake Granola/Cluely) — 15 blockers; gated behind the P1 cutline.
- `4673` (streaming transcript→notes→graph pipeline) — 5 blockers; the end-state orchestrator.
- `1c2f` (config UX + credential health epic) — 7 blockers.
- `c395` (cross-platform release-readiness matrix) — 7 blockers.
- `84f4` (OpenRouter accelerator routing) — 3 ready children; one merge cycle from done.

---

## 5. Cross-Cutting Patterns, Tech-Debt, and Risks

1. **Root-blocker concentration risk.** A single unfinished schema (`ad44`) gates ~12 seeds and the diarization sub-tree gates ~6 more behind it. Sequencing failure here stalls a third of the open backlog. This is the dominant structural risk.

2. **Clean-ref CI evidence is a systemic bottleneck.** Across audio, ci-testing, providers-llm, security-privacy, and ux-settings, many "open" seeds are *code-complete and headlessly tested* but cannot close without Blacksmith 3-OS run URLs from a clean pushed branch. The dirty 198-file checkout is the blocker. `bc1c` (ownership map) is the unlock prerequisite. **This is cheap leverage: a dozen seeds close with zero new code once the evidence pipeline runs.**

3. **rsac dependency is a release-integrity risk.** Still a sibling path dep in Cargo.toml while CI/release pin a SHA externally (`6381` closed, `fd9f` open, `c395` blocked). Cargo.lock is `.gitignored` despite Rust reproducibility being release-critical — noted in multiple seeds, no dedicated fix seed yet.

4. **SurrealDB decision is gating more than storage.** The file-engine eval (`2b2c`) blocks the embedded adapter (`48bb`), the memory-workspace UX (`ceda`), *and* the CI default-feature matrix decisions (noted in `0d58`/`74b2`/`c395` extensions). FileMemoryRepository is shipped and default, so this is a forward-looking decision, but it is touching CI planning today.

5. **Dual-harness fragmentation.** Two overlapping WebSocket harness seeds (`d042` fixture/parser, `02da` session runtime) both block `ad1d`; the split of concerns is not fully resolved, risking implementation duplication.

6. **Settings surface duplication.** SettingsPage and ExpressSetup duplicate source-blocker, readiness, native-realtime, and audio-default logic; every fix lands twice. A shared abstraction gap.

7. **Provider data-boundary metadata systematically incomplete** (`fee1` open): implemented providers carry `unknown` placeholders for retention/training/deletion. Privacy *enforcement* is strong (egress guards shipped); privacy *disclosure* lags.

8. **Privacy lifecycle gap.** Write-time egress controls are well-advanced (closed), but retention/audit/delete coverage (`c282`, `70a3`) is open and unstarted — a structural gap between write-time controls and lifecycle/delete/audit.

9. **Process-doc tax.** Recurring seeds (`32e3`, `1a60`, `085c`, `4d43`, `bc1c`) spend effort documenting commit state / methodology / ownership before feature work — a tax imposed by parallel work in a perpetually-dirty tree.

10. **Healthy disciplines worth preserving:** parser-first provider strategy (fixtures + readiness before any live socket/Settings); live-smoke gating before promotion; hardware-free vs live-smoke two-tier testing; building redaction/no-sync guards *before* the transport exists; recording deferred decisions (`6cce` secrecy wrapper, `175e` codec) rather than leaving silent debt.

11. **Moonshine local STT** has deep, well-structured scaffold (mapper, bridge, bounded polling, fail-closed probe, downloader) but is fundamentally blocked on native C API bindings — incremental progress is impossible without that external library work.

---

## 6. Recommended Wave Plan (for mission P4/P5)

Goal: maximize parallelism while respecting hard dependencies. Five waves; Wave 0 and the CI-evidence track run largely in parallel.

### Wave 0 — Unblock the unblockers (parallel, do first)
Sequential-critical because everything hangs off them, but independent of each other:
- `bc1c` — dirty-worktree ownership map (**prerequisite for the entire CI-evidence track**).
- `ad44` — event-sourced data model (**prerequisite for the diarization + pipeline tracks**).
- `319c` — provision Soniox live-smoke credential (unblocks the Soniox chain).
- `2b2c` — SurrealDB file-engine eval (unblocks storage + CI default-feature decisions).

### Wave 1 — Parallel fan-out once Wave 0 lands
Three independent parallel tracks:

- **Track A (data model, after `ad44`):** `4da5` (revision ledger) → then `eb6c` (speaker timeline) and `9c89` (artifact migration); `0d1c` (ADR supersession) and `9d93` (frontend reducers) in parallel after `4da5`.
- **Track B (audio bus, independent):** `afca` (consumer registry) — pure prerequisite — then in parallel `a2ff`, `b5f3`, `e864`.
- **Track C (CI evidence, after `bc1c`, fully parallelizable):** `f53b`, `0d58`, `74b2`, `0162`, `fbf6`, `a805` — all code-complete, just need clean-ref dispatch. Then `0d66` (live audio smoke) → `f166`, `09a7`.

### Wave 2 — Second-order dependents
- After `eb6c`: `20f2` → `1fbd`, `dbac`; plus `b360`, `eebf`, `5011`, `3588` (diarization arc).
- After `d042`/`02da`: `ad1d` roadmap, then `226e` (Gladia runtime), `eb2e` (Speechmatics runtime).
- After `2586`+`74b2`+`0d66`+`fbf6`+`fd9f`+`0162`: close release epic `c395`.
- After `0117`+`0d58`: `9279` → `14e0` (Moonshine selectable).
- Soniox chain: `0b93` → `be03` → `e35f`.
- OpenRouter: `76bd`+`8772`+`61db` (parallel) → `84f4`.

### Wave 3 — Composed features
- Local TTS `1a8c` + playback `f53b` → S2S orchestrator `eee3` → `82b3`, `7fcc`.
- `396f` (OpenAI Realtime cloud S2S) in parallel.
- Credential/UX epic: `d262`→`cbde`, `a3d8` (after `0c08` keychain CI), `ad98`, `a6d4` → epic `1c2f`.
- After `48bb` (after `2b2c`): `ceda` memory-workspace UX.

### Wave 4 — Lifecycle, compliance, roadmap
- Retention/audit: `c282`, `70a3` (independent, can start earlier if capacity allows).
- `8181` benchmark suite, `a32f` SOC2/GDPR checklist, `1971` sharing/ACLs.
- Competitive roadmap `b153` and its UX children (`75a1`, `392b`, `9284`, `8235`, `53cf`) — only after the P1 platform cutline is green.

### Sequencing notes
- **Do not start** the diarization sub-tree, the transcript ledger, or the pipeline epic before `ad44` lands — they will rebase.
- **Time-box** `b521` (Node 24 actions) before the **2026-09-16** deadline regardless of wave.
- The CI-evidence track (Track C) is the cheapest backlog reduction available and should be staffed aggressively once `bc1c` produces the merge order.
