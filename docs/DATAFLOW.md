# AudioGraph — Data Flow (parallel vs sequential, knowledge accumulation, agent)

This document maps how data moves through AudioGraph: which stages run in
parallel, which are sequential, how the knowledge graph accumulates, and how
the AI-agent pieces fit. File references are `path:symbol` for grounding.

> Verified against a live session log (`%APPDATA%\audio-graph\logs\`): 40
> selected sources each spawned a capture thread, per-source resamplers were
> created, Deepgram streamed interim transcripts, and graph deltas were emitted
> — matching the flow below.

---

## 1. Threads & channels (the concurrency skeleton)

AudioGraph is a pipeline of **independent threads connected by channels**
(`crossbeam_channel`). Each box below is its own OS thread; arrows are channels.

```
 N capture threads            1 pipeline thread        1 dispatcher thread
 (one per selected source) ─► AudioPipeline       ─►  fan-out (clone)
   capture_thread_fn            process_chunk             │
   src-tauri/src/audio/         src-tauri/src/audio/      ├─► speech_audio ─► speech processor thread
   capture.rs                   pipeline.rs               └─► gemini_audio ─► Gemini WS thread (optional)
        │                            │
   raw AudioChunk               ProcessedAudioChunk
   (native rate/ch)             (16 kHz mono, 512-frame, tagged by source_id)
```

- **Capture (parallel, N threads).** `start_capture` spawns one thread per
  selected source (`capture.rs` ~`capture_thread_fn`). Each negotiates a
  device-supported format (`choose_capture_format`) and pushes native-format
  `AudioChunk`s into **one shared** `pipeline_tx`.
- **Resample/downmix (1 thread, per-source state).** `AudioPipeline`
  (`audio/pipeline.rs`) downmixes to mono + resamples to **16 kHz** with
  **independent rubato state per `source_id`** — sources stay logically
  separate here; they are *not* summed.
- **Dispatcher (1 thread).** Clones each `ProcessedAudioChunk` to the speech
  path and (when enabled) the Gemini path (`commands.rs`, dispatcher spawn).
  → **The two pipelines run in parallel and independently** (ADR-0001/0006).

These stages are a **sequential pipeline per chunk** (capture → resample →
dispatch) but run **concurrently across chunks and across sources** — while
chunk _n_ is being resampled, capture is already producing chunk _n+1_.

---

## 2. The two LLM-facing pipelines (parallel, sibling surfaces)

### A. Cascading pipeline: STT → LLM → TTS (default)

```
speech_audio ─► speech processor ─► ASR ─► diarization ─► transcript segment
              (run_speech_processor,   │                      │
               speech/mod.rs)          │                      ├─► emit TRANSCRIPT_UPDATE (frontend transcript)
                                       │                      ├─► persist to disk
                                       │                      ├─► spawn_agent_proposal_task (heuristic, parallel)
                                       │                      └─► spawn_extraction_task (LLM, parallel, background)
                                       │
   ASR provider chosen at runtime:     │
   - Deepgram streaming (1 WebSocket)  │  ← single-source today (see §5)
   - AssemblyAI / AWS / Sherpa stream  │
   - Local Whisper / cloud batch       │  ← per-source, accumulated independently
```

- **ASR is sequential within a source** but the **downstream steps fan out in
  parallel**: once a transcript segment exists
  (`emit_transcript_and_extract`, `speech/mod.rs`), it simultaneously
  (a) emits the transcript event, (b) persists, (c) fires a **heuristic agent
  proposal** task, and (d) fires a **background LLM extraction** task. None of
  these block the ASR loop.
- **Extraction runs on a priority executor** (`llm/executor.rs`,
  `LlmExecutor`): a dedicated thread with an **interactive** queue (chat) and a
  **background** queue (extraction). Interactive work preempts background so a
  long extraction never blocks chat. Cloud clients are **cloned out of their
  mutex before the blocking HTTP call** so extraction and chat never deadlock
  on the same lock. A **429 cooldown** pauses background extraction for 60 s.

### B. Native speech-to-speech (optional): Gemini Live / OpenAI realtime

```
gemini_audio ─► Gemini WS thread ─► native S2S model ─► transcript + (optional) audio
```

- Runs **fully in parallel** with the cascading pipeline (you can run both =
  "comparison mode"). Gated by the **Conversation mode** setting; its top-bar
  control only appears when native S2S is enabled.

---

## 3. Knowledge accumulation (how the graph grows)

```
transcript segment ──► sliding window (last 6 segments, read-only context)
        │                         │
        └──► spawn_extraction_task(text, speaker, context)   [background thread]
                         │
                         ▼
            LlmExecutor.extract_entities  ──► ontology-guided prompt
            (llm/executor.rs)                 (ontology.rs: Person/Org/Topic/
                         │                      Question/Task/Decision/…)
                         ▼
            ExtractionResult { entities, relations }
                         │
                         ▼
            TemporalKnowledgeGraph.process_extraction   [graph/temporal.rs]
              - upsert nodes by lowercased name (mention_count++)
              - add typed relations
              - bump event_counter, evict beyond capacity
                         │
            ┌────────────┴───────────────┐
            ▼                            ▼
   GRAPH_DELTA (every cycle)     GRAPH_UPDATE (full snapshot, every 10th)
            │                            │
            └──────────► frontend store ◄┘
                     setGraphSnapshot / applyGraphDelta
                     (preserve node identity + seed new-node positions)
                            │
                ┌───────────┴───────────┐
                ▼                       ▼
        KnowledgeGraphViewer        NotesPanel (derived: Participants,
        (react-force-graph)         Questions, Tasks, Decisions, Topics)
```

- **Accumulation is incremental + temporal.** Each extraction upserts into a
  single long-lived `TemporalKnowledgeGraph`; repeated mentions increment
  `mention_count` (node size), and the graph tracks first/last-seen + an event
  counter for eviction. It is **not** rebuilt per segment.
- **Sliding-window context** (last 6 segments) is passed as *read-only* context
  so the extractor resolves "this/here/it" and links a segment to the ongoing
  conversation — while still extracting only from the current segment.
- **The frontend never recomputes the graph** — the backend is the source of
  truth and pushes deltas/snapshots. **Notes are a pure client-side projection**
  of the typed graph (no extra LLM call), so they accumulate as the graph does.
- **Ordering note:** extraction is background and async, so graph updates for a
  segment land *slightly after* its transcript line — by design (transcript is
  never blocked on the LLM).

---

## 4. The AI-agent pieces

There are **three distinct agent-ish surfaces**, all decoupled:

1. **Heuristic proposals (local, no LLM).** `agent_proposal_kind`
   (`speech/mod.rs`) classifies each segment by simple rules into
   `Question` / `Task(GraphSuggestion)` / `Note` and emits an `agent-proposal`
   event. **Never calls the network** → never rate-limits.
   - **Questions default to the graph:** the frontend auto-calls
     `add_question_to_graph` (local: adds a `Question` node + `asks` relation),
     and the proposal card offers an **optional "Ask AI"** (routes to chat).
2. **Background extraction (LLM).** §3 — the real knowledge builder.
3. **Interactive chat (LLM, streaming).** `start_streaming_chat`
   (`commands.rs`) builds context from the graph + transcript and streams token
   deltas (`chat-token-delta` → `chat-token-done`). Runs on the executor's
   **interactive** queue (preempts extraction). Errors (e.g. 429) surface in the
   chat bubble instead of hanging.

```
segment ─┬─► heuristic proposal ─► agent-proposal event ─► overlay card
         │        └─(question)─► auto add_question_to_graph + optional Ask AI ─┐
         └─► background extraction ─► graph                                    │
                                                                               ▼
user prompt ─► start_streaming_chat ─► (graph+transcript context) ─► streamed reply
```

---

## 5. What's parallel vs sequential — summary

| Stage | Parallel? | Notes |
|---|---|---|
| Capture (per source) | **Parallel** (N threads) | one thread per selected source |
| Resample/downmix | Concurrent w/ capture | 1 thread, per-source rubato state |
| Cascading vs Gemini pipelines | **Parallel** | independent; "comparison mode" runs both |
| ASR within a source | Sequential | streaming socket or batch accumulator |
| transcript → {emit, persist, proposal, extraction} | **Parallel fan-out** | none blocks the ASR loop |
| Extraction vs Chat | Concurrent, **prioritized** | interactive preempts background; 429 cooldown |
| Graph updates | Sequential into one graph | delta every cycle, snapshot every 10th |
| Notes | Derived (client) | projection of the typed graph |

### Multi-source streaming (the mixer)
Streaming ASR is **one WebSocket**, so multiple sources can't each open their
own. **Deepgram now routes through an `AudioMixer`** (`audio/mixer.rs`,
`spawn_mixer`) inserted in front of the Deepgram worker: it sums the per-source
16 kHz-mono streams into one mixed stream (per-source ring buffers absorb
jitter, laggards are silence-filled, sum is scaled by 1/sqrt(active) and
clamped, idle sources evicted after 2 s). It's transparent for a single source.
`validate_streaming_asr_source_count` (`commands.rs`) therefore **no longer
limits Deepgram** to one source. AssemblyAI/AWS/Sherpa keep the one-source limit
until the mixer is wired into their branches too. Batch ASR (Whisper/cloud)
already handles N sources independently and is unaffected.
