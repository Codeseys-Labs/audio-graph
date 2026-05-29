SUCCESS: The process with PID 214776 (child process of PID 218976) has been terminated.
SUCCESS: The process with PID 218976 (child process of PID 219680) has been terminated.
SUCCESS: The process with PID 219680 (child process of PID 219040) has been terminated.
SUCCESS: The process with PID 219040 (child process of PID 215196) has been terminated.
SUCCESS: The process with PID 217860 (child process of PID 217820) has been terminated.
SUCCESS: The process with PID 217820 (child process of PID 217724) has been terminated.
SUCCESS: The process with PID 217724 (child process of PID 216656) has been terminated.
SUCCESS: The process with PID 216656 (child process of PID 210752) has been terminated.
SUCCESS: The process with PID 210752 (child process of PID 215196) has been terminated.
SUCCESS: The process with PID 215512 (child process of PID 219048) has been terminated.
SUCCESS: The process with PID 219048 (child process of PID 215196) has been terminated.
SUCCESS: The process with PID 217876 (child process of PID 217804) has been terminated.
SUCCESS: The process with PID 217804 (child process of PID 217680) has been terminated.
SUCCESS: The process with PID 217680 (child process of PID 212360) has been terminated.
SUCCESS: The process with PID 212360 (child process of PID 215048) has been terminated.
SUCCESS: The process with PID 215048 (child process of PID 215196) has been terminated.
SUCCESS: The process with PID 219568 (child process of PID 207708) has been terminated.
SUCCESS: The process with PID 207708 (child process of PID 218224) has been terminated.
SUCCESS: The process with PID 218224 (child process of PID 212584) has been terminated.
SUCCESS: The process with PID 212584 (child process of PID 219032) has been terminated.
SUCCESS: The process with PID 219032 (child process of PID 215196) has been terminated.
SUCCESS: The process with PID 215936 (child process of PID 214816) has been terminated.
SUCCESS: The process with PID 214816 (child process of PID 218108) has been terminated.
SUCCESS: The process with PID 218108 (child process of PID 210392) has been terminated.
SUCCESS: The process with PID 210392 (child process of PID 215196) has been terminated.
SUCCESS: The process with PID 218940 (child process of PID 217984) has been terminated.
SUCCESS: The process with PID 217984 (child process of PID 218664) has been terminated.
SUCCESS: The process with PID 218664 (child process of PID 218240) has been terminated.
SUCCESS: The process with PID 218240 (child process of PID 219132) has been terminated.
SUCCESS: The process with PID 219132 (child process of PID 215196) has been terminated.
SUCCESS: The process with PID 217496 (child process of PID 215444) has been terminated.
SUCCESS: The process with PID 215444 (child process of PID 215196) has been terminated.
SUCCESS: The process with PID 209772 (child process of PID 214948) has been terminated.
SUCCESS: The process with PID 214948 (child process of PID 219464) has been terminated.
SUCCESS: The process with PID 219464 (child process of PID 219060) has been terminated.
SUCCESS: The process with PID 219060 (child process of PID 215196) has been terminated.
SUCCESS: The process with PID 219536 (child process of PID 207500) has been terminated.
SUCCESS: The process with PID 207500 (child process of PID 218056) has been terminated.
SUCCESS: The process with PID 218056 (child process of PID 216848) has been terminated.
SUCCESS: The process with PID 216848 (child process of PID 215196) has been terminated.
CONFIRMED GOOD

- Backend transcript buffer is capped at 500 (`src-tauri/src/speech/mod.rs:573-579`), pending agent proposals at 200 (`src-tauri/src/speech/mod.rs:234-247`), graph at 1000/5000 (`src-tauri/src/graph/temporal.rs:31-35`), frontend transcript/turn/proposals at 500/100/50 (`src/store/index.ts:173-187`), and visible transcript DOM at 200 (`src/components/LiveTranscript.tsx:142-145`).
- Frontend graph avoids worst layout thrash: node identity is preserved (`src/store/index.ts:303-320`, `322-374`) and force simulation reheats only when node count grows (`src/components/KnowledgeGraphViewer.tsx:84-112`).
- Playback ring buffer is bounded at ~192k samples (`src-tauri/src/playback/mod.rs:103-107`).
- Whisper is loaded once per ASR worker lifetime, not per segment (`src-tauri/src/speech/mod.rs:1272-1296`). Local llama is stored in `state.llm_engine` after load/autoload (`src-tauri/src/commands.rs:828-856`, `1803-1831`).

ISSUES

- HIGH: Background extraction can grow unbounded under slow LLM/API. Every transcript submits to Rayon (`src-tauri/src/speech/mod.rs:729-733`), then blocks on the single LLM executor (`src-tauri/src/llm/executor.rs:136-167`); executor queues are plain unbounded `VecDeque`s (`src-tauri/src/llm/executor.rs:73-76`, `195-203`). In a 10-hour session with extraction slower than ~1 segment/1.5s, pending jobs/strings can OOM.
- MED: `chat_history` is unbounded and cloned wholesale on each chat request (`src-tauri/src/commands.rs:1144-1157`, `1164-1174`). A long active session with frequent chat can grow memory and prompt latency indefinitely.
- MED: Chat RAG dumps the whole graph into every prompt (`src-tauri/src/commands.rs:1097-1138`). At 1000 nodes/5000 edges this is large, slow, and token-expensive; top-k retrieval/summarized neighborhoods would scale better.
- MED: Audio hot path allocates repeatedly: mono conversion allocates per chunk (`src-tauri/src/audio/pipeline.rs:272-290`), resampler drains allocate `input_chunk` + `vec![...]` (`src-tauri/src/audio/pipeline.rs:159-176`), emitted chunks allocate (`src-tauri/src/audio/pipeline.rs:192-201`), mixer allocates frames/output (`src-tauri/src/audio/mixer.rs:89-99`; `src-tauri/src/audio/mix_math.rs:15-42`).
- MED: Streaming ASR conversion allocates per frame: Deepgram creates new PCM bytes (`src-tauri/src/asr/deepgram.rs:351-356`, `1055-1068`); AssemblyAI additionally base64-encodes each chunk (`src-tauri/src/asr/assemblyai.rs:259-265`).
- MED: Local Whisper batching adds built-in latency and duplicated work: 2s windows with 0.5s overlap (`src-tauri/src/speech/mod.rs:360-365`, `780-795`) reprocess ~25% of audio and emit only after accumulation (`src-tauri/src/speech/mod.rs:1230-1240`).
- LOW: Graph eviction scans for min on overflow (`src-tauri/src/graph/temporal.rs:283-353`). At 1000/5000 caps this is probably acceptable; snapshotting every extraction (`src-tauri/src/speech/mod.rs:453-471`) may cost more.

QUESTIONS

- Should extraction be lossy/backpressured like ASR (`try_send` drop at `src-tauri/src/speech/mod.rs:1231-1239`) instead of preserving every segment?
- Is cold-start on every transcription restart acceptable for Whisper, or should the model/context be cached across sessions?
- Are CUDA/Vulkan builds actually produced in release CI? Feature gates exist in `src-tauri/Cargo.toml`, but default is CPU-only.
