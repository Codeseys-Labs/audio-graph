# Wave A — Overview

Wave A delivers the foundation for the speech-to-graph + speak-aloud
pipeline. Three parallel-executable tasks:

1. **A1: TtsProvider trait + Deepgram Aura skeleton** — `tts/mod.rs` +
   `tts/deepgram_aura.rs`. Per ADR-0004.
2. **A2: OpenRouter first-class LLM provider** — `LlmProvider::OpenRouter`
   variant + test/list commands + UI plumbing. Per ADR-0005.
3. **A3: Streaming chat infrastructure** — `chat-token-delta` event +
   SSE consumption in `api_client.rs` + frontend subscription. Per ADR-0006.

These three are mostly independent: A1 builds in `src-tauri/src/tts/` (new
directory), A2 touches `src-tauri/src/llm/` + settings + commands, A3
touches `src-tauri/src/llm/api_client.rs` + `executor.rs` + commands +
frontend. Some overlap in `commands.rs` between A2 and A3, but they're at
different functions (chat command for A3 vs new `test_openrouter_*` for
A2). Worktree-isolated execution recommended; merge points documented per
plan.

**Out of scope for Wave A:**
- Audio playback subsystem (Wave B, audio-graph-8d75)
- Speak-aloud loop wiring (Wave C, audio-graph-92c7)
- Local TTS engines (post-Wave-C, audio-graph-1a8c)
- Native-S2S provider (gpt-realtime-2, audio-graph-396f — separate track)

**Wave A acceptance criterion:** `cargo check && cargo clippy && cargo test`
green on Linux + Windows CI; new code paths exercised by unit tests; no
runtime regressions in chat (blocking path still works as before).

**Wave A wallclock estimate:** 1 PR per task, executable in parallel.
Plan files name the exact files each agent owns.
