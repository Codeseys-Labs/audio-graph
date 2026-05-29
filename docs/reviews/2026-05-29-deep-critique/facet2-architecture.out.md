SUCCESS: The process with PID 211140 (child process of PID 200312) has been terminated.
SUCCESS: The process with PID 200312 (child process of PID 209400) has been terminated.
SUCCESS: The process with PID 209400 (child process of PID 211648) has been terminated.
SUCCESS: The process with PID 211648 (child process of PID 201812) has been terminated.
SUCCESS: The process with PID 211832 (child process of PID 211780) has been terminated.
SUCCESS: The process with PID 211780 (child process of PID 211728) has been terminated.
SUCCESS: The process with PID 211728 (child process of PID 193292) has been terminated.
SUCCESS: The process with PID 193292 (child process of PID 209904) has been terminated.
SUCCESS: The process with PID 209904 (child process of PID 201812) has been terminated.
SUCCESS: The process with PID 212872 (child process of PID 212836) has been terminated.
SUCCESS: The process with PID 212836 (child process of PID 212792) has been terminated.
SUCCESS: The process with PID 212792 (child process of PID 205140) has been terminated.
SUCCESS: The process with PID 205140 (child process of PID 211748) has been terminated.
SUCCESS: The process with PID 211748 (child process of PID 201812) has been terminated.
SUCCESS: The process with PID 158704 (child process of PID 209816) has been terminated.
SUCCESS: The process with PID 209816 (child process of PID 211928) has been terminated.
SUCCESS: The process with PID 211928 (child process of PID 22980) has been terminated.
SUCCESS: The process with PID 22980 (child process of PID 201812) has been terminated.
SUCCESS: The process with PID 204128 (child process of PID 207924) has been terminated.
SUCCESS: The process with PID 207924 (child process of PID 212172) has been terminated.
SUCCESS: The process with PID 212172 (child process of PID 211756) has been terminated.
SUCCESS: The process with PID 211756 (child process of PID 201812) has been terminated.
SUCCESS: The process with PID 212292 (child process of PID 208916) has been terminated.
SUCCESS: The process with PID 208916 (child process of PID 211952) has been terminated.
SUCCESS: The process with PID 211952 (child process of PID 208472) has been terminated.
SUCCESS: The process with PID 208472 (child process of PID 201812) has been terminated.
SUCCESS: The process with PID 210800 (child process of PID 211812) has been terminated.
SUCCESS: The process with PID 211812 (child process of PID 201812) has been terminated.
SUCCESS: The process with PID 211920 (child process of PID 211896) has been terminated.
SUCCESS: The process with PID 211896 (child process of PID 211824) has been terminated.
SUCCESS: The process with PID 211824 (child process of PID 169224) has been terminated.
SUCCESS: The process with PID 169224 (child process of PID 208312) has been terminated.
SUCCESS: The process with PID 208312 (child process of PID 201812) has been terminated.
SUCCESS: The process with PID 212808 (child process of PID 212736) has been terminated.
SUCCESS: The process with PID 212736 (child process of PID 212592) has been terminated.
SUCCESS: The process with PID 212592 (child process of PID 211740) has been terminated.
SUCCESS: The process with PID 211740 (child process of PID 211792) has been terminated.
SUCCESS: The process with PID 211792 (child process of PID 201812) has been terminated.
SUCCESS: The process with PID 210968 (child process of PID 199132) has been terminated.
SUCCESS: The process with PID 199132 (child process of PID 201812) has been terminated.
CONFIRMED GOOD

- Shared tail is partly factored: `TranscriptProcessingContext` and `emit_transcript_and_extract` centralize buffer/persist/events/extraction for ASR finals (`src-tauri/src/speech/mod.rs:503-565`, `573-660`).
- Extraction itself is also centralized in `process_extraction_and_emit` (`src-tauri/src/speech/mod.rs:399-423`), and the bounded Rayon pools show conscious long-session maintenance work (`src-tauri/src/speech/mod.rs:16-44`, `729-733`).
- Event names/payloads are normalized in `events.rs`, including provider-neutral turn events (`src-tauri/src/events.rs:132-153`, `285-296`).

