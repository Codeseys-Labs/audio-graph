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
