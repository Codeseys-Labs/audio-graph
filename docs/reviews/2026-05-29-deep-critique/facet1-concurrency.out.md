SUCCESS: The process with PID 206304 (child process of PID 202860) has been terminated.
SUCCESS: The process with PID 202860 (child process of PID 209936) has been terminated.
SUCCESS: The process with PID 201904 (child process of PID 168436) has been terminated.
SUCCESS: The process with PID 168436 (child process of PID 209936) has been terminated.
SUCCESS: The process with PID 200800 (child process of PID 155196) has been terminated.
SUCCESS: The process with PID 155196 (child process of PID 198924) has been terminated.
SUCCESS: The process with PID 198924 (child process of PID 193892) has been terminated.
SUCCESS: The process with PID 193892 (child process of PID 205192) has been terminated.
SUCCESS: The process with PID 205192 (child process of PID 209936) has been terminated.
SUCCESS: The process with PID 209128 (child process of PID 204412) has been terminated.
SUCCESS: The process with PID 204412 (child process of PID 117584) has been terminated.
SUCCESS: The process with PID 117584 (child process of PID 203260) has been terminated.
SUCCESS: The process with PID 203260 (child process of PID 114504) has been terminated.
SUCCESS: The process with PID 114504 (child process of PID 209936) has been terminated.
SUCCESS: The process with PID 210180 (child process of PID 208064) has been terminated.
SUCCESS: The process with PID 208064 (child process of PID 206472) has been terminated.
SUCCESS: The process with PID 206472 (child process of PID 209640) has been terminated.
SUCCESS: The process with PID 209640 (child process of PID 209936) has been terminated.
SUCCESS: The process with PID 197468 (child process of PID 158972) has been terminated.
SUCCESS: The process with PID 158972 (child process of PID 209076) has been terminated.
SUCCESS: The process with PID 209076 (child process of PID 208252) has been terminated.
SUCCESS: The process with PID 208252 (child process of PID 209936) has been terminated.
SUCCESS: The process with PID 210448 (child process of PID 208204) has been terminated.
SUCCESS: The process with PID 208204 (child process of PID 153772) has been terminated.
SUCCESS: The process with PID 153772 (child process of PID 203724) has been terminated.
SUCCESS: The process with PID 203724 (child process of PID 207716) has been terminated.
SUCCESS: The process with PID 207716 (child process of PID 209936) has been terminated.
SUCCESS: The process with PID 209212 (child process of PID 190396) has been terminated.
SUCCESS: The process with PID 190396 (child process of PID 203404) has been terminated.
SUCCESS: The process with PID 203404 (child process of PID 206544) has been terminated.
SUCCESS: The process with PID 206544 (child process of PID 209936) has been terminated.
SUCCESS: The process with PID 202880 (child process of PID 194384) has been terminated.
SUCCESS: The process with PID 194384 (child process of PID 210360) has been terminated.
SUCCESS: The process with PID 210360 (child process of PID 202780) has been terminated.
SUCCESS: The process with PID 202780 (child process of PID 209936) has been terminated.
SUCCESS: The process with PID 206756 (child process of PID 210536) has been terminated.
SUCCESS: The process with PID 210536 (child process of PID 206152) has been terminated.
SUCCESS: The process with PID 206152 (child process of PID 170460) has been terminated.
SUCCESS: The process with PID 170460 (child process of PID 209976) has been terminated.
SUCCESS: The process with PID 209976 (child process of PID 209936) has been terminated.
CONFIRMED GOOD

- No evidence that `knowledge_graph: Arc<Mutex<_>>` is held across LLM/HTTP: extraction is completed before taking the graph mutex (`src-tauri/src/speech/mod.rs:414-423`), then graph mutation/snapshot happens under the lock (`431-473`).
- Poison recovery is generally intentional in hot paths: graph mutex uses `unwrap_or_else(|e| e.into_inner())` (`speech/mod.rs:431-434`), executor queue/backend locks recover similarly (`llm/executor.rs:197`, `210-212`, `314-335`), session rotation recovers poisoned locks (`state.rs:367-383`). Remaining `.unwrap()` hits found are test-only in the searched files.
- CPU/HTTP ASR/LLM work is mostly off the async runtime: Whisper/cloud ASR run on OS threads (`speech/mod.rs:1165-1183`, `1627-1642`); blocking LLM API calls run on the single LLM executor thread and release client mutexes before HTTP (`llm/executor.rs:331-338`, `354-362`).

ISSUES

HIGH ΓÇö Stop/start can orphan duplicate consumers. `stop_transcribe` only clears `is_transcribing` and drops stored handles, without joining (`commands.rs:929-939`). If start is called again before the old accumulator/ASR worker observes the flag, `is_transcribing` becomes true again (`commands.rs:904-905`) and a second speech processor can consume the same `speech_audio_rx`, splitting audio. Same pattern exists for Gemini: `stop_gemini` sets active false and drops handles without joining (`commands.rs:2379-2409`), while old sender threads block in `gemini_rx.recv()` (`commands.rs:2144`) and can resume after restart.

MED ΓÇö Final audio segments are silently droppable. Normal backpressure drop is defensible for live audio, but shutdown flush uses `try_send` and ignores `Full` for final accumulated segments (`speech/mod.rs:1244-1247`, `1677-1679`). This can lose the tail of an utterance exactly when the user stops transcription.

MED ΓÇö Blocking crossbeam sends occur inside single-worker Tokio websocket runtimes. Gemini creates a 1-worker runtime (`gemini/mod.rs:335-341`) and sends into bounded crossbeam event channels (`gemini/mod.rs:291`, e.g. `1255`, `1288`, `1306`). If the command event thread is slow because it runs graph/LLM extraction synchronously (`commands.rs:2232-2252`), `Sender::send` can block the async websocket runtime. Deepgram/AssemblyAI have the same shape.

MED ΓÇö Gemini audio queue is unbounded. `tokio_mpsc::unbounded_channel::<AudioCmd>()` (`gemini/mod.rs:380`) plus `send_audio` always queues during reconnect by design (`gemini/mod.rs:417-443`). A prolonged reconnect or slow socket can grow memory without bound.

LOW ΓÇö Start guards are not atomic. `start_gemini` checks active under a read lock (`commands.rs:2056-2067`) and sets it later (`2113-2115`); concurrent invocations can both pass the guard before either writes.

QUESTIONS

- Is losing tail audio on stop acceptable UX? If not, flush should use bounded blocking send with timeout or drain/join.
- Should stop commands provide a ΓÇ£fully stoppedΓÇ¥ guarantee before allowing restart? Currently they are signal-only.
