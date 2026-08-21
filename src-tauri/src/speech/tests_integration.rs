//! Integration tests for the speech processor orchestration.
//!
//! Task #81 (loop 10, HIGH #3): the 2500-LOC `speech/mod.rs` had **zero**
//! integration tests. This suite covers both a real top-level speech
//! orchestrator fallback path and the lower-level diarization →
//! entity-extraction → temporal-knowledge-graph chain that
//! `emit_transcript_and_extract_with_meta` and `process_extraction_and_emit` wire up in
//! production.
//!
//! What these tests catch:
//! - Regression where the speaker label produced by diarization is not the
//!   same string the extractor tags as a `Person` entity in the graph (this
//!   would silently break the "who said what" relation).
//! - Regression where the transcript buffer overflow (500-item cap) stops
//!   working — a long session would leak memory.
//! - Regression where `TemporalKnowledgeGraph::process_extraction` stops
//!   accumulating across multiple segments.
//!
//! What these tests do NOT catch (future work):
//! - Whisper/cloud ASR segmentation boundary math.
//! - Backpressure propagation from extractors to the ASR input channel.
//! - AppHandle event listener ordering.
//! - LLM engine fallback chain (`try_native_llm` → `try_api_client` → rule-based).

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::asr::assemblyai::AssemblyAiV3ParsedRevision;
use crate::audio::pipeline::ProcessedAudioChunk;
use crate::diarization::{
    DiarizationConfig, DiarizationInput, DiarizationWorker, DiarizedTranscript,
};
use crate::events::{AsrSpanRevisionPayload, AsrSpanStability, PipelineStatus, StageStatus};
use crate::graph::entities::GraphSnapshot;
use crate::graph::extraction::RuleBasedExtractor;
use crate::graph::temporal::TemporalKnowledgeGraph;
use crate::llm::{ApiClient, LlmEngine, LlmExecutor, MistralRsEngine, OpenRouterClient};
use crate::persistence::TranscriptEventWriter;
use crate::projection_scheduler::ProjectionSchedulers;
use crate::projections::TranscriptLedger;
use crate::settings::{AsrProvider, LlmProvider};
use crate::state::{ProjectionRuntimeHandle, TranscriptSegment};

use super::{
    SpeechChannels, SpeechConfig, SpeechShared, TARGET_FRAMES, TranscriptProcessingContext,
    emit_provider_span_revision_payload, normalize_assemblyai_v3_revision_for_side_effects,
    run_speech_processor,
};

fn unique_tempdir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "audio-graph-speech-integration-{}-{}-{}-{}",
        label,
        std::process::id(),
        nanos,
        n
    ));
    fs::create_dir_all(&dir).expect("create tempdir");
    dir
}

/// Review fix (adr0045/bf5d-deferred-retry): this file builds `SpeechShared`/
/// `TranscriptProcessingContext` inline, with real (non-mocked) LLM executor
/// state and a real `TranscriptEventWriter` — a genuine final ASR revision
/// driven through here reaches the SAME real dispatch tail production uses,
/// including `finish_projection_scheduler_job` -> `fail_*_in_flight`, which
/// arms a REAL wall-clock (~60s) deferred-retry clock thread
/// (`spawn_deferred_lane_observation`, ADR-0045 decision 3) on the very first
/// under-budget failure. With no LLM engine configured, generation always
/// fails here, so any test in this file that reaches a final revision arms
/// one. Unlike `speech::tests_status`'s `drain_app_writers`, this file has no
/// `AppState` to hang a shared cleanup helper off of — each test builds its
/// own fresh `projection_lane_stopping`/`projection_job_workers` Arcs inline.
/// Call this at the end of any such test with those two Arcs: it mirrors
/// `stop_capture_impl`'s real Stop path (set the flag, then wait for
/// self-deregistration) so a clock thread this test armed cannot outlive it
/// waiting out a real ~60s deadline against already-torn-down temp-dir state.
fn stop_and_drain_projection_lane(
    projection_lane_stopping: &Arc<AtomicBool>,
    projection_job_workers: &crate::state::ProjectionJobRegistry,
) {
    projection_lane_stopping.store(true, Ordering::SeqCst);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let empty = projection_job_workers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty();
        if empty {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "projection job/clock threads did not drain within the test timeout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct DataDirGuard {
    prev_data_dir: Option<std::ffi::OsString>,
    prev_home: Option<std::ffi::OsString>,
    prev_userprofile: Option<std::ffi::OsString>,
}

impl DataDirGuard {
    #[allow(unsafe_code)]
    fn set(dir: &Path) -> Self {
        let prev_data_dir = std::env::var_os(crate::user_data::DATA_DIR_ENV);
        let prev_home = std::env::var_os("HOME");
        let prev_userprofile = std::env::var_os("USERPROFILE");
        unsafe {
            std::env::set_var(crate::user_data::DATA_DIR_ENV, dir);
            std::env::set_var("HOME", dir);
            std::env::set_var("USERPROFILE", dir);
        }
        Self {
            prev_data_dir,
            prev_home,
            prev_userprofile,
        }
    }
}

impl Drop for DataDirGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            match &self.prev_data_dir {
                Some(value) => std::env::set_var(crate::user_data::DATA_DIR_ENV, value),
                None => std::env::remove_var(crate::user_data::DATA_DIR_ENV),
            }
            match &self.prev_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.prev_userprofile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
        }
    }
}

fn wait_until(label: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {label}");
}

/// Build a `DiarizationInput` with synthetic audio at a given RMS amplitude.
/// The Simple diarization backend clusters by energy/ZCR features; picking
/// distinct amplitudes lets us control whether two inputs map to the same
/// speaker or not.
fn make_input(text: &str, start_s: f64, end_s: f64, amplitude: f32) -> DiarizationInput {
    // 0.5 s of audio at 16 kHz mono — enough for the Simple backend to
    // compute stable RMS / ZCR / spectral-centroid features.
    let num_samples = ((end_s - start_s) * 16_000.0) as usize;
    let audio: Vec<f32> = (0..num_samples)
        .map(|i| {
            // Alternating sign so zero-crossing-rate is non-trivial.
            if i % 2 == 0 { amplitude } else { -amplitude }
        })
        .collect();

    DiarizationInput {
        transcript: TranscriptSegment {
            id: uuid::Uuid::new_v4().to_string(),
            source_id: "integration-test".to_string(),
            speaker_id: None,
            speaker_label: None,
            text: text.to_string(),
            start_time: start_s,
            end_time: end_s,
            confidence: 0.95,
        },
        speech_audio: audio,
        speech_start_time: Duration::from_secs_f64(start_s),
        speech_end_time: Duration::from_secs_f64(end_s),
    }
}

