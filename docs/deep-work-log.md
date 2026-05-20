# Deep Work Log

Chronological record of deep-work-loop runs against the audio-graph backlog.

## Run 2026-05-19 16:45 — corrected goal: Deepgram + OpenRouter pipeline

**Started at:** 34e1b1ba4544a074e814969db2c0cd7c815dc92d

**Goal:** Complete the application with a real Deepgram STT → OpenRouter LLM →
Deepgram Aura TTS pipeline (with optional graph/notes branch). Native S2S
agents (Gemini Live, gpt-realtime-2) are sibling parallel agents, not
pipeline stages. Linux + Windows are the priority CI surface; macOS deferred
until L+W are solid.

**Carry-forward research:**
- `docs/research/deepgram-aura-streaming-tts.md` — produced in a prior loop
  attempt. WebFetch/context7 were denied in that run, so URLs need re-verification
  before protocol-sensitive code is shipped.


### Phase progress (2026-05-19)

- Phase 1: ✅ baseline at 34e1b1b
- Phase 2: ✅ seeds backlog re-anchored (eee3, 396f scope clarified)
- Phase 3: ✅ 4 research reports + 1 main-thread verification overlay
  - chat-tts-integration-map.md (✅ from Explore agent)
  - audio-playback.md (✅ from prior knowledge — verified main-thread)
  - deepgram-aura-streaming-tts.md (⚠️ prior knowledge — verified main-thread)
  - openrouter-api.md (❌ agent failed — covered by main-thread verification)
  - verified-2026-05-19.md (✅ main-thread tavily-verified)
- Phase 4: ✅ ADRs 0004, 0005, 0006 written; 0003 superseded-in-part
- Phase 5: ✅ Wave A plans (A1, A2, A3); Wave B + C provisional
- Phase 6+7: 🔄 Wave A execution dispatched in 3 worktrees
  - A1: TtsProvider + Aura
  - A2: OpenRouter LLM (blocking chat)
  - A3: Streaming chat infrastructure
- Phase 8+9: pending

**ADR statuses (post-sign-off):**
- ADR-0004: accepted 2026-05-19
- ADR-0005: accepted 2026-05-19
- ADR-0006: accepted 2026-05-19, both sub-decisions

### Wave A merge status (2026-05-20)

- **A1** (TtsProvider + Aura): merged via `fe20c65` after rebase from `34e1b1b` → `d384e6d`. A1's self-reported leakage of ~30min of edits to the main worktree was cleaned up via `git restore` + phantom-stat purge before merging. Conflicts in `commands.rs` and `lib.rs` resolved by keeping both blocks (additive).
- **A2** (OpenRouter): merged via `b4866b0` after rebase. Clean merge.
- **A3** (streaming chat): original attempt crashed at ~50 tool uses with internal API error. Re-dispatched 2026-05-20 in a new worktree with SCOPED-DOWN plan — Api + OpenRouter only; LocalLlama/MistralRs/Bedrock streaming punted to follow-up issues.
- **Reviewer** for A1+A2 dispatched in parallel with A3 retry — adversarial-review pattern; no executor reasoning shared.

CI run for merged Wave A: `26177045940` queued at 2026-05-20T16:51:42Z.

### Wave A landing (2026-05-20, continued)

- **A3 retry returned successfully** with ~990 LOC across 13 files: hand-rolled SSE parser, StreamRegistry, ChatTokenDelta/Done events + frontend coalescer. LocalLlama/MistralRs/Bedrock streaming explicitly deferred (filed as `audio-graph-b373`).
- **A3 reviewer report** flagged 6 findings:
  - HIGH: finish_reason from provider not propagated (filed `audio-graph-0e34`)
  - HIGH: SSE byte-by-byte test missing (fixed inline in `662c2a1`)
  - MEDIUM: StreamRegistry::cancel TOCTOU window (filed `audio-graph-93a3`)
  - MEDIUM: appendChatTokenDelta null-guard inverted (fixed inline in `662c2a1`)
  - LOW: send_chat_message _cancel naming (filed `audio-graph-9d6d`)
  - LOW: SseDecoder.buf unbounded (filed `audio-graph-3344`)
- **A1+A2 reviewer report** had also flagged 5 findings, of which:
  - HIGH: barge-in suppression at session layer (filed `audio-graph-7107`)
  - HIGH: sample_rate hardcoded (fixed inline in `9d1c4f3`)
  - MEDIUM: 12s wall-clock keepalive test (filed `audio-graph-0e19`; later determined to be the same architectural issue that caused the Windows runtime panic — actually a P0)
  - LOW: flush_seq tearing (filed `audio-graph-d875`)
  - Streaming follow-ups for non-cloud providers (filed `audio-graph-b373`)
- **CI iteration loop**: 4 distinct fix-batch commits pushed to converge on green:
  1. `9d1c4f3` — wire OpenRouter into LlmExecutor test, derive Debug on Aura, thread sample_rate
  2. `662c2a1` — byte-by-byte SSE test + null-guard fix
  3. `f588324` — runtime ownership refactor (the real fix for the Aura Windows panic), explicit OpenRouter chat headers, frontend tts_provider field
  4. `3f87d3b` — case-insensitive header assertions in OpenRouter tests
- **Final convergence**: each round of fixes reduced failure count: 2 (initial merge compile errors) → 7 Aura panics + 1 OpenRouter header bug + 4 frontend tsc errors → 2 OpenRouter header case bugs → 0 expected.

CI run `26179731973` is the verification gate; if green, Wave A is done.
