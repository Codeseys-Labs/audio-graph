# ASR Provider Refactoring Analysis: Feasibility Study

**Status:** Analysis complete, implementation pending approval
**Date:** 2026-04-16

## Executive Summary

Analysis of ~2000 lines of duplication across 6+ ASR worker implementations
identified 3 distinct architectural patterns (batch, streaming WebSocket,
streaming HTTP/2) sharing a common tail pipeline.

**Recommendation:** Option C — extract shared tail helper. 360 lines reduced,
very low risk, ~0.5 days effort. Optional Phase 2 infrastructure extractions
add 300 more lines of reduction over 5 days.

## Current State

| Provider | Lines | Pattern | Input | Transcribe |
|----------|-------|---------|-------|------------|
| Local Whisper | 175 | Batch | AccumulatedSegment (2s) | Sync blocking CPU |
| Cloud REST | 135 | Batch | AccumulatedSegment (2s) | Sync blocking HTTP |
| Deepgram | 280 | Streaming WS | ProcessedAudioChunk (32ms) | Async WS events |
| AssemblyAI | 260 | Streaming WS | ProcessedAudioChunk (32ms) | Async WS events |
| AWS Transcribe | 165 | Streaming HTTP/2 | ProcessedAudioChunk (32ms) | Async callback |
| sherpa-onnx | 160 | Streaming frame | ProcessedAudioChunk (32ms) | Sync frame-by-frame |

## The Shared Tail (appears 6× in speech/mod.rs)

```rust
// ~60 lines duplicated 6 times = 360 pure copy-paste
// 1. transcript_buffer.push + 500-item cap
// 2. transcript_writer.append (disk persist)
// 3. app_handle.emit(TRANSCRIPT_UPDATE, SPEAKER_DETECTED)
// 4. pipeline_status update (asr + diarization counts)
// 5. spawn_extraction_task (14 params)
```

## Design Options Rated

| Option | Feasibility | Reduction | Risk | Effort |
|--------|-------------|-----------|------|--------|
| **A: Full Trait** | 3/10 | 800 lines | High | 5 days |
| **B: Two Traits (Batch/Streaming)** | 6/10 | 600 lines | Medium | 4 days |
| **C: Extract Tail Only** | **9/10** | 360 lines | **Very Low** | **0.5 days** |
| **C + Infrastructure** | 8/10 | 650-700 lines | Low | 5-6 days |
| **D: Docs Only** | 10/10 | 0 lines | None | 1 day |

## Recommended: Option C

### Phase 1 (immediate, 0.5 day, very low risk)

Extract a `TranscriptProcessingContext` struct + single `emit_transcript_and_extract()`
helper. Replace 6 copies of the tail pipeline with a single call site.

```rust
pub struct TranscriptProcessingContext {
    pub transcript_buffer: Arc<RwLock<VecDeque<TranscriptSegment>>>,
    pub transcript_writer: Arc<Mutex<Option<TranscriptWriter>>>,
    pub pipeline_status: Arc<RwLock<PipelineStatus>>,
    pub app_handle: AppHandle,
    pub llm_engine: Arc<Mutex<Option<LlmEngine>>>,
    pub api_client: Arc<Mutex<Option<ApiClient>>>,
    pub mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>>,
    pub llm_provider: LlmProvider,
    pub graph_extractor: Arc<RuleBasedExtractor>,
    pub knowledge_graph: Arc<Mutex<TemporalKnowledgeGraph>>,
    pub graph_snapshot: Arc<RwLock<GraphSnapshot>>,
}

pub fn emit_transcript_and_extract(
    diarized: DiarizedTranscript,
    ctx: &TranscriptProcessingContext,
    asr_count: u64,
    diarization_count: u64,
    extraction_count: &Arc<AtomicU64>,
    graph_update_count: &Arc<AtomicU64>,
) { /* ~60 lines of the shared tail */ }
```

**Apply at 6 sites:**
1. `run_asr_worker()` ~line 1029
2. `run_cloud_asr_worker()` ~line 1424
3. `run_deepgram_event_receiver()` ~line 1795
4. `run_assemblyai_event_receiver()` ~line 2108
5. `run_aws_transcribe_speech_processor()` callback ~line 2219
6. `run_sherpa_onnx_speech_processor()` ~line 2404

### Phase 2 (optional, 4-5 days)

Extract batch accumulator loop (Whisper + Cloud) and WebSocket sender loop
(Deepgram + AssemblyAI). Gains: 300 more lines of reduction.

### Phase 3 (deferred)

Option B traits only if 3+ new streaming providers get added. Current
hetero­geneity (batch, WS events, callback, sync frame) makes a single
trait impractical.

## Why Not Option A (Full Trait)

The 4 execution models don't fit a single trait:
- Batch: `fn transcribe(segment) -> Vec<Transcript>`
- Async WS: `fn send_audio() + fn recv_event()` async
- Callback: `fn run_session(callback)`
- Sync frame: `fn process_chunk() -> Option<...>`

Forcing unification creates wrapper complexity exceeding the duplication saved.

## Risk Assessment for Option C

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Behavioral divergence | Very Low | Medium | Identical code → identical helper |
| Event emission order change | Very Low | Low | Preserve order in helper |
| Performance regression | Very Low | Very Low | Helper inlined |
| Test gaps | Medium | Medium | Add integration test before rollout |

**Overall: Very Low.** Option C is mechanical refactoring of identical code.