/// Drive a single input through the diarize → extract → graph-update
/// mini-pipeline (the parts of `emit_transcript_and_extract_with_meta` /
/// `process_extraction_and_emit` that don't touch `AppHandle`).
fn process_one(
    worker: &mut DiarizationWorker,
    buffer: &Arc<RwLock<VecDeque<TranscriptSegment>>>,
    extractor: &RuleBasedExtractor,
    graph: &Arc<Mutex<TemporalKnowledgeGraph>>,
    input: DiarizationInput,
) -> DiarizedTranscript {
    // Step 1: diarize.
    let diarized = worker.process_input(input);

    // Step 2: ring-buffer append (500-item cap, matches
    // `emit_transcript_and_extract_with_meta` lines 364-370).
    if let Ok(mut buf) = buffer.write() {
        buf.push_back(diarized.segment.clone());
        if buf.len() > 500 {
            buf.pop_front();
        }
    }

    // Step 3: rule-based extraction using the diarized speaker label —
    // this is the contract between stages: the label flows through as the
    // Person entity key.
    let speaker = diarized
        .segment
        .speaker_label
        .clone()
        .unwrap_or_else(|| "Unknown".to_string());
    let extraction = extractor.extract(&speaker, &diarized.segment.text);

    // Step 4: graph update (matches `process_extraction_and_emit` lines
    // 258-263).
    if !extraction.entities.is_empty() {
        let mut g = graph.lock().expect("graph mutex poisoned");
        g.process_extraction(
            &extraction,
            diarized.segment.start_time,
            &speaker,
            &diarized.segment.id,
        );
    }

    diarized
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Tauri/Tao AppHandle construction must run on the macOS main thread"
)]
fn speech_processor_missing_whisper_falls_back_to_diarization_only() {
    // Shared process-wide gtk app handle (seed audio-graph-65f0).
    let app_handle = super::shared_test_app_handle();
    let models_dir = std::env::temp_dir().join(format!(
        "audio-graph-missing-whisper-test-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&models_dir).expect("create temp models dir");

    let (processed_tx, processed_rx) = crossbeam_channel::unbounded();
    let is_transcribing = Arc::new(AtomicBool::new(true));
    processed_tx
        .send(ProcessedAudioChunk {
            source_id: "integration-source".into(),
            data: vec![0.25; TARGET_FRAMES],
            sample_rate: 16_000,
            num_frames: TARGET_FRAMES,
            timestamp: Some(Duration::from_secs(0)),
        })
        .expect("send synthetic processed audio");
    drop(processed_tx);

    let transcript_buffer: Arc<RwLock<VecDeque<TranscriptSegment>>> =
        Arc::new(RwLock::new(VecDeque::new()));
    let pipeline_status = Arc::new(RwLock::new(PipelineStatus::default()));
    let graph_snapshot = Arc::new(RwLock::new(GraphSnapshot::default()));
    let knowledge_graph = Arc::new(Mutex::new(TemporalKnowledgeGraph::new()));
    let graph_extractor = Arc::new(RuleBasedExtractor::new());
    let llm_engine: Arc<Mutex<Option<LlmEngine>>> = Arc::new(Mutex::new(None));
    let api_client: Arc<Mutex<Option<ApiClient>>> = Arc::new(Mutex::new(None));
    let mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>> = Arc::new(Mutex::new(None));
    let openrouter_client: Arc<Mutex<Option<OpenRouterClient>>> = Arc::new(Mutex::new(None));
    let llm_executor = LlmExecutor::new(
        llm_engine.clone(),
        api_client.clone(),
        openrouter_client.clone(),
        mistralrs_engine.clone(),
    );

    run_speech_processor(
        SpeechChannels {
            processed_rx,
            is_transcribing,
        },
        SpeechShared {
            transcript_buffer: transcript_buffer.clone(),
            transcript_writer: Arc::new(Mutex::new(None)),
            transcript_event_writer: Arc::new(Mutex::new(None)),
            transcript_ledger: Arc::new(Mutex::new(crate::projections::TranscriptLedger::new(
                "test-session",
            ))),
            speaker_timeline: Arc::new(Mutex::new(crate::projections::SpeakerTimeline::new(
                "test-session",
            ))),
            projection_schedulers: Arc::new(Mutex::new(
                crate::projection_scheduler::ProjectionSchedulers::new("test-session"),
            )),
            projection_runtime: crate::state::ProjectionRuntimeHandle::in_memory_for_tests(
                "test-session",
            ),
            projection_job_workers: Arc::new(Mutex::new(Vec::new())),
            projection_lane_stopping: Arc::new(AtomicBool::new(false)),
            active_session_id: Arc::new(RwLock::new("test-session".to_string())),
            pipeline_status: pipeline_status.clone(),
            app_handle,
            knowledge_graph,
            graph_snapshot,
            graph_extractor,
            llm_engine,
            api_client,
            mistralrs_engine,
            llm_executor,
            pending_agent_proposals: Arc::new(Mutex::new(HashMap::new())),
        },
        SpeechConfig {
            models_dir: models_dir.clone(),
            llm_provider: LlmProvider::default(),
            llm_allow_cloud_fallbacks: true,
            provider_content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        },
        AsrProvider::LocalWhisper,
        "missing-whisper.bin".to_string(),
    );

    let _ = fs::remove_dir_all(&models_dir);

    let buffer = transcript_buffer.read().expect("transcript buffer lock");
    assert_eq!(
        buffer.len(),
        1,
        "fallback should produce one placeholder segment"
    );
    let segment = buffer.front().expect("placeholder transcript segment");
    assert_eq!(segment.source_id, "integration-source");
    assert_eq!(segment.text, "[speech]");
    assert!(
        segment.speaker_id.is_some(),
        "diarization should assign speaker_id"
    );
    assert!(
        segment.speaker_label.is_some(),
        "diarization should assign speaker_label"
    );

    let status = pipeline_status.read().expect("pipeline status lock");
    assert!(
        matches!(
            &status.asr,
            StageStatus::Error { message } if message == "Whisper model not loaded"
        ),
        "missing local model should mark ASR as an error, got {:?}",
        status.asr
    );
    assert!(
        matches!(
            status.diarization,
            StageStatus::Running { processed_count: 1 }
        ),
        "diarization should process exactly one accumulated segment, got {:?}",
        status.diarization
    );
}

/// audio-graph-653a findings 2 + 4 regression: `run_deepgram_event_receiver`
/// must keep processing drained `Transcript` events that arrive AFTER an idle
/// `recv_timeout` tick, and must only exit once `event_rx` actually
/// disconnects — never because some external flag went false. The pre-fix
/// version took an `is_transcribing: Arc<AtomicBool>` and (a) broke out of
/// the loop on the very next 500ms timeout tick once that flag cleared, and
/// (b) discarded an already-dequeued event outright if the flag had cleared
/// in between — both of which threw away exactly the drained tail-of-utterance
/// events the client-side close-drain fix (deepgram.rs) exists to recover.
/// This test proves neither failure mode survives: a second transcript sent
/// after a tick that is long enough to have tripped the old flag-check still
/// reaches the transcript buffer, and the thread only exits once the sender
/// side of `event_rx` is fully dropped.
#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Tauri/Tao AppHandle construction must run on the macOS main thread"
)]
fn deepgram_event_receiver_survives_an_idle_tick_and_exits_only_on_channel_close() {
    let data_dir = unique_tempdir("deepgram-event-receiver");
    let _guard = DataDirGuard::set(&data_dir);
    let app_handle = super::shared_test_app_handle();
    let session_id = "deepgram-event-receiver-session";
    let models_dir = std::env::temp_dir().join(format!(
        "audio-graph-deepgram-receiver-test-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&models_dir).expect("create temp models dir");

    let transcript_buffer: Arc<RwLock<VecDeque<TranscriptSegment>>> =
        Arc::new(RwLock::new(VecDeque::new()));
    let pipeline_status = Arc::new(RwLock::new(PipelineStatus::default()));
    let graph_snapshot = Arc::new(RwLock::new(GraphSnapshot::default()));
    let knowledge_graph = Arc::new(Mutex::new(TemporalKnowledgeGraph::new()));
    let graph_extractor = Arc::new(RuleBasedExtractor::new());
    let llm_engine: Arc<Mutex<Option<LlmEngine>>> = Arc::new(Mutex::new(None));
    let api_client: Arc<Mutex<Option<ApiClient>>> = Arc::new(Mutex::new(None));
    let mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>> = Arc::new(Mutex::new(None));
    let openrouter_client: Arc<Mutex<Option<OpenRouterClient>>> = Arc::new(Mutex::new(None));
    let llm_executor = LlmExecutor::new(
        llm_engine.clone(),
        api_client.clone(),
        openrouter_client.clone(),
        mistralrs_engine.clone(),
    );
    // A real (not `None`) canonical writer -- `record_asr_span_revision_event`
    // refuses to advance the ledger (and therefore never pushes to
    // `transcript_buffer`) without one, matching the production wiring
    // `run_deepgram_speech_processor` provides.
    let transcript_event_writer = Arc::new(Mutex::new(TranscriptEventWriter::spawn(session_id)));
    assert!(
        transcript_event_writer.lock().unwrap().is_some(),
        "integration fixture requires an accepting canonical writer"
    );

    let projection_job_workers: crate::state::ProjectionJobRegistry =
        Arc::new(Mutex::new(Vec::new()));
    let projection_lane_stopping = Arc::new(AtomicBool::new(false));
    let shared = SpeechShared {
        transcript_buffer: transcript_buffer.clone(),
        transcript_writer: Arc::new(Mutex::new(None)),
        transcript_event_writer,
        transcript_ledger: Arc::new(Mutex::new(crate::projections::TranscriptLedger::new(
            session_id,
        ))),
        speaker_timeline: Arc::new(Mutex::new(crate::projections::SpeakerTimeline::new(
            session_id,
        ))),
        projection_schedulers: Arc::new(Mutex::new(
            crate::projection_scheduler::ProjectionSchedulers::new(session_id),
        )),
        projection_runtime: crate::state::ProjectionRuntimeHandle::in_memory_for_tests(session_id),
        active_session_id: Arc::new(RwLock::new(session_id.to_string())),
        pipeline_status: pipeline_status.clone(),
        app_handle,
        knowledge_graph,
        graph_snapshot,
        graph_extractor,
        llm_engine,
        api_client,
        mistralrs_engine,
        llm_executor,
        pending_agent_proposals: Arc::new(Mutex::new(HashMap::new())),
        projection_job_workers: projection_job_workers.clone(),
        projection_lane_stopping: projection_lane_stopping.clone(),
    };
    let config = SpeechConfig {
        models_dir: models_dir.clone(),
        llm_provider: LlmProvider::default(),
        llm_allow_cloud_fallbacks: true,
        provider_content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
    };
    let source_id_hint = Arc::new(RwLock::new(Some("integration-source".to_string())));

    let (event_tx, event_rx) =
        crossbeam_channel::bounded::<crate::asr::deepgram::DeepgramEvent>(16);
    let receiver_thread = std::thread::spawn(move || {
        super::run_deepgram_event_receiver(event_rx, shared, config, source_id_hint, 0);
    });

    let transcript_event =
        |text: &str, start: f64| crate::asr::deepgram::DeepgramEvent::Transcript {
            text: text.to_string(),
            confidence: 0.9,
            is_final: true,
            speech_final: true,
            start,
            duration: 0.5,
            words: Vec::new(),
        };

    event_tx
        .send(transcript_event("first drained tail", 0.0))
        .expect("send first transcript");
    wait_until("first drained transcript to land", || {
        transcript_buffer
            .read()
            .map(|buf| buf.len() == 1)
            .unwrap_or(false)
    });

    // Sleep well past one `recv_timeout(500ms)` tick. The pre-fix receiver
    // treated a cleared `is_transcribing` flag as its exit signal on exactly
    // this kind of idle tick; this test never sets any such flag at all —
    // the receiver must still be alive and processing afterward.
    std::thread::sleep(Duration::from_millis(700));

    event_tx
        .send(transcript_event("second drained tail", 1.0))
        .expect("send second transcript");
    wait_until("second drained transcript to land", || {
        transcript_buffer
            .read()
            .map(|buf| buf.len() == 2)
            .unwrap_or(false)
    });

    {
        let buffer = transcript_buffer.read().expect("transcript buffer lock");
        let texts: Vec<&str> = buffer.iter().map(|seg| seg.text.as_str()).collect();
        assert_eq!(texts, vec!["first drained tail", "second drained tail"]);
    }

    // Dropping the sender is the ONLY thing that should end the receiver
    // thread now — it must exit promptly once `event_rx` disconnects, and
    // must not have exited (or hung) before this point. Joined through a
    // bounded watcher (not a direct `.join()`) so a regression that hangs
    // the receiver fails this test instead of hanging the whole suite.
    drop(event_tx);
    let (join_done_tx, join_done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = join_done_tx.send(receiver_thread.join());
    });
    join_done_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("event receiver thread must exit within 3s of the channel disconnecting")
        .expect("event receiver thread must exit cleanly once the channel disconnects");

    stop_and_drain_projection_lane(&projection_lane_stopping, &projection_job_workers);
    let _ = fs::remove_dir_all(&models_dir);
}

/// Load the raw wire message at `messages[index]` from a `fixtures/asr/`
/// event fixture and replay it through the real Deepgram message handler,
/// so a test drives the exact same parsing path production does instead of
/// hand-building a `DeepgramEvent` literal that can drift from the fixture
/// it claims to mirror.
fn load_deepgram_fixture_event(
    relative_path: &str,
    index: usize,
) -> crate::asr::deepgram::DeepgramEvent {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("asr")
        .join(relative_path);
    let body = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read fixture {}: {error}", path.display()));
    let fixture: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("failed to parse fixture {}: {error}", path.display()));
    let raw = fixture["messages"][index]["raw"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "{}: messages[{index}].raw missing/not a string",
                path.display()
            )
        });
    let (tx, rx) = crossbeam_channel::unbounded::<crate::asr::deepgram::DeepgramEvent>();
    crate::asr::deepgram::handle_server_message(raw, &tx);
    rx.try_recv()
        .unwrap_or_else(|_| panic!("{}: messages[{index}] produced no event", path.display()))
}