ISSUES

HIGH

- `speech/mod.rs` is doing too much: routing provider selection, local Whisper model validation/loading, audio accumulation, diarization, extraction, graph updates, agent proposal generation, and 4 provider processors in one ~2.7k-line file. Natural seams: `accumulator.rs` around `AudioAccumulator` (`src-tauri/src/speech/mod.rs:740-755`), `tail.rs` for `emit_transcript_and_extract` (`503-660`), `extraction.rs` for `process_extraction_and_emit` (`399-497`), `agent.rs` for proposal logic (`185-330`), and `providers/{local,cloud,deepgram,assemblyai,aws,sherpa}.rs` for processor loops (`849-1095`, `1804-2491`, `2497-2758`).

MED

- Streaming ASR duplication is factorable. Worst copy-paste is Deepgram vs AssemblyAI client/session machinery: both build a dedicated one-worker Tokio runtime, create `audio_tx`, keep `connected/user_disconnected/pending_chunks`, run `session_task`, share the same 1/2/5/10 backoff, emit reconnect events, and preserve audio queues (`src-tauri/src/asr/deepgram.rs:239-270`, `532-699`; `src-tauri/src/asr/assemblyai.rs:163-214`, `379-508`). A generic `StreamingWsAsrSession` helper could own runtime/channel/backoff/reconnect while providers supply `open_ws`, audio-frame encoding, setup replay, and event parsing. AWS has a different SDK event-stream shape but still shares ΓÇ£audio sender + event receiver + lifecycle callbackΓÇ¥ concerns (`src-tauri/src/asr/aws_transcribe.rs:139-153`, `166-242`). Sherpa is local/no Tokio runtime, but shares the speech-side processor/diarization/tail loop (`src-tauri/src/speech/mod.rs:2606-2758`).

MED

- Provider abstraction is mostly enum-match dispatch, not trait-polymorphic. ASR is routed through early-return matches in `run_speech_processor` (`src-tauri/src/speech/mod.rs:928-1095`), while LLM provider selection/fallback is hardcoded in executor match ladders (`src-tauri/src/llm/executor.rs:260-279`, `282-310`). Pros: simple serialization/settings and explicit fallback order. Cons: every new provider touches central orchestration, settings, credentials, command sync, and executor routing, weakening the provider-agnostic design.

MED

- Error handling is mixed. `AppError` exists and explicitly says legacy string errors lose structure (`src-tauri/src/error.rs:1-14`, `107-118`), but many backend boundaries still return `Result<_, String>` or `map_err(format!)`, including ASR workers (`src-tauri/src/asr/mod.rs:220-251`), AWS (`src-tauri/src/asr/aws_transcribe.rs:145-149`), LLM executor chat (`src-tauri/src/llm/executor.rs:174-191`), and many commands. Critical event emits often use `let _ =` instead of `events::emit_or_log` (`src-tauri/src/speech/mod.rs:442`, `588-590`; `src-tauri/src/commands.rs:2221`, `2256`, `2328`).

LOW

- Gemini uses the extraction helper but bypasses the transcript tail and duplicates speech semantics: it hardcodes speaker `"Gemini"`, empty context, a generated segment id, and wall-clock timestamp (`src-tauri/src/commands.rs:2223-2252`). That avoids transcript persistence/agent proposal emission and may diverge from normal speech-path graph context.

LOW

- `AsrWorker::run` appears effectively dead/bypassed: only `transcribe_segment` is used by `speech::run_asr_worker` (`src-tauri/src/asr/mod.rs:119-209`; uses at `src-tauri/src/speech/mod.rs:1299-1339`). TODO/FIXME debt is light, but there are several `#[allow(dead_code)]` markers in provider/client types (`src-tauri/src/asr/deepgram.rs:447-449`, `src-tauri/src/asr/assemblyai.rs:132`, `346`).

QUESTIONS

- Should cloud/streaming ASR reconnect/status behavior be normalized into provider-neutral `AsrEvent` before `speech` sees it?
- Is Gemini transcription intended to become first-class transcript input, or only graph-enrichment side input?