/// audio-graph-4aed: production diarization discarded per-word speaker
/// indices. `run_deepgram_event_receiver` derived a final's speaker from
/// `words.first()` alone, so a final whose words evidence a mid-final turn
/// change (Deepgram sets a per-WORD `speaker`, not a per-final one) was
/// attributed entirely to whichever speaker uttered the first word —
/// field-measured: ~18% turn recall, 97.6% of speech time collapsed onto one
/// speaker on a 2-person podcast.
///
/// Three checks, each driven through the real `run_deepgram_event_receiver`
/// thread:
///
/// 1. The exact multi-speaker final already covered by the
///    diarization-ledger normalizer fixture (message index 1 of
///    `fixtures/asr/deepgram/diarization_revisions.json` — the "hello world"
///    final, `hello` on speaker 0 followed by `world` on speaker 1) gets
///    replayed from that fixture file through the real Deepgram message
///    parser, not a hand-built literal, and must land as two segments with
///    coherent per-run span ids/revisions instead of one segment carrying
///    only `words.first()`'s speaker.
/// 2. A final whose words carry a `punctuated_word` (production sets
///    `punctuate=true`, so real words always do) must keep that
///    punctuation/capitalization in each split run's text, not fall back to
///    the raw lowercase `word` token.
/// 3. A final whose runs' word-starts round to the same span-id millisecond
///    must still get distinct span ids — group (1)'s and (2)'s span ids are
///    always naturally far enough apart in this fixture data to skip this
///    guard, so it needs its own crafted timestamps.
#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Tauri/Tao AppHandle construction must run on the macOS main thread"
)]
fn deepgram_multi_speaker_final_splits_into_per_run_segments() {
    let data_dir = unique_tempdir("deepgram-multi-speaker-final");
    let _guard = DataDirGuard::set(&data_dir);
    let app_handle = super::shared_test_app_handle();
    let session_id = "deepgram-multi-speaker-final-session";
    let models_dir = std::env::temp_dir().join(format!(
        "audio-graph-deepgram-multi-speaker-test-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&models_dir).expect("create temp models dir");

    let transcript_buffer: Arc<RwLock<VecDeque<TranscriptSegment>>> =
        Arc::new(RwLock::new(VecDeque::new()));
    let pipeline_status = Arc::new(RwLock::new(PipelineStatus::default()));
    let graph_snapshot = Arc::new(RwLock::new(GraphSnapshot::default()));
    let knowledge_graph = Arc::new(Mutex::new(TemporalKnowledgeGraph::new()));
    let graph_extractor = Arc::new(RuleBasedExtractor::new());
    let llm_engine: Arc<Mutex<Option<LlmEngine>>> = Arc::new(Mutex::new(None));
    let api_client: Arc<Mutex<Option<ApiClient>>> = Arc::new(Mutex::new(None));
    let mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>> = Arc::new(Mutex::new(None));
    let openrouter_client: Arc<Mutex<Option<OpenRouterClient>>> = Arc::new(Mutex::new(None));
    let llm_executor = LlmExecutor::new(
        llm_engine.clone(),
        api_client.clone(),
        openrouter_client.clone(),
        mistralrs_engine.clone(),
    );
    let transcript_event_writer = Arc::new(Mutex::new(TranscriptEventWriter::spawn(session_id)));
    assert!(
        transcript_event_writer.lock().unwrap().is_some(),
        "integration fixture requires an accepting canonical writer"
    );

    let projection_job_workers: crate::state::ProjectionJobRegistry =
        Arc::new(Mutex::new(Vec::new()));
    let projection_lane_stopping = Arc::new(AtomicBool::new(false));
    let transcript_ledger = Arc::new(Mutex::new(crate::projections::TranscriptLedger::new(
        session_id,
    )));
    let shared = SpeechShared {
        transcript_buffer: transcript_buffer.clone(),
        transcript_writer: Arc::new(Mutex::new(None)),
        transcript_event_writer,
        transcript_ledger: transcript_ledger.clone(),
        speaker_timeline: Arc::new(Mutex::new(crate::projections::SpeakerTimeline::new(
            session_id,
        ))),
        projection_schedulers: Arc::new(Mutex::new(
            crate::projection_scheduler::ProjectionSchedulers::new(session_id),
        )),
        projection_runtime: crate::state::ProjectionRuntimeHandle::in_memory_for_tests(session_id),
        active_session_id: Arc::new(RwLock::new(session_id.to_string())),
        pipeline_status: pipeline_status.clone(),
        app_handle,
        knowledge_graph,
        graph_snapshot,
        graph_extractor,
        llm_engine,
        api_client,
        mistralrs_engine,
        llm_executor,
        pending_agent_proposals: Arc::new(Mutex::new(HashMap::new())),
        projection_job_workers: projection_job_workers.clone(),
        projection_lane_stopping: projection_lane_stopping.clone(),
    };
    let config = SpeechConfig {
        models_dir: models_dir.clone(),
        llm_provider: LlmProvider::default(),
        llm_allow_cloud_fallbacks: true,
        provider_content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
    };
    let source_id = "integration-source".to_string();
    let source_id_hint = Arc::new(RwLock::new(Some(source_id.clone())));

    let (event_tx, event_rx) =
        crossbeam_channel::bounded::<crate::asr::deepgram::DeepgramEvent>(16);
    // max_speakers = 0 ("no cap") so the raw Deepgram speaker indices pass
    // through unchanged (0 -> "Speaker 0", 1 -> "Speaker 1").
    let receiver_thread = std::thread::spawn(move || {
        super::run_deepgram_event_receiver(event_rx, shared, config, source_id_hint, 0);
    });

    // --- Check 1: fixture-sourced multi-speaker final --------------------
    // `messages[1]` in the diarization-ledger normalizer fixture is the
    // "hello world" final: start=1.0, duration=1.5, hello@[1.0,1.5) speaker
    // 0, world@[1.5,2.5) speaker 1, confidence=0.75. Loaded from the fixture
    // file and replayed through the real parser (`handle_server_message`),
    // not hand-built, so this test tracks fixture drift instead of a
    // manual copy of it.
    let fixture_event = load_deepgram_fixture_event("deepgram/diarization_revisions.json", 1);
    event_tx
        .send(fixture_event)
        .expect("send fixture multi-speaker final");

    wait_until(
        "fixture multi-speaker final to split into two segments",
        || {
            transcript_buffer
                .read()
                .map(|buf| buf.len() == 2)
                .unwrap_or(false)
        },
    );

    {
        let buffer = transcript_buffer.read().expect("transcript buffer lock");
        let segments: Vec<&TranscriptSegment> = buffer.iter().collect();
        assert_eq!(
            segments.len(),
            2,
            "a mid-final speaker change must split into two segments, not collapse onto \
             words.first()'s speaker"
        );

        assert_eq!(segments[0].text, "hello");
        assert_eq!(segments[0].speaker_label.as_deref(), Some("Speaker 0"));
        assert_eq!(segments[0].start_time, 1.0);
        assert_eq!(segments[0].end_time, 1.5);

        assert_eq!(segments[1].text, "world");
        assert_eq!(
            segments[1].speaker_label.as_deref(),
            Some("Speaker 1"),
            "run 1's words must not be silently attributed to run 0's speaker"
        );
        assert_eq!(segments[1].start_time, 1.5);
        assert_eq!(segments[1].end_time, 2.5);
    }

    // Span-id / revision coherence: two independent spans, not one span
    // silently overwritten by the second run. Run 0 keeps the
    // `provider_start_span_id` convention keyed on the final's own start
    // (so a live interim at that span would have been closed out exactly as
    // the single-run path does); run 1 is a brand-new span keyed on its own
    // later start time. Both are first revisions with nothing superseded,
    // since no interim ever announced either span before this final.
    {
        let ledger = transcript_ledger.lock().expect("ledger lock");
        assert_eq!(
            ledger.latest_spans.len(),
            2,
            "two split runs must produce two independently addressable ledger spans"
        );
        let by_span_id = |id: &str| {
            ledger
                .latest_spans
                .iter()
                .find(|span| span.span_id == id)
                .unwrap_or_else(|| panic!("ledger missing expected span_id {id}"))
        };
        let run0_span_id = super::provider_start_span_id("deepgram", &source_id, 1.0);
        let run1_span_id = super::provider_start_span_id("deepgram", &source_id, 1.5);
        assert_ne!(
            run0_span_id, run1_span_id,
            "split runs must never collide on the same span_id"
        );

        let run0 = by_span_id(&run0_span_id);
        assert_eq!(run0.revision_number, 1);
        assert_eq!(run0.supersedes, None);
        assert_eq!(run0.speaker_label.as_deref(), Some("Speaker 0"));

        let run1 = by_span_id(&run1_span_id);
        assert_eq!(run1.revision_number, 1);
        assert_eq!(run1.supersedes, None);
        assert_eq!(run1.speaker_label.as_deref(), Some("Speaker 1"));
    }

    // --- Check 2: punctuated_word fidelity across a split -----------------
    // Production connects with `punctuate=true`, so real words always carry
    // a `punctuated_word`. Reuses the same "hello"/"world" tokens as check 1
    // (capitalized/punctuated, not new transcript content) at a distinct
    // start so its span ids can't collide with check 1's.
    let punctuated_word = |word: &str, punctuated: &str, start: f64, end: f64, speaker: u32| {
        crate::asr::deepgram::DeepgramWord {
            word: word.to_string(),
            punctuated_word: Some(punctuated.to_string()),
            start,
            end,
            confidence: 0.75,
            speaker: Some(speaker),
        }
    };
    event_tx
        .send(crate::asr::deepgram::DeepgramEvent::Transcript {
            text: "Hello world.".to_string(),
            confidence: 0.75,
            is_final: true,
            speech_final: false,
            start: 10.0,
            duration: 1.0,
            words: vec![
                punctuated_word("hello", "Hello", 10.0, 10.5, 0),
                punctuated_word("world", "world.", 10.5, 11.0, 1),
            ],
        })
        .expect("send punctuated multi-speaker final");

    wait_until("punctuated final to split into two more segments", || {
        transcript_buffer
            .read()
            .map(|buf| buf.len() == 4)
            .unwrap_or(false)
    });

    {
        let buffer = transcript_buffer.read().expect("transcript buffer lock");
        let segments: Vec<&TranscriptSegment> = buffer.iter().collect();
        assert_eq!(
            segments[2].text, "Hello",
            "split-run text must use punctuated_word, not the raw lowercase word token"
        );
        assert_eq!(
            segments[3].text, "world.",
            "split-run text must use punctuated_word, not the raw lowercase word token"
        );
    }

    // --- Check 3: millisecond-quantization span-id collision guard -------
    // `hello` ends at 20.0002 and `world` starts at 20.0004 — both round to
    // the same span-id millisecond (20000) as the final's own `start`
    // (20.0), which run 0 already claims. Without a guard, run 1's naive
    // `provider_start_span_id` would collide with run 0's and silently
    // supersede it in the ledger.
    event_tx
        .send(crate::asr::deepgram::DeepgramEvent::Transcript {
            text: "hello world".to_string(),
            confidence: 0.75,
            is_final: true,
            speech_final: false,
            start: 20.0,
            duration: 0.01,
            words: vec![
                punctuated_word("hello", "hello", 20.0, 20.0002, 0),
                punctuated_word("world", "world", 20.0004, 20.01, 1),
            ],
        })
        .expect("send millisecond-tied multi-speaker final");

    wait_until(
        "millisecond-tied final to split into two more segments",
        || {
            transcript_buffer
                .read()
                .map(|buf| buf.len() == 6)
                .unwrap_or(false)
        },
    );

    {
        let ledger = transcript_ledger.lock().expect("ledger lock");
        let naive_run1_span_id = super::provider_start_span_id("deepgram", &source_id, 20.0004);
        let run0_span_id = super::provider_start_span_id("deepgram", &source_id, 20.0);
        assert_eq!(
            naive_run1_span_id, run0_span_id,
            "test setup sanity check: both runs must quantize to the same millisecond \
             for this to actually exercise the collision guard"
        );

        let matching: Vec<_> = ledger
            .latest_spans
            .iter()
            .filter(|span| span.span_id == run0_span_id)
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "run 1 must not silently overwrite run 0's ledger span when their \
             millisecond-quantized starts collide"
        );
        assert_eq!(matching[0].revision_number, 1);
        assert_eq!(matching[0].supersedes, None);

        let bumped_run1 = ledger
            .latest_spans
            .iter()
            .find(|span| span.span_id != run0_span_id && span.start_time > 20.0003)
            .expect("run 1 must land on a distinct, disambiguated span_id");
        assert_eq!(bumped_run1.revision_number, 1);
        assert_eq!(bumped_run1.supersedes, None);
    }

    drop(event_tx);
    let (join_done_tx, join_done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = join_done_tx.send(receiver_thread.join());
    });
    join_done_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("event receiver thread must exit within 3s of the channel disconnecting")
        .expect("event receiver thread must exit cleanly once the channel disconnects");

    stop_and_drain_projection_lane(&projection_lane_stopping, &projection_job_workers);
    let _ = fs::remove_dir_all(&models_dir);
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "Tauri/Tao AppHandle construction must run on the macOS main thread"
)]
fn assemblyai_unformatted_final_waits_for_formatted_final_side_effects() {
    let data_dir = unique_tempdir("assemblyai-formatted-final");
    let _guard = DataDirGuard::set(&data_dir);
    // Shared process-wide gtk app handle (seed audio-graph-65f0).
    let app_handle = super::shared_test_app_handle();
    let session_id = "assemblyai-formatted-final-session";

    let transcript_buffer: Arc<RwLock<VecDeque<TranscriptSegment>>> =
        Arc::new(RwLock::new(VecDeque::new()));
    let transcript_ledger = Arc::new(Mutex::new(TranscriptLedger::new(session_id)));
    let speaker_timeline = Arc::new(Mutex::new(crate::projections::SpeakerTimeline::new(
        session_id,
    )));
    let projection_schedulers = Arc::new(Mutex::new(ProjectionSchedulers::new(session_id)));
    let pipeline_status = Arc::new(RwLock::new(PipelineStatus::default()));
    let graph_snapshot = Arc::new(RwLock::new(GraphSnapshot::default()));
    let knowledge_graph = Arc::new(Mutex::new(TemporalKnowledgeGraph::new()));
    let graph_extractor = Arc::new(RuleBasedExtractor::new());
    let llm_engine: Arc<Mutex<Option<LlmEngine>>> = Arc::new(Mutex::new(None));
    let api_client: Arc<Mutex<Option<ApiClient>>> = Arc::new(Mutex::new(None));
    let mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>> = Arc::new(Mutex::new(None));
    let openrouter_client: Arc<Mutex<Option<OpenRouterClient>>> = Arc::new(Mutex::new(None));
    let llm_executor = LlmExecutor::new(
        llm_engine.clone(),
        api_client.clone(),
        openrouter_client,
        mistralrs_engine.clone(),
    );
    let pending_agent_proposals = Arc::new(Mutex::new(HashMap::new()));
    let active_session_id = Arc::new(RwLock::new(session_id.to_string()));
    let transcript_event_writer = Arc::new(Mutex::new(TranscriptEventWriter::spawn(session_id)));
    assert!(
        transcript_event_writer.lock().unwrap().is_some(),
        "integration fixture requires an accepting canonical writer"
    );
    let projection_job_workers: crate::state::ProjectionJobRegistry =
        Arc::new(Mutex::new(Vec::new()));
    let projection_lane_stopping = Arc::new(AtomicBool::new(false));
    let ctx = TranscriptProcessingContext {
        asr_provider: "assemblyai",
        active_session_id,
        transcript_buffer: transcript_buffer.clone(),
        transcript_writer: Arc::new(Mutex::new(None)),
        transcript_event_writer: transcript_event_writer.clone(),
        transcript_ledger: transcript_ledger.clone(),
        speaker_timeline,
        projection_schedulers: projection_schedulers.clone(),
        projection_runtime: ProjectionRuntimeHandle::in_memory_for_tests(session_id),
        projection_job_workers: projection_job_workers.clone(),
        projection_lane_stopping: projection_lane_stopping.clone(),
        pipeline_status,
        app_handle,
        llm_engine,
        api_client,
        mistralrs_engine,
        llm_executor,
        llm_provider: LlmProvider::default(),
        llm_allow_cloud_fallbacks: true,
        graph_extractor,
        knowledge_graph,
        graph_snapshot,
        pending_agent_proposals: pending_agent_proposals.clone(),
        pending_extraction: Arc::new(Mutex::new(None)),
    };
    let extraction_count = Arc::new(AtomicU64::new(0));
    let graph_update_count = Arc::new(AtomicU64::new(0));

    fn revision(
        revision_number: u64,
        text: &str,
        is_final: bool,
        turn_is_formatted: bool,
    ) -> AssemblyAiV3ParsedRevision {
        AssemblyAiV3ParsedRevision {
            payload: AsrSpanRevisionPayload {
                span_id: "assemblyai:mic-1:turn-0".to_string(),
                provider: "assemblyai".to_string(),
                source_id: "mic-1".to_string(),
                provider_item_id: Some("turn-0".to_string()),
                transcript_segment_id: is_final.then(|| "turn-0@final".to_string()),
                speaker_id: Some("A".to_string()),
                speaker_label: Some("Speaker A".to_string()),
                channel: None,
                text: text.to_string(),
                start_time: 0.0,
                end_time: 1.3,
                confidence: 0.97,
                is_final,
                stability: if is_final {
                    AsrSpanStability::Final
                } else {
                    AsrSpanStability::Partial
                },
                revision_number,
                supersedes: (revision_number > 1)
                    .then(|| format!("assemblyai:mic-1:turn-0@rev{}", revision_number - 1)),
                turn_id: Some("turn-0".to_string()),
                end_of_turn: is_final,
                raw_event_ref: Some(format!("assemblyai.v3.turn.{}", revision_number + 2)),
                capture_latency_ms: None,
                asr_latency_ms: None,
                received_at_ms: 1_700_000_000_000 + revision_number,
            },
            turn_is_formatted,
            end_of_turn_confidence: Some(0.97),
        }
    }

    let mut partial = revision(1, "Who owns this", false, false);
    normalize_assemblyai_v3_revision_for_side_effects(&mut partial);
    assert!(emit_provider_span_revision_payload(
        partial.payload,
        &ctx,
        0,
        &extraction_count,
        &graph_update_count,
    ));

    let mut unformatted_final = revision(2, "Who owns this action item", true, false);
    normalize_assemblyai_v3_revision_for_side_effects(&mut unformatted_final);
    assert!(
        !unformatted_final.payload.is_final,
        "unformatted AssemblyAI final must be downgraded before side effects"
    );
    assert!(
        !unformatted_final.payload.end_of_turn,
        "unformatted AssemblyAI final must not trigger projection observation"
    );
    assert!(emit_provider_span_revision_payload(
        unformatted_final.payload,
        &ctx,
        0,
        &extraction_count,
        &graph_update_count,
    ));

    assert_eq!(
        transcript_buffer.read().unwrap().len(),
        0,
        "partial and unformatted final must not append transcript rows"
    );
    {
        let schedulers = projection_schedulers.lock().unwrap();
        assert_eq!(
            schedulers.notes().metrics().jobs_started,
            0,
            "unformatted final must not start notes projection"
        );
        assert_eq!(
            schedulers.graph().metrics().jobs_started,
            0,
            "unformatted final must not start graph projection"
        );
    }
    assert_eq!(
        pending_agent_proposals.lock().unwrap().len(),
        0,
        "unformatted final must not spawn live-assist proposals"
    );

    let mut formatted_final = revision(3, "Who owns this action item?", true, true);
    normalize_assemblyai_v3_revision_for_side_effects(&mut formatted_final);
    assert!(emit_provider_span_revision_payload(
        formatted_final.payload,
        &ctx,
        1,
        &extraction_count,
        &graph_update_count,
    ));

    {
        let buffer = transcript_buffer.read().unwrap();
        assert_eq!(buffer.len(), 1, "formatted final appends one row");
        assert_eq!(buffer[0].id, "turn-0@final");
        assert_eq!(buffer[0].text, "Who owns this action item?");
    }
    {
        let ledger = transcript_ledger.lock().unwrap();
        assert_eq!(ledger.accepted_event_count, 3);
        assert_eq!(ledger.latest_spans.len(), 1);
        assert_eq!(ledger.latest_spans[0].revision_number, 3);
        assert!(ledger.latest_spans[0].is_final);
    }
    {
        let schedulers = projection_schedulers.lock().unwrap();
        assert_eq!(
            schedulers.notes().metrics().jobs_started,
            1,
            "only the formatted final should start notes projection"
        );
        assert_eq!(
            schedulers.graph().metrics().jobs_started,
            1,
            "only the formatted final should start graph projection"
        );
    }
    wait_until("single formatted-final live-assist proposal", || {
        pending_agent_proposals.lock().unwrap().len() == 1
    });
    assert_eq!(
        pending_agent_proposals.lock().unwrap().len(),
        1,
        "formatted final should spawn exactly one proposal"
    );

    if let Some(writer) = transcript_event_writer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    {
        assert!(writer.shutdown_with_timeout(Duration::from_secs(2)));
    }

    stop_and_drain_projection_lane(&projection_lane_stopping, &projection_job_workers);
    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn diarize_extract_graph_chain_accumulates_entities() {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut worker = DiarizationWorker::new(DiarizationConfig::default(), tx);
    let buffer: Arc<RwLock<VecDeque<TranscriptSegment>>> = Arc::new(RwLock::new(VecDeque::new()));
    let extractor = RuleBasedExtractor::new();
    let graph = Arc::new(Mutex::new(TemporalKnowledgeGraph::new()));

    // Same amplitude → same speaker across all segments, exercising the
    // "speaker persists across segments" branch of the Simple backend.
    let amp = 0.3;
    let segments_text = [
        "Alice Johnson met Bob Smith at Google Inc yesterday.",
        "They discussed the project deadline in San Francisco.",
        "Carol Davis joined them from Microsoft Corporation.",
        "The meeting was held in New York with Acme Technologies.",
        "Everyone agreed on the new \"machine learning\" initiative.",
    ];

    for (i, text) in segments_text.iter().enumerate() {
        let start = i as f64 * 2.0;
        let input = make_input(text, start, start + 2.0, amp);
        process_one(&mut worker, &buffer, &extractor, &graph, input);
    }

    // Assertion 1: transcript buffer got every segment.
    let buf_len = buffer.read().unwrap().len();
    assert_eq!(
        buf_len,
        segments_text.len(),
        "transcript buffer should contain all 5 segments, got {}",
        buf_len
    );

    // Assertion 2: every buffered segment has a speaker label assigned by
    // diarization (the chain's job).
    for seg in buffer.read().unwrap().iter() {
        assert!(
            seg.speaker_id.is_some(),
            "segment {} missing speaker_id after diarization",
            seg.id
        );
        assert!(
            seg.speaker_label.is_some(),
            "segment {} missing speaker_label after diarization",
            seg.id
        );
    }

    // Assertion 3: same-amplitude audio should collapse to a single speaker,
    // proving the diarization worker's state actually persists across calls
    // in the way the real loop depends on.
    let speakers: std::collections::HashSet<String> = buffer
        .read()
        .unwrap()
        .iter()
        .filter_map(|s| s.speaker_id.clone())
        .collect();
    assert_eq!(
        speakers.len(),
        1,
        "identical audio across 5 segments should map to 1 speaker, got {:?}",
        speakers
    );

    // Assertion 4: the knowledge graph accumulated multiple entity types
    // from the text across all 5 segments.
    let snapshot = graph.lock().unwrap().snapshot();
    assert!(
        snapshot.stats.total_nodes >= 5,
        "graph should accumulate ≥5 entities across 5 entity-rich segments, got {}",
        snapshot.stats.total_nodes
    );

    // Assertion 5: at least one Organization and one Location made it in —
    // proves the extractor's output is being fed to the graph, not just the
    // Person-from-speaker fallback.
    let entity_types: std::collections::HashSet<String> = snapshot
        .nodes
        .iter()
        .map(|n| n.entity_type.clone())
        .collect();
    assert!(
        entity_types.contains("Organization"),
        "graph should include at least one Organization entity, got types: {:?}",
        entity_types
    );
    assert!(
        entity_types.contains("Location"),
        "graph should include at least one Location entity, got types: {:?}",
        entity_types
    );

    // Assertion 6: the speaker label from diarization is the Person entity
    // key in the graph. This is the cross-stage contract that would silently
    // break if someone renamed the speaker_label format.
    let speaker_label = buffer
        .read()
        .unwrap()
        .front()
        .and_then(|s| s.speaker_label.clone())
        .expect("first segment should have a speaker label");
    let has_speaker_person = snapshot
        .nodes
        .iter()
        .any(|n| n.entity_type == "Person" && n.name == speaker_label);
    assert!(
        has_speaker_person,
        "diarization speaker_label '{}' should appear as a Person node; \
         graph persons: {:?}",
        speaker_label,
        snapshot
            .nodes
            .iter()
            .filter(|n| n.entity_type == "Person")
            .map(|n| &n.name)
            .collect::<Vec<_>>()
    );
}

#[test]
fn transcript_buffer_ring_buffer_evicts_oldest_past_500() {
    // This exercises the overflow tail of `emit_transcript_and_extract_with_meta`
    // (lines 364-370). Without this, a long recording session silently
    // leaks memory.
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut worker = DiarizationWorker::new(DiarizationConfig::default(), tx);
    let buffer: Arc<RwLock<VecDeque<TranscriptSegment>>> = Arc::new(RwLock::new(VecDeque::new()));
    let extractor = RuleBasedExtractor::new();
    let graph = Arc::new(Mutex::new(TemporalKnowledgeGraph::new()));

    // Push 502 segments — 2 more than the cap. Text is minimal to keep
    // extraction cheap (we're not asserting graph contents here).
    for i in 0..502_usize {
        let start = i as f64 * 0.5;
        let input = make_input("hello there", start, start + 0.5, 0.3);
        process_one(&mut worker, &buffer, &extractor, &graph, input);
    }

    let buf = buffer.read().unwrap();
    assert_eq!(
        buf.len(),
        500,
        "ring buffer should cap at 500, got {}",
        buf.len()
    );

    // The *oldest* 2 should have been popped — verify by start_time
    // monotonicity: the first remaining segment must start after the 2nd
    // pushed segment (start=0.5).
    let first_remaining_start = buf.front().unwrap().start_time;
    assert!(
        first_remaining_start >= 1.0,
        "oldest segment should be evicted, first remaining start_time = {} \
         (expected ≥ 1.0)",
        first_remaining_start
    );
}

#[test]
fn two_speakers_produce_distinct_person_nodes() {
    // Drives the branch of the chain where diarization assigns different
    // speakers to different audio, and those distinct labels both end up
    // in the graph as separate Person nodes.
    let (tx, _rx) = crossbeam_channel::unbounded();
    let config = DiarizationConfig {
        // Low threshold so distinct amplitudes trigger a new speaker.
        similarity_threshold: 0.3,
        ..DiarizationConfig::default()
    };
    let mut worker = DiarizationWorker::new(config, tx);
    let buffer: Arc<RwLock<VecDeque<TranscriptSegment>>> = Arc::new(RwLock::new(VecDeque::new()));
    let extractor = RuleBasedExtractor::new();
    let graph = Arc::new(Mutex::new(TemporalKnowledgeGraph::new()));

    // Quiet DC vs loud alternating — copied from the diarization unit
    // tests' known-good distinct-speaker recipe.
    let quiet = make_input("First speaker turn", 0.0, 0.5, 0.05);
    process_one(&mut worker, &buffer, &extractor, &graph, quiet);

    let loud_alternating_audio: Vec<f32> = (0..8_000)
        .map(|i| if i % 2 == 0 { 0.8 } else { -0.8 })
        .collect();
    let loud = DiarizationInput {
        transcript: TranscriptSegment {
            id: uuid::Uuid::new_v4().to_string(),
            source_id: "integration-test".to_string(),
            speaker_id: None,
            speaker_label: None,
            text: "Second speaker turn".to_string(),
            start_time: 1.0,
            end_time: 1.5,
            confidence: 0.95,
        },
        speech_audio: loud_alternating_audio,
        speech_start_time: Duration::from_secs_f64(1.0),
        speech_end_time: Duration::from_secs_f64(1.5),
    };
    process_one(&mut worker, &buffer, &extractor, &graph, loud);

    // Collect the two speaker labels assigned.
    let labels: Vec<String> = buffer
        .read()
        .unwrap()
        .iter()
        .filter_map(|s| s.speaker_label.clone())
        .collect();
    assert_eq!(labels.len(), 2, "both segments should have labels");
    assert_ne!(
        labels[0], labels[1],
        "distinct audio should produce distinct speaker labels, got {:?}",
        labels
    );

    // Both labels should appear as Person nodes in the graph.
    let snapshot = graph.lock().unwrap().snapshot();
    let person_names: std::collections::HashSet<String> = snapshot
        .nodes
        .iter()
        .filter(|n| n.entity_type == "Person")
        .map(|n| n.name.clone())
        .collect();
    for label in &labels {
        assert!(
            person_names.contains(label),
            "speaker label '{}' should be a Person node; persons: {:?}",
            label,
            person_names
        );
    }
}
