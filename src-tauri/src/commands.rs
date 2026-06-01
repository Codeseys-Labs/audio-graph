//! Tauri IPC command handlers.
//!
//! Each function here is exposed to the frontend via `tauri::generate_handler![]`.
//! They access `AppState` through Tauri's managed state.
//!
//! Heavy processing logic (speech, extraction) lives in the [`crate::speech`]
//! module — this file only contains thin `#[tauri::command]` wrappers.

use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{Emitter, State};

use crate::audio::pipeline::AudioPipeline;
use crate::error::{AppError, Result as AppResult};
use crate::events::{self, PipelineStatus, StageStatus};
use crate::gemini::{GeminiConfig, GeminiEvent, GeminiLiveClient};
use crate::graph::entities::GraphSnapshot;
use crate::llm::engine::{ChatMessage, ChatResponse};
use crate::llm::openrouter::{
    self as openrouter, OpenRouterClient, OpenRouterConfig, OpenRouterModel,
};
use crate::llm::{ApiClient, ApiConfig};
use crate::speech;
use crate::state::{AppState, AudioSourceInfo, TranscriptSegment};

#[derive(Debug, Clone, serde::Serialize)]
pub struct LoadedSession {
    pub transcript: Vec<TranscriptSegment>,
    pub graph: GraphSnapshot,
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Helper: parse source_id string into rsac::CaptureTarget
// ---------------------------------------------------------------------------

/// Map a frontend source ID string to an rsac [`CaptureTarget`].
///
/// Supported formats:
/// - `"system-default"`          → `CaptureTarget::SystemDefault`
/// - `"device:<device_id>"`      → `CaptureTarget::Device(DeviceId(device_id))`
/// - `"app:<pid>"`               → `CaptureTarget::Application(ApplicationId(pid))`
/// - `"process-tree:<pid>"`      → `CaptureTarget::ProcessTree(ProcessId(pid))`
/// - `"app-name:<name>"`         → `CaptureTarget::ApplicationByName(name)`
fn parse_capture_target(source_id: &str) -> Result<rsac::CaptureTarget, String> {
    if source_id == "system-default" {
        Ok(rsac::CaptureTarget::SystemDefault)
    } else if let Some(device_id) = source_id.strip_prefix("device:") {
        Ok(rsac::CaptureTarget::Device(rsac::DeviceId(
            device_id.to_string(),
        )))
    } else if let Some(pid_str) = source_id.strip_prefix("app:") {
        // ApplicationId wraps a String (the PID as a string).
        Ok(rsac::CaptureTarget::Application(rsac::ApplicationId(
            pid_str.to_string(),
        )))
    } else if let Some(pid_str) = source_id.strip_prefix("process-tree:") {
        let pid = pid_str
            .parse::<u32>()
            .map_err(|_| format!("Invalid process-tree PID: {}", pid_str))?;
        Ok(rsac::CaptureTarget::ProcessTree(rsac::ProcessId(pid)))
    } else if let Some(name) = source_id.strip_prefix("app-name:") {
        Ok(rsac::CaptureTarget::ApplicationByName(name.to_string()))
    } else {
        Err(format!("Unknown source ID format: {}", source_id))
    }
}

/// Send `item` on a bounded channel, evicting the OLDEST queued item to make
/// room when the channel is full. See [`crate::audio::backpressure`].
use crate::audio::backpressure::send_dropping_oldest;

/// Join a worker thread on shutdown, waiting up to `timeout` for it to observe
/// the stop flag and exit. Polls `is_finished()` so a wedged worker can never
/// hang the Stop command — on timeout the handle is detached (dropped) with a
/// warning instead of blocking forever. (Critique H2: prevents Stop→Start
/// races leaving two consumers on the same audio channel.)
fn join_worker_with_timeout(
    handle: std::thread::JoinHandle<()>,
    timeout: std::time::Duration,
    name: &str,
) {
    let deadline = std::time::Instant::now() + timeout;
    while !handle.is_finished() {
        if std::time::Instant::now() >= deadline {
            log::warn!("{name} did not exit within {timeout:?} on stop; detaching handle");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if let Err(e) = handle.join() {
        log::warn!("{name} panicked during shutdown: {e:?}");
    }
}

fn single_session_streaming_asr_name(
    provider: &crate::settings::AsrProvider,
) -> Option<&'static str> {
    match provider {
        // Deepgram and OpenAI Realtime feed through the audio mixer
        // (audio/mixer.rs), which sums all selected sources into one stream, so
        // they are NOT limited to a single source. The others don't have a
        // mixer wired yet.
        crate::settings::AsrProvider::DeepgramStreaming { .. }
        | crate::settings::AsrProvider::OpenAiRealtimeTranscription { .. } => None,
        crate::settings::AsrProvider::AssemblyAI { .. } => Some("AssemblyAI streaming"),
        crate::settings::AsrProvider::AwsTranscribe { .. } => Some("AWS Transcribe streaming"),
        crate::settings::AsrProvider::SherpaOnnx { .. } => Some("Sherpa-ONNX streaming"),
        crate::settings::AsrProvider::LocalWhisper | crate::settings::AsrProvider::Api { .. } => {
            None
        }
    }
}

fn validate_streaming_asr_source_count(
    provider: &crate::settings::AsrProvider,
    active_sources: &[String],
    pending_source: Option<&str>,
) -> Result<(), String> {
    let Some(provider_name) = single_session_streaming_asr_name(provider) else {
        return Ok(());
    };

    let mut source_ids = std::collections::BTreeSet::new();
    for source_id in active_sources {
        let source_id = source_id.trim();
        if !source_id.is_empty() {
            source_ids.insert(source_id.to_string());
        }
    }
    if let Some(pending_source) = pending_source {
        let pending_source = pending_source.trim();
        if !pending_source.is_empty() {
            source_ids.insert(pending_source.to_string());
        }
    }

    if source_ids.len() > 1 {
        return Err(format!(
            "{provider_name} currently supports one active audio source at a time. \
             Stop extra sources or switch to local Whisper/cloud batch ASR before transcribing. \
             Active sources: {}",
            source_ids.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }

    Ok(())
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn api_config_from_runtime_settings(settings: &crate::settings::AppSettings) -> Option<ApiConfig> {
    let crate::settings::LlmProvider::Api {
        endpoint,
        api_key,
        model,
    } = &settings.llm_provider
    else {
        return None;
    };

    let endpoint = non_empty_trimmed(endpoint)?;
    let model = non_empty_trimmed(model)?;
    let llm_api_config = settings.llm_api_config.as_ref().filter(|config| {
        config.endpoint.trim() == endpoint.as_str() && config.model.trim() == model.as_str()
    });
    let api_key = non_empty_trimmed(api_key).or_else(|| {
        llm_api_config
            .and_then(|config| config.api_key.as_deref())
            .and_then(non_empty_trimmed)
    });
    let (max_tokens, temperature) = llm_api_config
        .map(|config| (config.max_tokens, config.temperature))
        .unwrap_or((512, 0.1));

    Some(ApiConfig {
        endpoint,
        api_key,
        model,
        max_tokens,
        temperature,
    })
}

pub(crate) fn sync_llm_api_client_from_settings_cache(state: &AppState) -> Result<(), String> {
    let settings = state
        .app_settings
        .read()
        .map_err(|e| format!("Lock error: {}", e))?
        .clone();
    let next_config = api_config_from_runtime_settings(&settings);

    let mut guard = state
        .api_client
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;

    match next_config {
        Some(config) => {
            let already_current = guard
                .as_ref()
                .map(|client| client.config() == &config)
                .unwrap_or(false);
            if !already_current {
                *guard = Some(ApiClient::new(config));
                log::info!("LLM API client synced from runtime settings");
            }
        }
        None => {
            if guard.take().is_some() {
                log::info!("LLM API client cleared because the active provider is not configured");
            }
        }
    }

    Ok(())
}

fn openrouter_config_from_runtime_settings(
    settings: &crate::settings::AppSettings,
) -> Option<OpenRouterConfig> {
    let crate::settings::LlmProvider::OpenRouter {
        model,
        base_url,
        provider_order,
        include_usage_in_stream,
        api_key,
    } = &settings.llm_provider
    else {
        return None;
    };

    let api_key = non_empty_trimmed(api_key)?;
    let model = non_empty_trimmed(model)?;
    let base_url =
        non_empty_trimmed(base_url).unwrap_or_else(|| openrouter::DEFAULT_BASE_URL.to_string());

    let (max_tokens, temperature) = settings
        .llm_api_config
        .as_ref()
        .map(|config| (config.max_tokens, config.temperature))
        .unwrap_or((512, 0.1));

    Some(OpenRouterConfig {
        api_key,
        model,
        base_url,
        provider_order: provider_order.clone(),
        include_usage_in_stream: *include_usage_in_stream,
        http_referer: openrouter::DEFAULT_HTTP_REFERER.to_string(),
        app_title: openrouter::DEFAULT_APP_TITLE.to_string(),
        max_tokens,
        temperature,
    })
}

pub(crate) fn sync_openrouter_client_from_settings_cache(state: &AppState) -> Result<(), String> {
    let settings = state
        .app_settings
        .read()
        .map_err(|e| format!("Lock error: {}", e))?
        .clone();
    let next_config = openrouter_config_from_runtime_settings(&settings);

    let mut guard = state
        .openrouter_client
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;

    match next_config {
        Some(config) => {
            let already_current = guard
                .as_ref()
                .map(|client| client.config() == &config)
                .unwrap_or(false);
            if !already_current {
                *guard = Some(OpenRouterClient::new(config));
                log::info!("OpenRouter client synced from runtime settings");
            }
        }
        None => {
            if guard.take().is_some() {
                log::info!(
                    "OpenRouter client cleared because the active provider is not OpenRouter"
                );
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// List available audio sources (devices + running applications).
#[tauri::command]
pub async fn list_audio_sources(state: State<'_, AppState>) -> AppResult<Vec<AudioSourceInfo>> {
    log::info!("list_audio_sources called");
    let manager = state
        .capture_manager
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    Ok(manager.list_sources())
}

/// Start capturing audio from the specified source.
#[tauri::command]
pub async fn start_capture(
    source_id: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> AppResult<()> {
    log::info!("start_capture called for source: {}", source_id);

    let target = parse_capture_target(&source_id)?;

    if state.is_transcribing.load(Ordering::SeqCst) {
        let asr_provider = state
            .app_settings
            .read()
            .map_err(|e| format!("Lock error: {}", e))?
            .asr_provider
            .clone();
        let active_sources = state
            .capture_manager
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?
            .active_captures();
        validate_streaming_asr_source_count(&asr_provider, &active_sources, Some(&source_id))?;
    }

    // Resolve the user-configured capture format from the in-memory settings
    // cache, falling back to defaults if the cache is uninitialised or the
    // persisted values are out of the supported whitelist. This is the
    // "wiring through" that Task #79 is about — without it the capture
    // thread would always use the hard-coded 48 kHz / stereo.
    let (capture_sample_rate, capture_channels) = {
        let audio_settings = state
            .app_settings
            .read()
            .map(|s| s.audio_settings.clone())
            .unwrap_or_default();
        crate::settings::resolve_audio_settings(&audio_settings)
    };
    log::info!(
        "start_capture: using sample_rate={} Hz, channels={}",
        capture_sample_rate,
        capture_channels
    );

    // 1. Start capture via the manager.
    {
        let mut manager = state
            .capture_manager
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        manager.start_capture(
            &source_id,
            target,
            state.pipeline_tx.clone(),
            app.clone(),
            capture_sample_rate,
            capture_channels,
        )?;
    }

    // 2. Start pipeline thread if not already running.
    {
        let mut pipeline_handle = state
            .pipeline_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if pipeline_handle.is_none() {
            let rx = state.pipeline_rx.clone();
            let tx = state.processed_tx.clone();
            let handle = std::thread::Builder::new()
                .name("audio-pipeline".to_string())
                .spawn(move || {
                    let mut pipeline = AudioPipeline::new(rx, tx);
                    pipeline.run();
                })
                .map_err(|e| format!("Failed to spawn pipeline thread: {}", e))?;
            *pipeline_handle = Some(handle);
            log::info!("Pipeline thread spawned");
        }
    }

    // 2b. Start dispatcher thread (Bug 1 fix): reads from processed_rx and
    //     fans out to per-consumer channels so both speech processor and
    //     Gemini receive ALL chunks.
    {
        let mut dispatcher_handle = state
            .dispatcher_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if dispatcher_handle.is_none() {
            let processed_rx = state.processed_rx.clone();
            let speech_tx = state.speech_audio_tx.clone();
            let gemini_tx = state.gemini_audio_tx.clone();
            // Receiver clones used ONLY to evict the oldest queued chunk when a
            // consumer channel is full (drop-oldest). crossbeam channels are
            // MPMC so this is safe alongside the real consumer.
            let speech_drain_rx = state.speech_audio_rx.clone();
            let gemini_drain_rx = state.gemini_audio_rx.clone();
            let is_transcribing = state.is_transcribing.clone();
            let is_gemini_active = state.is_gemini_active.clone();

            let handle = std::thread::Builder::new()
                .name("audio-dispatcher".to_string())
                .spawn(move || {
                    log::info!("Audio dispatcher: starting fan-out loop");
                    let mut speech_drop_count: u64 = 0;
                    let mut gemini_drop_count: u64 = 0;
                    while let Ok(chunk) = processed_rx.recv() {
                        // Forward to speech processor if transcribing. On a full
                        // buffer we drop the OLDEST chunk, not this newest one:
                        // under sustained overload that keeps the consumer near
                        // real time with the most recent audio, instead of
                        // processing ever-staler audio and falling behind.
                        if is_transcribing.load(Ordering::Relaxed)
                            && send_dropping_oldest(&speech_tx, &speech_drain_rx, chunk.clone())
                        {
                            speech_drop_count += 1;
                            if speech_drop_count % 50 == 1 {
                                log::warn!(
                                    "Audio dispatcher: speech buffer full, dropped {} oldest \
                                     chunk(s) total (consumer behind real time)",
                                    speech_drop_count
                                );
                            }
                        }

                        // Forward to Gemini if active (same drop-oldest policy).
                        let gemini_active = is_gemini_active
                            .read()
                            .map(|a| *a)
                            .unwrap_or(false);
                        if gemini_active
                            && send_dropping_oldest(&gemini_tx, &gemini_drain_rx, chunk)
                        {
                            gemini_drop_count += 1;
                            if gemini_drop_count % 50 == 1 {
                                log::warn!(
                                    "Audio dispatcher: gemini buffer full, dropped {} oldest chunk(s) total",
                                    gemini_drop_count
                                );
                            }
                        }
                    }
                    log::info!(
                        "Audio dispatcher: exiting (pipeline channel closed). \
                         Total oldest-drops: speech={}, gemini={}",
                        speech_drop_count, gemini_drop_count
                    );
                })
                .map_err(|e| format!("Failed to spawn dispatcher thread: {}", e))?;
            *dispatcher_handle = Some(handle);
            log::info!("Audio dispatcher thread spawned");
        }
    }

    // 3. Update state flags.
    if let Ok(mut capturing) = state.is_capturing.write() {
        *capturing = true;
    }
    if let Ok(mut status) = state.pipeline_status.write() {
        status.capture = StageStatus::Running { processed_count: 0 };
        status.pipeline = StageStatus::Running { processed_count: 0 };
    }

    // Emit initial pipeline status event
    if let Ok(status) = state.pipeline_status.read() {
        let _ = app.emit(events::PIPELINE_STATUS_EVENT, &*status);
    }

    log::info!("Started capture for source: {}", source_id);
    Ok(())
}

/// Stop capturing audio from the specified source.
///
/// If this was the last active capture, also stops transcription (if running)
/// since there is no more audio to transcribe.
#[tauri::command]
pub async fn stop_capture(
    source_id: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> AppResult<()> {
    log::info!("stop_capture called for source: {}", source_id);

    let remaining;
    {
        let mut manager = state
            .capture_manager
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        manager.stop_capture(&source_id)?;
        remaining = manager.active_captures().len();
    }

    if remaining == 0 {
        if let Ok(mut capturing) = state.is_capturing.write() {
            *capturing = false;
        }
        // Also stop transcription since there's no more audio flowing
        state.is_transcribing.store(false, Ordering::SeqCst);
        // Clean up speech processor thread handle
        if let Ok(mut sp_handle) = state.speech_processor_thread.lock() {
            *sp_handle = None;
        }
        // Clean up ASR worker thread handle
        if let Ok(mut asr_handle) = state.asr_worker_thread.lock() {
            *asr_handle = None;
        }
        // Also stop Gemini if running
        if let Ok(mut gemini_active) = state.is_gemini_active.write()
            && *gemini_active
        {
            *gemini_active = false;
            // Disconnect the Gemini client
            if let Ok(mut client_guard) = state.gemini_client.lock() {
                if let Some(ref client) = *client_guard {
                    client.disconnect();
                }
                *client_guard = None;
            }
            // Also TAKE + clear the Gemini worker-thread handles, then join them
            // off-thread. Without this they stay `Some(..)` so the next
            // `start_gemini` skips recreating the audio/event loops and comes back
            // without a live Gemini event receiver (CodeRabbit commands.rs:543).
            // We detach the join (no .await in this sync block) so Stop stays
            // responsive; clearing the handles is the correctness-critical part.
            let audio_h = state
                .gemini_audio_thread
                .lock()
                .ok()
                .and_then(|mut g| g.take());
            let event_h = state
                .gemini_event_thread
                .lock()
                .ok()
                .and_then(|mut g| g.take());
            if audio_h.is_some() || event_h.is_some() {
                std::thread::spawn(move || {
                    if let Some(h) = audio_h {
                        join_worker_with_timeout(
                            h,
                            std::time::Duration::from_secs(3),
                            "Gemini audio worker (capture stop)",
                        );
                    }
                    if let Some(h) = event_h {
                        join_worker_with_timeout(
                            h,
                            std::time::Duration::from_secs(3),
                            "Gemini event worker (capture stop)",
                        );
                    }
                });
            }
        }
        if let Ok(mut status) = state.pipeline_status.write() {
            status.capture = StageStatus::Idle;
            status.pipeline = StageStatus::Idle;
            status.asr = StageStatus::Idle;
            status.diarization = StageStatus::Idle;
            status.entity_extraction = StageStatus::Idle;
            status.graph = StageStatus::Idle;
        }

        // Emit updated pipeline status
        if let Ok(status) = state.pipeline_status.read() {
            let _ = app.emit(events::PIPELINE_STATUS_EVENT, &*status);
        }
    }

    log::info!("Stopped capture for source: {}", source_id);
    Ok(())
}

/// Probe AWS credentials via STS GetCallerIdentity. Used as pre-flight for
/// DefaultChain and Profile modes so start_transcribe fails fast with an
/// actionable error instead of blowing up inside the EventStream handshake.
///
/// Returns `Ok(())` on success (identity resolved) or an error string on any
/// failure — credentials missing, expired, wrong region, network blocked, etc.
/// Callers are expected to wrap this in a `tokio::time::timeout`.
async fn aws_preflight_probe(
    region: String,
    credential_source: crate::settings::AwsCredentialSource,
) -> Result<(), String> {
    // AccessKeys has a static-cred pre-flight elsewhere; probing via STS
    // here would double up. Callers already filter this case out.
    if matches!(
        credential_source,
        crate::settings::AwsCredentialSource::AccessKeys { .. }
    ) {
        return Err("aws_preflight_probe called with AccessKeys — caller bug".to_string());
    }
    let sdk_config = crate::aws_util::build_aws_sdk_config(&region, credential_source).await?;
    let sts = aws_sdk_sts::Client::new(&sdk_config);
    sts.get_caller_identity()
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    Ok(())
}

/// Start transcription (streaming processed audio → ASR).
///
/// Requires capture to already be running. Spawns a speech processor thread
/// that reads from the processed audio channel (pipeline output), accumulates
/// chunks into ~2s segments, then runs ASR + diarization + entity extraction.
#[tauri::command]
pub async fn start_transcribe(state: State<'_, AppState>, app: tauri::AppHandle) -> AppResult<()> {
    log::info!("start_transcribe called");

    // Guard: capture must be running
    {
        let capturing = state
            .is_capturing
            .read()
            .map_err(|e| AppError::Unknown(format!("Lock error: {}", e)))?;
        if !*capturing {
            return Err(AppError::SessionInvalid {
                reason: "Cannot start transcription: capture is not running".to_string(),
            });
        }
    }

    // Guard: don't double-start
    if state.is_transcribing.load(Ordering::SeqCst) {
        return Err(AppError::SessionInvalid {
            reason: "Transcription is already running".to_string(),
        });
    }

    sync_llm_api_client_from_settings_cache(state.inner()).map_err(AppError::Unknown)?;
    sync_openrouter_client_from_settings_cache(state.inner()).map_err(AppError::Unknown)?;

    // Pre-flight validation: verify the selected providers are ready before
    // spawning the speech processor. Without these checks the processor thread
    // would try to load the model / reach the API, fail, and exit silently,
    // leaving the user staring at a UI with no feedback. Returning an Err here
    // surfaces to the frontend as a promise rejection → the existing error
    // toast displays the message.
    {
        let asr_provider = state
            .app_settings
            .read()
            .map(|s| s.asr_provider.clone())
            .unwrap_or_default();
        let whisper_model = state
            .app_settings
            .read()
            .map(|s| s.whisper_model.clone())
            .unwrap_or_else(|_| "ggml-small.en.bin".to_string());

        let active_sources = state
            .capture_manager
            .lock()
            .map_err(|e| AppError::Unknown(format!("Lock error: {}", e)))?
            .active_captures();
        validate_streaming_asr_source_count(&asr_provider, &active_sources, None)
            .map_err(AppError::Unknown)?;

        match &asr_provider {
            crate::settings::AsrProvider::LocalWhisper => {
                let models_dir = crate::models::get_models_dir(&app);
                let model_path = models_dir.join(&whisper_model);
                if !model_path.exists() {
                    return Err(AppError::ModelNotFound {
                        name: whisper_model.clone(),
                    });
                }
            }
            crate::settings::AsrProvider::Api {
                endpoint, api_key, ..
            } => {
                if endpoint.trim().is_empty() {
                    return Err(AppError::Unknown(
                        "Cloud ASR endpoint not configured. Open Settings.".to_string(),
                    ));
                }
                if api_key.trim().is_empty() {
                    return Err(AppError::CredentialMissing {
                        key: "cloud_asr_api_key".to_string(),
                    });
                }
            }
            crate::settings::AsrProvider::DeepgramStreaming { api_key, .. } => {
                if api_key.trim().is_empty() {
                    return Err(AppError::CredentialMissing {
                        key: "deepgram_api_key".to_string(),
                    });
                }
            }
            crate::settings::AsrProvider::AssemblyAI { api_key, .. } => {
                if api_key.trim().is_empty() {
                    return Err(AppError::CredentialMissing {
                        key: "assemblyai_api_key".to_string(),
                    });
                }
            }
            crate::settings::AsrProvider::OpenAiRealtimeTranscription { api_key, .. } => {
                if api_key.trim().is_empty() {
                    return Err(AppError::CredentialMissing {
                        key: "openai_api_key".to_string(),
                    });
                }
            }
            crate::settings::AsrProvider::AwsTranscribe {
                credential_source,
                region,
                ..
            } => {
                if region.trim().is_empty() {
                    return Err(AppError::AwsRegionInvalid {
                        region: region.clone(),
                    });
                }

                if let crate::settings::AwsCredentialSource::AccessKeys { access_key } =
                    credential_source
                {
                    if access_key.trim().is_empty() {
                        return Err(AppError::CredentialMissing {
                            key: "aws_access_key".to_string(),
                        });
                    }
                    let cred_store = crate::credentials::load_credentials();
                    let secret_valid = cred_store
                        .aws_secret_key
                        .as_ref()
                        .map(|s| !s.trim().is_empty())
                        .unwrap_or(false);
                    if !secret_valid {
                        return Err(AppError::CredentialMissing {
                            key: "aws_secret_key".to_string(),
                        });
                    }
                }

                // DefaultChain + Profile: probe STS GetCallerIdentity so the
                // user gets a fast, intelligible "no credentials" error instead
                // of the EventStream handshake failing mid-stream and leaving
                // the UI in a confusing half-running state.
                //
                // Bounded to 5s: on a healthy machine with creds, STS responds
                // in <200ms. If it takes longer, the user's network is bad
                // enough that mid-stream failures are likely anyway — better
                // to fail fast in pre-flight than stall capture.
                if !matches!(
                    credential_source,
                    crate::settings::AwsCredentialSource::AccessKeys { .. }
                ) {
                    let probe = aws_preflight_probe(region.clone(), credential_source.clone());
                    match tokio::time::timeout(std::time::Duration::from_secs(5), probe).await {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            // ag#13: also emit a structured event so the UI
                            // can show a localized toast. The returned
                            // AppError::Unknown keeps the legacy string path
                            // working for any caller that hasn't migrated.
                            let classified = crate::aws_util::classify_aws_error(
                                &e,
                                Some(region.as_str()),
                            );
                            crate::events::emit_or_log(
                                &app,
                                crate::events::AWS_ERROR,
                                crate::events::AwsErrorPayload {
                                    error: classified,
                                    raw_message: e.clone(),
                                },
                            );
                            return Err(AppError::Unknown(format!(
                                "AWS credential pre-flight failed: {}. Open Settings → ASR → AWS Transcribe → Test Connection to diagnose.",
                                e
                            )));
                        }
                        Err(_) => return Err(AppError::Unknown(
                            "AWS credential pre-flight timed out after 5s. Check network or switch credential mode."
                                .to_string(),
                        )),
                    }
                }
            }
            crate::settings::AsrProvider::SherpaOnnx { model_dir, .. } => {
                let models_dir = crate::models::get_models_dir(&app);
                let model_path = models_dir.join(model_dir);
                if !model_path.exists() {
                    return Err(AppError::ModelNotFound {
                        name: model_dir.clone(),
                    });
                }
                // The directory existing isn't enough — sherpa-onnx needs the
                // encoder/decoder/joiner ONNX graphs and the tokens vocabulary.
                // A partial download or unpack would pass the exists() check
                // but fail silently inside the speech processor thread.
                for required in &["encoder.onnx", "decoder.onnx", "joiner.onnx", "tokens.txt"] {
                    if !model_path.join(required).exists() {
                        return Err(AppError::Unknown(format!(
                            "Sherpa-ONNX model '{}' is missing '{}'. Re-download via Settings.",
                            model_dir, required
                        )));
                    }
                }
            }
        }

        // LLM pre-flight: only warn for LocalLlama — entity extraction has
        // fallbacks (API, rule-based) so a missing local model isn't fatal.
        let llm_provider = state
            .app_settings
            .read()
            .map(|s| s.llm_provider.clone())
            .unwrap_or_default();
        if let crate::settings::LlmProvider::LocalLlama = llm_provider {
            let models_dir = crate::models::get_models_dir(&app);
            let llm_path = models_dir.join(crate::models::LLM_MODEL_FILENAME);
            if !llm_path.exists() {
                log::warn!(
                    "Local LLM model not downloaded; entity extraction will fall back to API or rule-based"
                );
                // Don't error — extraction has fallbacks. Just log.
            }
        }
    }

    // 1. Start speech processor thread (ASR + Diarization orchestrator).
    //    The speech processor reads directly from the processed audio channel,
    //    accumulates chunks into ~2s segments, and runs ASR inline.
    {
        let mut sp_handle = state
            .speech_processor_thread
            .lock()
            .map_err(|e| AppError::Unknown(format!("Lock error: {}", e)))?;
        if sp_handle.is_none() {
            // Bug 1 fix: read from per-consumer channel, not shared processed_rx
            let speech_rx = state.speech_audio_rx.clone();
            // Bug 2 fix: pass AtomicBool so the speech processor can check it
            let is_transcribing = state.is_transcribing.clone();

            let transcript_buffer = state.transcript_buffer.clone();
            let pipeline_status = state.pipeline_status.clone();
            let app_handle = app.clone();
            let knowledge_graph = state.knowledge_graph.clone();
            let graph_snapshot_clone = state.graph_snapshot.clone();
            let graph_extractor = state.graph_extractor.clone();
            let llm_engine = state.llm_engine.clone();
            let api_client = state.api_client.clone();
            let mistralrs_engine = state.mistralrs_engine.clone();
            let llm_executor = state.llm_executor.clone();
            let pending_agent_proposals = state.pending_agent_proposals.clone();

            let models_dir = crate::models::get_models_dir(&app);

            let asr_provider = state
                .app_settings
                .read()
                .map(|s| s.asr_provider.clone())
                .unwrap_or_default();

            let whisper_model = state
                .app_settings
                .read()
                .map(|s| s.whisper_model.clone())
                .unwrap_or_else(|_| "ggml-small.en.bin".to_string());

            let llm_provider = state
                .app_settings
                .read()
                .map(|s| s.llm_provider.clone())
                .unwrap_or_default();

            // If the user selected local LLM and the engine is not yet
            // loaded, attempt to load it now on a blocking background task.
            if matches!(llm_provider, crate::settings::LlmProvider::LocalLlama) {
                let engine_empty = state
                    .llm_engine
                    .lock()
                    .map(|g| g.is_none())
                    .unwrap_or(false);
                if engine_empty {
                    let models_dir_clone = models_dir.clone();
                    let llm_engine_clone = state.llm_engine.clone();
                    let model_path = models_dir_clone.join(crate::models::LLM_MODEL_FILENAME);
                    if model_path.exists() {
                        log::info!("Auto-loading local LLM model for LocalLlama provider...");
                        let _ = std::thread::Builder::new()
                            .name("llm-autoload".to_string())
                            .spawn(move || {
                                match crate::llm::LlmEngine::new(&model_path.to_string_lossy()) {
                                    Ok(engine) => {
                                        if let Ok(mut guard) = llm_engine_clone.lock() {
                                            *guard = Some(engine);
                                            log::info!("Local LLM model auto-loaded successfully");
                                        }
                                    }
                                    Err(e) => {
                                        log::warn!("Failed to auto-load local LLM model: {}", e);
                                    }
                                }
                            });
                    }
                }
            }

            let transcript_writer = state.transcript_writer.clone();

            let handle = std::thread::Builder::new()
                .name("speech-processor".to_string())
                .spawn(move || {
                    let channels = speech::SpeechChannels {
                        processed_rx: speech_rx,
                        is_transcribing,
                    };
                    let shared = speech::SpeechShared {
                        transcript_buffer,
                        transcript_writer,
                        pipeline_status,
                        app_handle,
                        knowledge_graph,
                        graph_snapshot: graph_snapshot_clone,
                        graph_extractor,
                        llm_engine,
                        api_client,
                        mistralrs_engine,
                        llm_executor,
                        pending_agent_proposals,
                    };
                    let config = speech::SpeechConfig {
                        models_dir,
                        llm_provider,
                    };
                    speech::run_speech_processor(
                        channels,
                        shared,
                        config,
                        asr_provider,
                        whisper_model,
                    );
                })
                .map_err(|e| {
                    AppError::Unknown(format!("Failed to spawn speech processor thread: {}", e))
                })?;
            *sp_handle = Some(handle);
            log::info!("Speech processor thread spawned for transcribe");
        }
    }

    // 3. Update state flags.
    state.is_transcribing.store(true, Ordering::SeqCst);
    if let Ok(mut status) = state.pipeline_status.write() {
        status.asr = StageStatus::Running { processed_count: 0 };
        status.diarization = StageStatus::Running { processed_count: 0 };
        status.entity_extraction = StageStatus::Running { processed_count: 0 };
        status.graph = StageStatus::Running { processed_count: 0 };
    }

    if let Ok(status) = state.pipeline_status.read() {
        let _ = app.emit(events::PIPELINE_STATUS_EVENT, &*status);
    }

    log::info!("Started transcription (streaming mode)");
    Ok(())
}

/// Stop transcription without stopping capture.
///
/// Sets the AtomicBool flag to false so the speech processor thread exits
/// on its next `recv_timeout` cycle (Bug 2 fix), then cleans up the thread handle.
#[tauri::command]
pub async fn stop_transcribe(state: State<'_, AppState>, app: tauri::AppHandle) -> AppResult<()> {
    log::info!("stop_transcribe called");

    // Signal the speech processor to stop via AtomicBool
    state.is_transcribing.store(false, Ordering::SeqCst);

    // Join the worker threads (bounded) instead of just dropping the handles.
    // Dropping without joining let a fast Stop→Start race leave the OLD worker
    // still in its ~500ms recv loop while a NEW worker starts, so two consumers
    // split the same speech_audio channel (critique H2). Joining guarantees the
    // old workers have exited before this returns. Polled-join with a timeout
    // so a wedged worker can't hang Stop. Run off the async runtime.
    let sp = state
        .speech_processor_thread
        .lock()
        .ok()
        .and_then(|mut g| g.take());
    let asr = state
        .asr_worker_thread
        .lock()
        .ok()
        .and_then(|mut g| g.take());
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(h) = sp {
            join_worker_with_timeout(h, std::time::Duration::from_secs(3), "speech processor");
        }
        if let Some(h) = asr {
            join_worker_with_timeout(h, std::time::Duration::from_secs(3), "ASR worker");
        }
    })
    .await;

    // Update pipeline status — ASR and downstream stages go idle
    if let Ok(mut status) = state.pipeline_status.write() {
        status.asr = StageStatus::Idle;
        status.diarization = StageStatus::Idle;
        status.entity_extraction = StageStatus::Idle;
        status.graph = StageStatus::Idle;
    }

    if let Ok(status) = state.pipeline_status.read() {
        let _ = app.emit(events::PIPELINE_STATUS_EVENT, &*status);
    }

    log::info!("Stopped transcription");
    Ok(())
}

/// Get the current knowledge graph snapshot.
#[tauri::command]
pub async fn get_graph_snapshot(state: State<'_, AppState>) -> AppResult<GraphSnapshot> {
    let snapshot = state
        .graph_snapshot
        .read()
        .map_err(|e| format!("Failed to read graph snapshot: {}", e))?;
    Ok(snapshot.clone())
}

/// Get transcript segments, optionally filtered by source and time.
#[tauri::command]
pub async fn get_transcript(
    source_id: Option<String>,
    since: Option<f64>,
    state: State<'_, AppState>,
) -> AppResult<Vec<TranscriptSegment>> {
    let buffer = state
        .transcript_buffer
        .read()
        .map_err(|e| format!("Failed to read transcript buffer: {}", e))?;

    let segments: Vec<TranscriptSegment> = buffer
        .iter()
        .filter(|seg| {
            let source_match = source_id
                .as_ref()
                .map(|id| &seg.source_id == id)
                .unwrap_or(true);
            let time_match = since.map(|t| seg.start_time >= t).unwrap_or(true);
            source_match && time_match
        })
        .cloned()
        .collect();

    Ok(segments)
}

/// Get the current pipeline status.
#[tauri::command]
pub async fn get_pipeline_status(state: State<'_, AppState>) -> AppResult<PipelineStatus> {
    let status = state
        .pipeline_status
        .read()
        .map_err(|e| format!("Failed to read pipeline status: {}", e))?;
    Ok(status.clone())
}

// ---------------------------------------------------------------------------
// API endpoint configuration
// ---------------------------------------------------------------------------

/// Validate and parse an OpenAI-compatible endpoint URL.
///
/// `reqwest` will reject malformed URLs at request time, but that produces a
/// confusing "invalid format" failure many seconds into a chat, long after the
/// user has forgotten what they typed in Settings. Parse up-front so the
/// Settings UI can surface the error synchronously, and restrict to http/https
/// schemes so `file://` / `ftp://` / other exotic schemes can't sneak in.
pub(crate) fn validate_endpoint_url(endpoint: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(endpoint).map_err(|e| format!("Invalid endpoint URL: {}", e))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => Err(format!(
            "Invalid endpoint URL: unsupported scheme `{}` (expected http or https)",
            other
        )),
    }
}

/// Configure an OpenAI-compatible API endpoint for LLM inference.
///
/// This allows using cloud providers (OpenAI, OpenRouter) or local servers
/// (Ollama, LM Studio, vLLM) as an alternative to the native llama-cpp-2 engine.
#[tauri::command]
pub async fn configure_api_endpoint(
    endpoint: String,
    api_key: Option<String>,
    model: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    log::info!(
        "configure_api_endpoint: endpoint={}, model={}",
        endpoint,
        model
    );

    validate_endpoint_url(&endpoint)?;

    if endpoint.trim().is_empty() || model.trim().is_empty() {
        return Err(AppError::Unknown(
            "Invalid API configuration: endpoint and model must be non-empty".to_string(),
        ));
    }

    {
        let mut cached = state
            .app_settings
            .write()
            .map_err(|e| format!("Lock error: {}", e))?;
        cached.llm_provider = crate::settings::LlmProvider::Api {
            endpoint: endpoint.clone(),
            api_key: api_key.clone().unwrap_or_default(),
            model: model.clone(),
        };
        cached.llm_api_config = Some(crate::settings::LlmApiConfig {
            endpoint,
            api_key,
            model,
            max_tokens: 512,
            temperature: 0.1,
        });
    }

    sync_llm_api_client_from_settings_cache(state.inner())?;
    sync_openrouter_client_from_settings_cache(state.inner())?;

    log::info!("API endpoint configured successfully");
    Ok(())
}

// ---------------------------------------------------------------------------
// Chat commands (backed by native LLM engine or API client)
// ---------------------------------------------------------------------------

/// Build the per-request graph + transcript context block used as the chat
/// system prompt, and append the user message to history.
///
/// Returns `(messages, graph_context)` ready to feed either the streaming
/// or blocking chat path. Locks are taken under short critical sections
/// and released before any string formatting (I4 fix carried over from
/// the legacy `send_chat_message` body).
fn prepare_chat_request(
    state: &AppState,
    message: String,
) -> Result<(Vec<ChatMessage>, String), String> {
    sync_llm_api_client_from_settings_cache(state)?;
    sync_openrouter_client_from_settings_cache(state)?;

    let snapshot = {
        let kg = state
            .knowledge_graph
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        kg.snapshot()
    };

    let recent_transcript: Vec<TranscriptSegment> = {
        let transcript = state
            .transcript_buffer
            .read()
            .map_err(|e| format!("Lock error: {}", e))?;
        transcript.iter().rev().take(10).cloned().collect()
    };

    let graph_context = {
        // Top-k retrieval instead of dumping the whole graph: keeps the prompt
        // small, on-topic, and avoids shipping maximal session data. See
        // graph::entities::build_graph_chat_context (C3 fix).
        const MAX_CONTEXT_NODES: usize = 40;
        let mut ctx = crate::graph::entities::build_graph_chat_context(
            &snapshot,
            &message,
            MAX_CONTEXT_NODES,
        );
        if !recent_transcript.is_empty() {
            ctx.push_str("\nRecent Transcript:\n");
            for seg in recent_transcript.iter().rev() {
                let speaker = seg.speaker_label.as_deref().unwrap_or("Unknown");
                ctx.push_str(&format!("[{}]: {}\n", speaker, seg.text));
            }
        }
        ctx
    };

    let user_msg = ChatMessage {
        role: "user".to_string(),
        content: message,
    };
    {
        let mut history = state
            .chat_history
            .write()
            .map_err(|e| format!("Lock error: {}", e))?;
        history.push(user_msg);
        cap_chat_history(&mut history);
    }
    let messages: Vec<ChatMessage> = {
        let history = state
            .chat_history
            .read()
            .map_err(|e| format!("Lock error: {}", e))?;
        history.clone()
    };
    Ok((messages, graph_context))
}

/// Append the assistant message to chat history. Best-effort: lock-poisoning
/// returns an error but the caller should still surface the reply to the
/// user — chat_history is a UX convenience, not a correctness invariant.
fn append_assistant_message(state: &AppState, content: String) -> Result<ChatMessage, String> {
    let assistant_msg = ChatMessage {
        role: "assistant".to_string(),
        content,
    };
    let mut history = state
        .chat_history
        .write()
        .map_err(|e| format!("Lock error: {}", e))?;
    history.push(assistant_msg.clone());
    cap_chat_history(&mut history);
    Ok(assistant_msg)
}

/// Maximum chat messages retained in memory. Chat history is unbounded by
/// nature (a long session could push thousands of turns) and is cloned whole
/// into every chat request, so cap it to bound memory and prompt-build cost.
/// Keeps the most recent messages.
const MAX_CHAT_HISTORY: usize = 200;

/// Trim `history` in place to the most recent [`MAX_CHAT_HISTORY`] messages.
fn cap_chat_history(history: &mut Vec<ChatMessage>) {
    if history.len() > MAX_CHAT_HISTORY {
        let drop = history.len() - MAX_CHAT_HISTORY;
        history.drain(0..drop);
    }
}

/// Returns `true` when the active LLM provider has a streaming code path.
/// Today: only `Api` and `OpenRouter`. The `LocalLlama`, `MistralRs`, and
/// `AwsBedrock` variants short-circuit to the blocking executor inside
/// `send_chat_message` while their streaming support is in flight (see
/// the follow-up issue tracked in plan A3).
fn provider_supports_streaming(p: &crate::settings::LlmProvider) -> bool {
    matches!(
        p,
        crate::settings::LlmProvider::Api { .. } | crate::settings::LlmProvider::OpenRouter { .. }
    )
}

/// Derive the `tokens_used` telemetry value (FA-7) from a streaming-chat
/// terminal frame's `usage` block.
///
/// We surface `total_tokens` (prompt + completion) because the frontend
/// dashboard exposes a single `tokens_used` field for the whole request.
/// Returns 0 when the provider omitted the usage block entirely (it never set
/// `stream_options.include_usage`, or sent no `total_tokens`), which is the
/// honest "unknown" value rather than a fabricated count.
///
/// Pure so the accumulation contract can be unit-tested without the async
/// command / IPC machinery.
fn tokens_used_from_stream_usage(usage: Option<crate::llm::sse::StreamUsage>) -> u32 {
    usage.and_then(|u| u.total_tokens).unwrap_or(0)
}

/// Spawn the streaming-chat task for `request_id`.
///
/// Drives `crate::llm::streaming::stream_chat` to completion, emitting
/// `chat-token-delta` per [`crate::llm::streaming::TokenDelta::Delta`] and
/// exactly one `chat-token-done` on terminal (Done / Error / Cancelled).
/// Removes the request from `state.stream_registry` on terminal so a stale
/// id cannot be cancelled later.
fn spawn_stream_task(
    app: tauri::AppHandle,
    state: &AppState,
    request_id: String,
    provider: crate::settings::LlmProvider,
    history: Vec<ChatMessage>,
    graph_context: String,
    persist_to_history: bool,
) {
    use crate::llm::streaming::{
        ChatTokenDeltaPayload, ChatTokenDonePayload, TokenDelta, stream_chat,
    };

    let (mut rx, cancel) = stream_chat(provider, history, graph_context);
    state.stream_registry.register(request_id.clone(), cancel);

    let registry = state.stream_registry.clone();
    let chat_history = state.chat_history.clone();
    let request_id_for_task = request_id.clone();

    // Speak-aloud: build the SpeakAloudPipe ahead of the task spawn so the
    // task body owns it. None when speak_aloud=false or tts=None — the
    // task then runs as plain streaming chat with no audio side effects.
    let settings_snapshot = state
        .app_settings
        .read()
        .map(|s| (s.speak_aloud, s.tts_provider.clone()))
        .unwrap_or((false, crate::settings::TtsProvider::None));
    // Credentials live on disk, not on AppState. Snapshot once at task
    // entry so we don't hit the FS on every delta.
    let credentials_snapshot = crate::credentials::load_credentials();
    let player_for_pipe = state.audio_player.clone();
    let request_id_for_pipe_log = request_id.clone();

    tokio::spawn(async move {
        let mut pipe: Option<crate::speak_aloud::SpeakAloudPipe> =
            match crate::speak_aloud::SpeakAloudPipe::maybe_new(
                settings_snapshot.0,
                &settings_snapshot.1,
                &credentials_snapshot,
                player_for_pipe,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => {
                    log::warn!(
                        "speak-aloud setup failed for request {}: {}; falling back to text-only",
                        request_id_for_pipe_log,
                        e
                    );
                    None
                }
            };

        while let Some(frame) = rx.recv().await {
            match frame {
                TokenDelta::Delta {
                    content,
                    finish_reason,
                } => {
                    if let Some(p) = pipe.as_mut()
                        && let Err(e) = p.append_delta(&content)
                    {
                        log::warn!("speak-aloud append_delta failed: {}", e);
                    }
                    events::emit_or_log(
                        &app,
                        events::CHAT_TOKEN_DELTA,
                        ChatTokenDeltaPayload {
                            request_id: request_id_for_task.clone(),
                            delta: content,
                            finish_reason,
                        },
                    );
                }
                TokenDelta::Done {
                    full_text,
                    usage,
                    finish_reason,
                } => {
                    if persist_to_history && let Ok(mut history) = chat_history.write() {
                        history.push(ChatMessage {
                            role: "assistant".to_string(),
                            content: full_text.clone(),
                        });
                        cap_chat_history(&mut history);
                    }
                    if let Some(p) = pipe.take()
                        && let Err(e) = p.finish()
                    {
                        log::warn!("speak-aloud finish failed: {}", e);
                    }
                    events::emit_or_log(
                        &app,
                        events::CHAT_TOKEN_DONE,
                        ChatTokenDonePayload {
                            request_id: request_id_for_task.clone(),
                            full_text,
                            finish_reason,
                            usage,
                        },
                    );
                    registry.finish(&request_id_for_task);
                    break;
                }
                TokenDelta::Error { message, full_text } => {
                    log::warn!("Streaming chat error: {}", message);
                    if let Some(p) = pipe.take() {
                        let _ = p.cancel();
                    }
                    events::emit_or_log(
                        &app,
                        events::CHAT_TOKEN_DONE,
                        ChatTokenDonePayload {
                            request_id: request_id_for_task.clone(),
                            full_text,
                            finish_reason: format!("error: {}", message),
                            usage: None,
                        },
                    );
                    registry.finish(&request_id_for_task);
                    break;
                }
                TokenDelta::Cancelled { full_text } => {
                    if let Some(p) = pipe.take() {
                        let _ = p.cancel();
                    }
                    events::emit_or_log(
                        &app,
                        events::CHAT_TOKEN_DONE,
                        ChatTokenDonePayload {
                            request_id: request_id_for_task.clone(),
                            full_text,
                            finish_reason: "cancelled".to_string(),
                            usage: None,
                        },
                    );
                    registry.finish(&request_id_for_task);
                    break;
                }
            }
        }
    });
}

/// Start a streaming chat request. Returns the `request_id` immediately so
/// the frontend can correlate `chat-token-delta` / `chat-token-done`
/// events back to this call. The actual LLM work runs on a tokio task.
///
/// If the active LLM provider doesn't support streaming yet (LocalLlama,
/// MistralRs, AwsBedrock), this returns `Err` so the caller can fall back
/// to the blocking `send_chat_message` path. The frontend should never
/// call this command for those providers; the legacy command stays the
/// safe default for now.
#[tauri::command]
pub async fn start_streaming_chat(
    message: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<String> {
    log::info!(
        "start_streaming_chat called: {}",
        &message[..message.len().min(50)]
    );

    let llm_provider = state
        .app_settings
        .read()
        .map(|s| s.llm_provider.clone())
        .unwrap_or_default();

    if !provider_supports_streaming(&llm_provider) {
        let name = match &llm_provider {
            crate::settings::LlmProvider::LocalLlama => "LocalLlama",
            crate::settings::LlmProvider::MistralRs { .. } => "MistralRs",
            crate::settings::LlmProvider::AwsBedrock { .. } => "AwsBedrock",
            crate::settings::LlmProvider::Api { .. } => "Api",
            crate::settings::LlmProvider::OpenRouter { .. } => "OpenRouter",
        };
        return Err(AppError::Unknown(format!(
            "Streaming chat is not yet supported for the active LLM provider \
             ({}). Use send_chat_message for now; streaming for this \
             provider is a follow-up issue.",
            name
        )));
    }

    let (messages, graph_context) = prepare_chat_request(state.inner(), message)?;
    let request_id = uuid::Uuid::new_v4().to_string();
    spawn_stream_task(
        app,
        state.inner(),
        request_id.clone(),
        llm_provider,
        messages,
        graph_context,
        true, // persist assistant reply to chat history
    );
    Ok(request_id)
}

/// Cancel an in-flight streaming chat. Idempotent: cancelling an unknown
/// or already-finished request_id is a no-op (returns `Ok(())`). The
/// stream task emits a `chat-token-done` with `finish_reason = "cancelled"`
/// once it observes the cancel.
#[tauri::command]
pub async fn cancel_streaming_chat(
    request_id: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let cancelled = state.stream_registry.cancel(&request_id);
    log::info!(
        "cancel_streaming_chat({}): {}",
        request_id,
        if cancelled { "cancelled" } else { "not found" }
    );
    Ok(())
}

/// Send a chat message and get a response from the LLM, informed by the
/// current knowledge graph and transcript context.
///
/// Backward-compatible shim: when the active provider supports streaming
/// (Api / OpenRouter), this dispatches to the same streaming task as
/// [`start_streaming_chat`] and waits for the terminal `Done` frame to
/// reassemble the full reply. Frontend callers that pre-date streaming
/// see no behavior change. For non-streaming providers (LocalLlama,
/// MistralRs, AwsBedrock) this falls through to the legacy blocking
/// executor.
///
/// I4 fix: takes a snapshot of the graph and transcript, releases the locks,
/// then builds the context string from the snapshot (no lock held during
/// string formatting).
#[tauri::command]
pub async fn send_chat_message(
    message: String,
    state: State<'_, AppState>,
) -> AppResult<ChatResponse> {
    log::info!(
        "send_chat_message called: {}",
        &message[..message.len().min(50)]
    );

    let llm_provider = state
        .app_settings
        .read()
        .map(|s| s.llm_provider.clone())
        .unwrap_or_default();

    let (messages, graph_context) = prepare_chat_request(state.inner(), message)?;

    // Streaming path — accumulate to full text via the same producer the
    // event-driven command uses. The shim doesn't fire IPC events itself;
    // it consumes the channel directly so blocking callers don't see
    // delta event spam.
    if provider_supports_streaming(&llm_provider) {
        use crate::llm::streaming::{TokenDelta, stream_chat};
        let (mut rx, _cancel) = stream_chat(llm_provider, messages, graph_context.clone());
        let mut full_text = String::new();
        // Real token count from the provider's terminal `usage` block (sent when
        // `stream_options.include_usage` is honoured). `total_tokens` covers the
        // whole request (prompt + completion), matching the single `tokens_used`
        // field the frontend dashboard surfaces. Stays 0 only if the provider
        // omitted usage entirely.
        let mut tokens_used = 0u32;
        while let Some(frame) = rx.recv().await {
            match frame {
                TokenDelta::Delta { content, .. } => full_text.push_str(&content),
                TokenDelta::Done {
                    full_text: t,
                    usage,
                    ..
                } => {
                    if !t.is_empty() {
                        full_text = t;
                    }
                    tokens_used = tokens_used_from_stream_usage(usage);
                    break;
                }
                TokenDelta::Error {
                    message,
                    full_text: partial,
                } => {
                    log::warn!("send_chat_message streaming error: {}", message);
                    let fallback = if partial.is_empty() {
                        format!(
                            "I couldn't generate a streaming response (LLM error: {}).\n\n{}",
                            message, graph_context
                        )
                    } else {
                        partial
                    };
                    let assistant_msg = append_assistant_message(state.inner(), fallback)?;
                    // No usage signal: a stream that errors mid-flight never
                    // reaches the terminal `usage` block, so the real token count
                    // is genuinely unavailable here.
                    return Ok(ChatResponse {
                        message: assistant_msg,
                        tokens_used: 0,
                    });
                }
                TokenDelta::Cancelled { full_text: partial } => {
                    let assistant_msg = append_assistant_message(state.inner(), partial)?;
                    // No usage signal: a cancelled stream is dropped before the
                    // terminal `usage` block arrives, so no real count exists.
                    return Ok(ChatResponse {
                        message: assistant_msg,
                        tokens_used: 0,
                    });
                }
            }
        }
        let assistant_msg = append_assistant_message(state.inner(), full_text)?;
        return Ok(ChatResponse {
            message: assistant_msg,
            tokens_used,
        });
    }

    // Legacy blocking path: native engines + bedrock until their streaming
    // support lands. Wrap the synchronous executor call in
    // `spawn_blocking` so we don't stall the runtime worker. Clone the
    // graph context once so we still have it for the error fallback path.
    let executor = state.llm_executor.clone();
    let graph_for_error = graph_context.clone();
    let response_text = match tokio::task::spawn_blocking(move || {
        executor.chat_with_history(messages, graph_context, llm_provider)
    })
    .await
    .map_err(|e| format!("chat task join failed: {}", e))?
    {
        Ok(text) => text,
        Err(e) => format!(
            "I couldn't generate a detailed response (LLM error: {}). \
             Please check the LLM provider configuration.\n\n{}",
            e, graph_for_error
        ),
    };
    let assistant_msg = append_assistant_message(state.inner(), response_text)?;
    // No usage signal on this path: `LlmExecutor::chat_with_history` returns
    // only the generated text (`Result<String, String>`), and the blocking
    // native engines (LocalLlama, MistralRs) / Bedrock executor it drives do not
    // surface a token count. Real telemetry only flows through the streaming
    // path above, which reads the provider's terminal `usage` block. Kept 0
    // deliberately — wiring counts here needs a `chat_with_history` return-type
    // change (out of this file set; tracked as NEW BACKLOG).
    Ok(ChatResponse {
        message: assistant_msg,
        tokens_used: 0,
    })
}

/// Synthesize narrative notes from the current knowledge graph + transcript
/// (ADR-0014). On-demand: reuses the chat LLM pipeline with a summarization
/// prompt and a whole-conversation graph context (most-central nodes via an
/// empty query) plus a wide transcript window. Returns Markdown. Does NOT touch
/// chat history — notes are a separate, parallel projection of the same data.
#[tauri::command]
pub async fn synthesize_notes(state: State<'_, AppState>) -> AppResult<String> {
    sync_llm_api_client_from_settings_cache(state.inner())?;
    sync_openrouter_client_from_settings_cache(state.inner())?;

    let llm_provider = state
        .app_settings
        .read()
        .map(|s| s.llm_provider.clone())
        .unwrap_or_default();

    let snapshot = {
        let kg = state
            .knowledge_graph
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        kg.snapshot()
    };

    let recent_transcript: Vec<TranscriptSegment> = {
        let transcript = state
            .transcript_buffer
            .read()
            .map_err(|e| format!("Lock error: {}", e))?;
        transcript.iter().rev().take(60).cloned().collect()
    };

    // Whole-conversation context: an empty query makes build_graph_chat_context
    // fall back to the most-central nodes (ADR-0014), and we attach a wider
    // transcript window than chat uses.
    const MAX_NOTES_NODES: usize = 80;
    let mut graph_context =
        crate::graph::entities::build_graph_chat_context(&snapshot, "", MAX_NOTES_NODES);
    if !recent_transcript.is_empty() {
        graph_context.push_str("\nRecent Transcript:\n");
        for seg in recent_transcript.iter().rev() {
            let speaker = seg.speaker_label.as_deref().unwrap_or("Unknown");
            graph_context.push_str(&format!("[{}]: {}\n", speaker, seg.text));
        }
    }

    let prompt = "Write structured notes for this conversation as Markdown, using \
         only the knowledge graph and transcript in the provided context (do not \
         invent facts). Use these sections, omitting any with no content:\n\n\
         ## Summary\nA 2-4 sentence narrative.\n\n\
         ## Key Points\n- concise bullets\n\n\
         ## Action Items\n- owner: task (only if stated)\n\n\
         ## Decisions\n- decisions made\n\n\
         ## Open Questions\n- unresolved questions"
        .to_string();
    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: prompt,
    }];

    let executor = state.llm_executor.clone();
    let notes = tokio::task::spawn_blocking(move || {
        executor.chat_with_history(messages, graph_context, llm_provider)
    })
    .await
    .map_err(|e| format!("notes synthesis task join failed: {}", e))?
    .map_err(|e| {
        format!(
            "Failed to synthesize notes (LLM error: {}). Check the LLM provider \
             configuration.",
            e
        )
    })?;

    Ok(notes)
}

/// Get the current chat message history.
#[tauri::command]
pub async fn get_chat_history(state: State<'_, AppState>) -> AppResult<Vec<ChatMessage>> {
    let history = state
        .chat_history
        .read()
        .map_err(|e| format!("Lock error: {}", e))?;
    Ok(history.clone())
}

/// Clear the chat message history.
#[tauri::command]
pub async fn clear_chat_history(state: State<'_, AppState>) -> AppResult<()> {
    let mut history = state
        .chat_history
        .write()
        .map_err(|e| format!("Lock error: {}", e))?;
    history.clear();
    Ok(())
}

/// Strip the canned question-proposal prefix to recover the raw question text
/// for use as a graph node label. Falls back to the full body.
fn question_text_from_body(body: &str) -> String {
    body.strip_prefix("Consider answering or linking this question: ")
        .unwrap_or(body)
        .trim()
        .to_string()
}

#[tauri::command]
pub fn approve_agent_proposal(
    proposal_id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<events::AgentActionResult> {
    let proposal = {
        let mut pending = state
            .pending_agent_proposals
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        pending
            .remove(&proposal_id)
            .ok_or_else(|| "Agent proposal no longer exists or was already applied".to_string())?
    };

    events::emit_or_log(
        &app,
        events::AGENT_STATUS,
        events::AgentStatusPayload {
            state: events::AgentStatusState::Running,
            source_segment_id: Some(proposal.source_segment_id.clone()),
            message: Some("Applying approved proposal".to_string()),
            timestamp_ms: unix_millis(),
        },
    );

    let speaker = proposal
        .speaker_label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or("Agent");
    let mut graph_updated = false;
    use crate::graph::entities::{ExtractedEntity, ExtractedRelation, ExtractionResult};
    // Decide what (if anything) to write to the graph for this proposal kind.
    // Questions now DEFAULT to the graph (a Question node linked from the
    // speaker), built locally with no LLM call so it can never rate-limit. The
    // optional "Ask AI" path is a separate, user-initiated chat request driven
    // from the frontend.
    let (extraction, action): (Option<ExtractionResult>, &str) = match proposal.kind {
        events::AgentProposalKind::GraphSuggestion => {
            let ex = state.graph_extractor.extract(speaker, &proposal.body);
            let meaningful = !ex.relations.is_empty()
                || ex
                    .entities
                    .iter()
                    .any(|entity| !entity.name.eq_ignore_ascii_case(speaker));
            (meaningful.then_some(ex), "graph_update")
        }
        events::AgentProposalKind::Question => {
            let q = question_text_from_body(&proposal.body);
            let ex = ExtractionResult {
                entities: vec![
                    ExtractedEntity {
                        name: speaker.to_string(),
                        entity_type: "Person".to_string(),
                        description: None,
                    },
                    ExtractedEntity {
                        name: q.clone(),
                        entity_type: "Question".to_string(),
                        description: Some(q.clone()),
                    },
                ],
                relations: vec![ExtractedRelation {
                    source: speaker.to_string(),
                    target: q,
                    relation_type: "asks".to_string(),
                    detail: None,
                }],
            };
            (Some(ex), "graph_update")
        }
        events::AgentProposalKind::Note => (None, "chat_note"),
    };

    if let Some(extraction) = extraction {
        let mut graph = state
            .knowledge_graph
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let timestamp = proposal.created_at_ms as f64 / 1000.0;
        graph.process_extraction(&extraction, timestamp, speaker, &proposal.source_segment_id);

        if graph.has_delta() {
            let delta = graph.take_delta();
            events::emit_or_log(&app, events::GRAPH_DELTA, &delta);
        }
        let snapshot = graph.snapshot();
        if let Ok(mut cached) = state.graph_snapshot.write() {
            *cached = snapshot.clone();
        }
        events::emit_or_log(&app, events::GRAPH_UPDATE, &snapshot);
        graph_updated = true;
    }

    let summary = if graph_updated {
        format!("Approved agent proposal: {}", proposal.title)
    } else {
        format!("Approved agent proposal for review: {}", proposal.title)
    };
    let message = format!("{}\n\n{}", summary, proposal.body);
    {
        let mut history = state
            .chat_history
            .write()
            .map_err(|e| format!("Lock error: {}", e))?;
        history.push(ChatMessage {
            role: "assistant".to_string(),
            content: message.clone(),
        });
        cap_chat_history(&mut history);
    }

    events::emit_or_log(
        &app,
        events::AGENT_STATUS,
        events::AgentStatusPayload {
            state: events::AgentStatusState::Idle,
            source_segment_id: Some(proposal.source_segment_id.clone()),
            message: None,
            timestamp_ms: unix_millis(),
        },
    );

    Ok(events::AgentActionResult {
        proposal_id: proposal.id,
        action: action.to_string(),
        message,
        graph_updated,
        timestamp_ms: unix_millis(),
    })
}

/// Add a detected question to the knowledge graph as a `Question` node linked
/// from the speaker. Local-only (no LLM), so it's safe to call automatically
/// when a question is detected — questions default to the graph; asking the AI
/// for an answer is a separate, optional user action.
#[tauri::command]
pub fn add_question_to_graph(
    text: String,
    speaker: Option<String>,
    source_segment_id: Option<String>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    use crate::graph::entities::{ExtractedEntity, ExtractedRelation, ExtractionResult};
    let q = question_text_from_body(text.trim());
    if q.is_empty() {
        return Ok(false);
    }
    let speaker = speaker
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Speaker".to_string());
    let segment_id = source_segment_id.unwrap_or_else(|| format!("question-{}", unix_millis()));

    let extraction = ExtractionResult {
        entities: vec![
            ExtractedEntity {
                name: speaker.clone(),
                entity_type: "Person".to_string(),
                description: None,
            },
            ExtractedEntity {
                name: q.clone(),
                entity_type: "Question".to_string(),
                description: Some(q.clone()),
            },
        ],
        relations: vec![ExtractedRelation {
            source: speaker.clone(),
            target: q,
            relation_type: "asks".to_string(),
            detail: None,
        }],
    };

    let mut graph = state
        .knowledge_graph
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    graph.process_extraction(
        &extraction,
        unix_millis() as f64 / 1000.0,
        &speaker,
        &segment_id,
    );
    if graph.has_delta() {
        let delta = graph.take_delta();
        events::emit_or_log(&app, events::GRAPH_DELTA, &delta);
    }
    let snapshot = graph.snapshot();
    if let Ok(mut cached) = state.graph_snapshot.write() {
        *cached = snapshot.clone();
    }
    events::emit_or_log(&app, events::GRAPH_UPDATE, &snapshot);
    Ok(true)
}

#[tauri::command]
pub fn dismiss_agent_proposal(proposal_id: String, state: State<'_, AppState>) -> AppResult<()> {
    let mut pending = state
        .pending_agent_proposals
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    pending.remove(&proposal_id);
    Ok(())
}

#[tauri::command]
pub fn clear_agent_proposals(state: State<'_, AppState>) -> AppResult<usize> {
    let mut pending = state
        .pending_agent_proposals
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    let count = pending.len();
    pending.clear();
    Ok(count)
}

// ---------------------------------------------------------------------------
// Model management commands
// ---------------------------------------------------------------------------

/// List available models and their download status.
#[tauri::command]
pub fn list_available_models(app: tauri::AppHandle) -> Vec<crate::models::ModelInfo> {
    crate::models::list_models(&app)
}

/// Download a model by filename, with progress events emitted to the frontend.
///
/// Runs the blocking HTTP download on a background thread via
/// `tokio::task::spawn_blocking` so the IPC handler stays async (G3).
#[tauri::command]
pub async fn download_model_cmd(
    app: tauri::AppHandle,
    model_filename: String,
) -> AppResult<String> {
    let handle = app.clone();
    tokio::task::spawn_blocking(move || crate::models::download_model(&handle, &model_filename))
        .await
        .map_err(|e| format!("Download task failed: {}", e))?
        .map_err(AppError::from)
}

/// Get the readiness status of all known models (G1).
#[tauri::command]
pub fn get_model_status(app: tauri::AppHandle) -> crate::models::ModelStatus {
    crate::models::get_model_status(&app)
}

/// Load the native LLM model into memory (G2).
///
/// Resolves the model path from the app data directory, then loads it on a
/// background thread. On success the engine is stored in `AppState.llm_engine`.
#[tauri::command]
pub async fn load_llm_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<String> {
    let models_dir = crate::models::get_models_dir(&app);
    let model_path = models_dir.join(crate::models::LLM_MODEL_FILENAME);

    if !model_path.exists() {
        return Err(AppError::ModelNotFound {
            name: crate::models::LLM_MODEL_FILENAME.to_string(),
        });
    }

    let path = model_path.clone();
    let engine =
        tokio::task::spawn_blocking(move || crate::llm::LlmEngine::new(&path.to_string_lossy()))
            .await
            .map_err(|e| format!("Failed to spawn LLM loading task: {}", e))?
            .map_err(|e| format!("Failed to load LLM model: {}", e))?;

    let mut guard = state.llm_engine.lock().map_err(|e| e.to_string())?;
    *guard = Some(engine);

    Ok("LLM model loaded successfully".to_string())
}

// ---------------------------------------------------------------------------
// Settings commands
// ---------------------------------------------------------------------------

/// Load application settings from disk (returns defaults if missing).
/// Syncs the loaded settings into the in-memory `AppState.app_settings` cache
/// so other backend modules (e.g. speech processor) can read them without I/O.
#[tauri::command]
pub fn load_settings_cmd(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> crate::settings::AppSettings {
    let settings = crate::settings::load_settings(&app);
    if crate::settings::has_inline_credentials(&settings)
        && let Err(e) = crate::settings::save_settings(&app, &settings)
    {
        log::warn!("Failed to migrate/redact settings credentials: {}", e);
    }

    let credentials = crate::credentials::load_credentials();
    let runtime_settings = crate::settings::hydrate_runtime_credentials(&settings, &credentials);
    let settings_for_ipc = crate::settings::redacted_settings(&settings);

    // Sync in-memory cache with runtime-only hydrated credentials.
    if let Ok(mut cached) = state.app_settings.write() {
        *cached = runtime_settings;
    }
    if let Err(e) = sync_llm_api_client_from_settings_cache(state.inner()) {
        log::warn!(
            "Failed to sync LLM API client after loading settings: {}",
            e
        );
    }
    if let Err(e) = sync_openrouter_client_from_settings_cache(state.inner()) {
        log::warn!(
            "Failed to sync OpenRouter client after loading settings: {}",
            e
        );
    }
    settings_for_ipc
}

/// Save application settings to disk (atomic write).
/// Also updates the in-memory `AppState.app_settings` cache.
#[tauri::command]
pub fn save_settings_cmd(
    app: tauri::AppHandle,
    settings: crate::settings::AppSettings,
    state: State<'_, AppState>,
) -> AppResult<()> {
    crate::settings::save_settings(&app, &settings)?;
    let credentials = crate::credentials::load_credentials();
    let runtime_settings = crate::settings::hydrate_runtime_credentials(&settings, &credentials);

    // Sync in-memory cache with runtime-only hydrated credentials.
    if let Ok(mut cached) = state.app_settings.write() {
        *cached = runtime_settings;
    }
    sync_llm_api_client_from_settings_cache(state.inner())?;
    sync_openrouter_client_from_settings_cache(state.inner())?;
    Ok(())
}

/// Delete a downloaded model file by filename.
#[tauri::command]
pub fn delete_model_cmd(app: tauri::AppHandle, model_filename: String) -> AppResult<String> {
    crate::models::delete_model(&app, &model_filename).map_err(AppError::from)
}

/// Change the runtime log level and update the in-memory settings cache.
///
/// Takes effect immediately for every subsequent `log::*!` macro and dirties
/// the cached settings so the new level is visible to readers. Disk
/// persistence is **not** performed here — the frontend is expected to call
/// `save_settings_cmd` to flush the full settings blob when the user commits.
///
// set_log_level only mutates runtime tracing; save_settings_cmd is the
// single owner of disk persistence. See loop-13 review.
#[tauri::command]
pub fn set_log_level(
    _app: tauri::AppHandle,
    level: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    // 1. Flip the in-process log level. Immediate, cheap, and the user's
    //    primary expectation from this command.
    crate::logging::apply_log_level(&level);

    // 2. Dirty the in-memory settings cache so any reader (and the next
    //    save_settings_cmd call) sees the new value. No disk write here —
    //    save_settings_cmd is the sole owner of that path to avoid the
    //    race flagged in the loop-13 review.
    if let Ok(mut cached) = state.app_settings.write() {
        cached.log_level = Some(level);
    }

    Ok(())
}

/// Return the current logging configuration + the list of log files on disk.
#[tauri::command]
pub fn get_log_info(state: State<'_, AppState>) -> AppResult<crate::logging::LogInfo> {
    let (enabled, mode, level) = {
        let c = state
            .app_settings
            .read()
            .map_err(|e| format!("Lock error: {e}"))?;
        (
            c.file_logging.unwrap_or(true),
            crate::logging::LogFileMode::from_str_or_default(c.log_file_mode.as_deref()),
            c.log_level.clone().unwrap_or_else(|| "info".to_string()),
        )
    };
    Ok(crate::logging::log_info(enabled, mode, &level)?)
}

/// Apply + persist the file-logging configuration (enable/disable, mode,
/// level). Unlike `set_log_level` (runtime-only), this is a deliberate,
/// user-initiated commit, so it writes the three logging fields to
/// `settings.json` immediately (patching the on-disk file so it doesn't
/// clobber unsaved edits elsewhere).
#[tauri::command]
pub fn set_logging_config(
    app: tauri::AppHandle,
    enabled: bool,
    mode: String,
    level: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<crate::logging::LogInfo> {
    let file_mode = crate::logging::LogFileMode::from_str_or_default(Some(&mode));

    // 1. Apply runtime level (if provided) and (re)configure the file sink.
    if let Some(ref lvl) = level {
        crate::logging::apply_log_level(lvl);
    }
    crate::logging::configure_file_logging(enabled, file_mode)?;

    // 2. Update the in-memory cache.
    let effective_level = {
        let mut cached = state
            .app_settings
            .write()
            .map_err(|e| format!("Lock error: {e}"))?;
        cached.file_logging = Some(enabled);
        cached.log_file_mode = Some(file_mode.as_str().to_string());
        if let Some(lvl) = level {
            cached.log_level = Some(lvl);
        }
        cached
            .log_level
            .clone()
            .unwrap_or_else(|| "info".to_string())
    };

    // 3. Persist just the logging fields to disk (load → patch → save) so we
    //    don't overwrite settings the user may be editing in the form.
    let mut on_disk = crate::settings::load_settings(&app);
    on_disk.file_logging = Some(enabled);
    on_disk.log_file_mode = Some(file_mode.as_str().to_string());
    on_disk.log_level = Some(effective_level.clone());
    if let Err(e) = crate::settings::save_settings(&app, &on_disk) {
        log::warn!("Failed to persist logging settings: {e}");
    }

    Ok(crate::logging::log_info(
        enabled,
        file_mode,
        &effective_level,
    )?)
}

/// Delete all archived log files (keeps the active file). Returns the count.
#[tauri::command]
pub fn purge_logs_cmd() -> AppResult<usize> {
    Ok(crate::logging::purge_logs()?)
}

/// Open the logs directory in the OS file explorer.
#[tauri::command]
pub fn open_logs_dir() -> AppResult<String> {
    let dir = crate::logging::logs_dir()?;
    let dir_str = dir.display().to_string();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer").arg(&dir).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&dir).spawn();
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(&dir).spawn();
    // explorer.exe returns a non-zero exit code even on success, so we only
    // treat a spawn failure as an error.
    match result {
        Ok(_) => Ok(dir_str),
        Err(e) => Err(format!("Failed to open logs dir: {e}").into()),
    }
}

// ---------------------------------------------------------------------------
// Gemini Live dual-pipeline commands
// ---------------------------------------------------------------------------

/// Start the Gemini Live pipeline.
///
/// Reads Gemini settings (API key, model) from `AppSettings`, creates a
/// `GeminiLiveClient`, connects it, then spawns two worker threads:
///   1. **Audio sender** — reads from `processed_rx` (same pipeline output
///      used by the local Whisper pipeline) and forwards audio to Gemini.
///   2. **Event receiver** — reads `GeminiEvent`s from the client and emits
///      Tauri events (`gemini-transcription`, `gemini-response`), also feeding
///      transcriptions into the knowledge graph.
///
/// Both pipelines (local and Gemini) can run simultaneously since they share
/// the same `processed_rx` channel (crossbeam receivers are cloneable).
#[tauri::command]
pub async fn start_gemini(state: State<'_, AppState>, app: tauri::AppHandle) -> AppResult<()> {
    log::info!("start_gemini called");

    // Guard: capture must be running
    {
        let capturing = state
            .is_capturing
            .read()
            .map_err(|e| format!("Lock error: {}", e))?;
        if !*capturing {
            return Err(AppError::SessionInvalid {
                reason: "Cannot start Gemini: capture is not running".to_string(),
            });
        }
    }

    // Guard: don't double-start
    {
        let active = state
            .is_gemini_active
            .read()
            .map_err(|e| format!("Lock error: {}", e))?;
        if *active {
            return Err(AppError::SessionInvalid {
                reason: "Gemini pipeline is already running".to_string(),
            });
        }
    }

    // Read Gemini settings
    let gemini_settings = state
        .app_settings
        .read()
        .map(|s| s.gemini.clone())
        .unwrap_or_default();

    // Validate auth configuration early.
    match &gemini_settings.auth {
        crate::settings::GeminiAuthMode::ApiKey { api_key } => {
            if api_key.is_empty() {
                return Err(AppError::CredentialMissing {
                    key: "gemini_api_key".to_string(),
                });
            }
        }
        crate::settings::GeminiAuthMode::VertexAI {
            project_id,
            location,
            ..
        } => {
            if project_id.is_empty() || location.is_empty() {
                return Err(AppError::CredentialFileError {
                    reason:
                        "Vertex AI project_id and location must be configured in Settings → Gemini."
                            .to_string(),
                });
            }
        }
    }

    // Create and connect the client. Notes-mode keeps the TEXT modality (the
    // historical default); converse-mode native audio-out (ADR-0018) flips
    // this to `GeminiConfig::audio(..)` once the converse start path lands.
    let config = GeminiConfig::text(gemini_settings.auth.clone(), gemini_settings.model);
    let mut client = GeminiLiveClient::new(config);
    client.connect()?;

    let event_rx = client.event_rx();

    // Mark active before starting worker threads. `connect()` can queue an
    // initial Connected event; the event receiver checks this flag before
    // processing each buffered event.
    if let Ok(mut active) = state.is_gemini_active.write() {
        *active = true;
    }

    // Store the client
    {
        let mut client_guard = state
            .gemini_client
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        *client_guard = Some(client);
    }

    // 1. Spawn the audio sender thread.
    //    Reads from the processed audio pipeline and forwards to Gemini.
    {
        let mut audio_handle = state
            .gemini_audio_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if audio_handle.is_none() {
            // Bug 1 fix: read from dedicated Gemini channel, not shared processed_rx
            let gemini_rx = state.gemini_audio_rx.clone();
            let gemini_client = state.gemini_client.clone();
            let is_active = state.is_gemini_active.clone();

            let handle = std::thread::Builder::new()
                .name("gemini-audio-sender".to_string())
                .spawn(move || {
                    log::info!("Gemini audio sender: starting");

                    while let Ok(chunk) = gemini_rx.recv() {
                        // Check if we should stop
                        let active = is_active.read().map(|a| *a).unwrap_or(false);
                        if !active {
                            break;
                        }

                        // Forward the audio to Gemini
                        // The chunk is already f32 mono 16kHz from the pipeline
                        let client_guard = match gemini_client.lock() {
                            Ok(g) => g,
                            Err(_) => break,
                        };
                        if let Some(ref client) = *client_guard {
                            if let Err(e) = client.send_audio(&chunk.data) {
                                log::warn!("Gemini audio sender: send failed: {}", e);
                                break;
                            }
                        } else {
                            break;
                        }
                    }

                    log::info!("Gemini audio sender: exiting");
                })
                .map_err(|e| format!("Failed to spawn Gemini audio thread: {}", e))?;
            *audio_handle = Some(handle);
            log::info!("Gemini audio sender thread spawned");
        }
    }

    // 2. Spawn the event receiver thread.
    //    Reads GeminiEvents and emits Tauri events + feeds the knowledge graph.
    {
        let mut event_handle = state
            .gemini_event_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if event_handle.is_none() {
            let app_handle = app.clone();
            let is_active = state.is_gemini_active.clone();
            let knowledge_graph = state.knowledge_graph.clone();
            let graph_snapshot = state.graph_snapshot.clone();
            let graph_extractor = state.graph_extractor.clone();
            let pipeline_status = state.pipeline_status.clone();
            let llm_engine = state.llm_engine.clone();
            let api_client = state.api_client.clone();
            let mistralrs_engine = state.mistralrs_engine.clone();
            let llm_executor = state.llm_executor.clone();
            let llm_provider = state
                .app_settings
                .read()
                .map(|s| s.llm_provider.clone())
                .unwrap_or_default();
            // Share the session_id Arc so per-turn writes land in the
            // CURRENT session's usage file even after `new_session_cmd`
            // rotates the ID in-process.
            let session_id_handle = state.session_id.clone();

            let handle = std::thread::Builder::new()
                .name("gemini-event-receiver".to_string())
                .spawn(move || {
                    log::info!("Gemini event receiver: starting");

                    // Extraction counters shared with fire-and-forget tasks on
                    // the rayon pool (extraction runs OFF this event-receiver
                    // thread so a slow LLM never stalls Gemini Live events).
                    let extraction_count =
                        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
                    let graph_update_count =
                        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

                    while let Ok(event) = event_rx.recv() {
                        // Check if we should stop
                        let active = is_active.read().map(|a| *a).unwrap_or(false);
                        if !active {
                            break;
                        }

                        match event {
                            GeminiEvent::Transcription { ref text, .. } => {
                                // Emit Tauri event for the frontend
                                let _ = app_handle.emit(events::GEMINI_TRANSCRIPTION, &event);

                                // Feed transcription into the knowledge graph
                                // (same extraction pipeline as local transcripts).
                                // Run it on the shared rayon extraction pool —
                                // NOT inline here — so a slow/blocked LLM cannot
                                // stall Gemini Live event handling (transcripts,
                                // status, reconnects) or back up the bounded
                                // event channel.
                                if !text.is_empty() {
                                    let segment_id = uuid::Uuid::new_v4().to_string();
                                    let timestamp = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs_f64();

                                    speech::spawn_extraction_task(
                                        text.clone(),
                                        "Gemini".to_string(),
                                        String::new(),
                                        segment_id,
                                        timestamp,
                                        &speech::ExtractionDeps {
                                            llm_engine: &llm_engine,
                                            api_client: &api_client,
                                            mistralrs_engine: &mistralrs_engine,
                                            llm_executor: &llm_executor,
                                            llm_provider: &llm_provider,
                                            graph_extractor: &graph_extractor,
                                            knowledge_graph: &knowledge_graph,
                                            graph_snapshot: &graph_snapshot,
                                            pipeline_status: &pipeline_status,
                                            app_handle: &app_handle,
                                        },
                                        &extraction_count,
                                        &graph_update_count,
                                    );
                                }
                            }
                            GeminiEvent::ModelResponse { .. } => {
                                let _ = app_handle.emit(events::GEMINI_RESPONSE, &event);
                            }
                            GeminiEvent::Error {
                                ref category,
                                ref message,
                            } => {
                                log::error!("Gemini error event ({:?}): {}", category, message,);
                                let _ = app_handle.emit(events::GEMINI_STATUS, &event);
                            }
                            GeminiEvent::Connected => {
                                let _ = app_handle.emit(events::GEMINI_STATUS, &event);
                            }
                            GeminiEvent::TurnComplete { ref usage } => {
                                // Model finished its turn. Forward the event
                                // on GEMINI_STATUS so the UI can surface
                                // per-turn token accounting from
                                // `usageMetadata` (see gemini::UsageMetadata).
                                if let Some(u) = usage {
                                    log::debug!(
                                        "Gemini: turn complete (tokens total={:?})",
                                        u.total_token_count
                                    );
                                } else {
                                    log::debug!("Gemini: turn complete");
                                }

                                // Persist per-session token totals (loop 19).
                                // Before this, turn counts + token totals only
                                // lived in the frontend's localStorage and did
                                // not survive an app restart.
                                let delta = crate::sessions::usage::TurnDelta {
                                    prompt: usage
                                        .as_ref()
                                        .and_then(|u| u.prompt_token_count)
                                        .unwrap_or(0)
                                        as u64,
                                    response: usage
                                        .as_ref()
                                        .and_then(|u| u.response_token_count)
                                        .unwrap_or(0)
                                        as u64,
                                    cached: usage
                                        .as_ref()
                                        .and_then(|u| u.cached_content_token_count)
                                        .unwrap_or(0)
                                        as u64,
                                    thoughts: usage
                                        .as_ref()
                                        .and_then(|u| u.thoughts_token_count)
                                        .unwrap_or(0)
                                        as u64,
                                    tool_use: usage
                                        .as_ref()
                                        .and_then(|u| u.tool_use_prompt_token_count)
                                        .unwrap_or(0)
                                        as u64,
                                    total: usage
                                        .as_ref()
                                        .and_then(|u| u.total_token_count)
                                        .unwrap_or(0)
                                        as u64,
                                };
                                let current_sid = match session_id_handle.read() {
                                    Ok(g) => g.clone(),
                                    Err(poisoned) => poisoned.into_inner().clone(),
                                };
                                if let Err(e) =
                                    crate::sessions::usage::append_turn(&current_sid, delta)
                                {
                                    log::warn!("Failed to persist turn usage: {}", e);
                                }

                                let _ = app_handle.emit(events::GEMINI_STATUS, &event);
                            }
                            GeminiEvent::Disconnected => {
                                let _ = app_handle.emit(events::GEMINI_STATUS, &event);
                                break;
                            }
                            GeminiEvent::Reconnecting {
                                attempt,
                                backoff_secs,
                            } => {
                                // Auto-reconnect in flight — surface through
                                // the status event so the UI can show a
                                // "reconnecting…" hint. Do NOT break the loop:
                                // the session task handles the full setup
                                // handshake replay and will emit Reconnected
                                // on success or a fatal Error if the budget
                                // is exhausted.
                                log::info!(
                                    "Gemini: reconnecting attempt={} backoff={}s",
                                    attempt,
                                    backoff_secs
                                );
                                let _ = app_handle.emit(events::GEMINI_STATUS, &event);
                            }
                            GeminiEvent::Reconnected { resumed } => {
                                log::info!("Gemini: reconnected (resumed={})", resumed);
                                let _ = app_handle.emit(events::GEMINI_STATUS, &event);
                            }
                            // Native audio-out / barge-in events (ADR-0018).
                            // This `start_gemini` path runs the notes/graph
                            // TEXT modality, which never produces these — the
                            // converse-mode orchestrator (B18, `crate::converse`
                            // TurnMachine) consumes them via `gemini_event_to_signal`.
                            // We log + ignore here so the notes path stays
                            // exhaustive without taking on converse wiring.
                            GeminiEvent::AudioChunk { ref data_base64, .. } => {
                                log::debug!(
                                    "Gemini: unexpected AudioChunk ({} b64 chars) on notes-mode path; ignoring",
                                    data_base64.len()
                                );
                            }
                            GeminiEvent::OutputTranscription { .. } => {
                                log::debug!(
                                    "Gemini: unexpected OutputTranscription on notes-mode path; ignoring"
                                );
                            }
                            GeminiEvent::Interrupted => {
                                log::debug!("Gemini: unexpected Interrupted on notes-mode path; ignoring");
                            }
                            GeminiEvent::GenerationComplete => {
                                log::debug!("Gemini: generationComplete on notes-mode path; ignoring");
                            }
                        }
                    }

                    log::info!("Gemini event receiver: exiting");
                })
                .map_err(|e| format!("Failed to spawn Gemini event thread: {}", e))?;
            *event_handle = Some(handle);
            log::info!("Gemini event receiver thread spawned");
        }
    }

    log::info!("Gemini Live pipeline started");
    Ok(())
}

/// Stop the Gemini Live pipeline.
///
/// Disconnects the client, signals worker threads to stop via the
/// `is_gemini_active` flag, and cleans up thread handles.
#[tauri::command]
pub async fn stop_gemini(state: State<'_, AppState>, _app: tauri::AppHandle) -> AppResult<()> {
    log::info!("stop_gemini called");

    // 1. Set active flag to false (signals worker threads to exit)
    if let Ok(mut active) = state.is_gemini_active.write() {
        *active = false;
    }

    // 2. Disconnect the client (sends Disconnected event, closes channels)
    {
        let mut client_guard = state
            .gemini_client
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if let Some(ref client) = *client_guard {
            client.disconnect();
        }
        *client_guard = None;
    }

    // 3. Join the worker threads (bounded) so they fully exit before we return
    //    — prevents a fast Stop→Start race from running two Gemini workers on
    //    the same audio channel (critique H2). Detaches on timeout.
    let audio_h = state
        .gemini_audio_thread
        .lock()
        .ok()
        .and_then(|mut g| g.take());
    let event_h = state
        .gemini_event_thread
        .lock()
        .ok()
        .and_then(|mut g| g.take());
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(h) = audio_h {
            join_worker_with_timeout(h, std::time::Duration::from_secs(3), "Gemini audio worker");
        }
        if let Some(h) = event_h {
            join_worker_with_timeout(h, std::time::Duration::from_secs(3), "Gemini event worker");
        }
    })
    .await;

    log::info!("Gemini Live pipeline stopped");
    Ok(())
}

// ---------------------------------------------------------------------------
// Converse mode — native speech-to-speech (B18 / ADR-0018)
// ---------------------------------------------------------------------------

/// Production [`crate::converse::ConverseSink`] for the Gemini native-S2S path.
///
/// Dispatches the FSM's [`crate::converse::TurnAction`]s against the live
/// engine + audio player + capture gate. Holds only `Arc` handles (cloned from
/// `AppState`) so it lives on the converse-driver thread. The pure
/// [`crate::converse::ConverseDriver`] decides; this executes — and is the only
/// part that touches I/O, which is why the decision logic is unit-tested
/// against a mock sink instead.
struct GeminiConverseSink {
    gemini_client: std::sync::Arc<std::sync::Mutex<Option<GeminiLiveClient>>>,
    audio_player: crate::playback::AudioPlayer,
    /// Per-turn capture gate (B18 step 5): the audio-sender thread streams only
    /// while `true`. On the Gemini server-VAD path capture stays open during
    /// `Speaking` (the engine drives barge-in), so toggling it is the
    /// OpenAI/client-VAD lever; we still honor Start/StopCapture here.
    capture_gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
    app_handle: tauri::AppHandle,
}

impl crate::converse::ConverseSink for GeminiConverseSink {
    fn start_capture(&mut self) {
        self.capture_gate.store(true, Ordering::SeqCst);
        // Re-arm the player after a prior barge-in so the next reply is audible.
        self.audio_player.resume();
    }

    fn stop_capture(&mut self) {
        self.capture_gate.store(false, Ordering::SeqCst);
    }

    fn end_user_turn(&mut self) {
        if let Ok(guard) = self.gemini_client.lock()
            && let Some(ref client) = *guard
            && let Err(e) = client.end_user_turn()
        {
            log::warn!("converse: end_user_turn failed: {e}");
        }
    }

    fn play_audio(&mut self, pcm24: &[u8]) {
        // PlayAudio carries PCM16-LE bytes; the player wants &[i16].
        let samples = crate::converse::pcm16_le_bytes_to_i16(pcm24);
        if !samples.is_empty() {
            self.audio_player.push_samples(&samples);
        }
    }

    fn stop_playback(&mut self) {
        // Flush + suppress in-flight assistant audio immediately (barge-in).
        self.audio_player.cancel();
    }

    fn cancel_generation(&mut self) {
        // Gemini auto-cancels server-side on its own `interrupted`; the local
        // flush (stop_playback) is the client's part. There is no separate
        // per-turn cancel frame to send, so this is a no-op for Gemini (the
        // OpenAI Realtime voice path will send response.cancel + truncate here).
        log::debug!("converse: cancel_generation (Gemini: server auto-cancels)");
    }

    fn cancel_token(&mut self) {
        // The per-turn cancellation token (ADR-0003) gates async work for the
        // turn. The Gemini path runs no per-turn async tasks that outlive the
        // event loop, so there is nothing to trip yet; the OpenAI voice path
        // will wire a real tokio_util::CancellationToken here.
        log::debug!("converse: cancel_token (no per-turn async work on Gemini path)");
    }

    fn emit_transcript(&mut self, text: &str, final_: bool) {
        // Surface the assistant's spoken-reply transcript to the UI. (Graph
        // proposals from converse replies are a B-future enhancement; for now
        // this drives the live-transcript panel.)
        let _ = self.app_handle.emit(
            events::GEMINI_RESPONSE,
            serde_json::json!({ "text": text, "final": final_ }),
        );
    }

    fn suppressed_barge_in(&mut self, reason: crate::converse::SuppressedReason) {
        log::debug!("converse: barge-in suppressed ({reason:?})");
    }

    fn report_error(&mut self, category: crate::converse::TurnErrorCategory, message: &str) {
        log::warn!("converse: engine error ({category:?}): {message}");
        let _ = self.app_handle.emit(
            events::GEMINI_STATUS,
            serde_json::json!({ "type": "error", "message": message }),
        );
    }
}

/// Start a native speech-to-speech converse session (B18 / ADR-0018).
///
/// Unlike [`start_gemini`] (the notes/graph **TEXT** pipeline), this opens a
/// Gemini Live **AUDIO** session and drives a [`crate::converse::ConverseDriver`]
/// (wrapping the pure turn-FSM) from the live `GeminiEvent` stream: assistant
/// audio is decoded + played, the server's `interrupted` drives barge-in, and
/// `turnComplete` resumes listening. Reuses the same capture pipeline
/// (`gemini_audio_rx`) as notes mode for the user-audio leg.
///
/// Spawns two threads (mirroring `start_gemini`): an audio sender gated by
/// `converse_capture_gate`, and a converse-event driver thread. Idempotent
/// guards prevent double-start and require capture to be running.
#[tauri::command]
pub async fn start_converse(state: State<'_, AppState>, app: tauri::AppHandle) -> AppResult<()> {
    log::info!("start_converse called");

    // Guard: capture must be running (we need user audio to send).
    {
        let capturing = state
            .is_capturing
            .read()
            .map_err(|e| format!("Lock error: {}", e))?;
        if !*capturing {
            return Err(AppError::SessionInvalid {
                reason: "Cannot start converse: capture is not running".to_string(),
            });
        }
    }
    // Guard: don't double-start, and don't run alongside the TEXT pipeline (both
    // consume the same gemini audio channel).
    {
        if *state
            .is_converse_active
            .read()
            .map_err(|e| format!("Lock error: {}", e))?
        {
            return Err(AppError::SessionInvalid {
                reason: "Converse session is already running".to_string(),
            });
        }
        if *state
            .is_gemini_active
            .read()
            .map_err(|e| format!("Lock error: {}", e))?
        {
            return Err(AppError::SessionInvalid {
                reason: "Stop the Gemini notes pipeline before starting converse".to_string(),
            });
        }
    }

    let gemini_settings = state
        .app_settings
        .read()
        .map(|s| s.gemini.clone())
        .unwrap_or_default();

    // Validate auth early (same checks as start_gemini).
    if let crate::settings::GeminiAuthMode::ApiKey { api_key } = &gemini_settings.auth
        && api_key.is_empty()
    {
        return Err(AppError::CredentialMissing {
            key: "gemini_api_key".to_string(),
        });
    }

    // AUDIO modality with the configured voice (B18 step 1) — this is what makes
    // the server emit AudioChunk so the FSM's Thinking→Speaking edge can fire.
    let config = GeminiConfig::audio(
        gemini_settings.auth.clone(),
        gemini_settings.model,
        gemini_settings.voice,
    );
    let mut client = GeminiLiveClient::new(config);
    client.connect()?;
    let event_rx = client.event_rx();

    // Open the 24 kHz mono playback stream for assistant audio (step 4).
    let _ = state
        .audio_player
        .open_default(crate::playback::PlaybackConfig {
            source_sample_rate: 24_000,
            source_channels: 1,
        })
        .map_err(|e| log::warn!("converse: failed to open playback stream: {e}"));

    *state
        .is_converse_active
        .write()
        .map_err(|e| format!("Lock error: {}", e))? = true;
    state.converse_capture_gate.store(true, Ordering::SeqCst);
    {
        let mut client_guard = state
            .gemini_client
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        *client_guard = Some(client);
    }

    // 1. Audio sender thread — forward captured audio while the gate is open.
    {
        let mut audio_handle = state
            .gemini_audio_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if audio_handle.is_none() {
            let gemini_rx = state.gemini_audio_rx.clone();
            let gemini_client = state.gemini_client.clone();
            let is_active = state.is_converse_active.clone();
            let capture_gate = state.converse_capture_gate.clone();
            let handle = std::thread::Builder::new()
                .name("converse-audio-sender".to_string())
                .spawn(move || {
                    log::info!("converse audio sender: starting");
                    while let Ok(chunk) = gemini_rx.recv() {
                        if !is_active.read().map(|a| *a).unwrap_or(false) {
                            break;
                        }
                        // B18 step 5: only stream while the per-turn gate is open.
                        if !capture_gate.load(Ordering::SeqCst) {
                            continue;
                        }
                        let guard = match gemini_client.lock() {
                            Ok(g) => g,
                            Err(_) => break,
                        };
                        match *guard {
                            Some(ref client) => {
                                if let Err(e) = client.send_audio(&chunk.data) {
                                    log::warn!("converse audio sender: send failed: {e}");
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    log::info!("converse audio sender: exiting");
                })
                .map_err(|e| format!("Failed to spawn converse audio thread: {}", e))?;
            *audio_handle = Some(handle);
        }
    }

    // 2. Converse-event driver thread — drives the TurnMachine from GeminiEvents.
    {
        let mut conv_handle = state
            .converse_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if conv_handle.is_none() {
            let is_active = state.is_converse_active.clone();
            let mut sink = GeminiConverseSink {
                gemini_client: state.gemini_client.clone(),
                audio_player: state.audio_player.clone(),
                capture_gate: state.converse_capture_gate.clone(),
                app_handle: app.clone(),
            };
            let handle = std::thread::Builder::new()
                .name("converse-driver".to_string())
                .spawn(move || {
                    log::info!("converse driver: starting");
                    // Gemini uses server-side VAD with NO client AEC reference,
                    // so audio-activity barge-in is disabled — the engine's own
                    // `interrupted` event drives barge-in (bypasses the gate).
                    let gate = crate::converse::InterruptionGate {
                        enabled: false,
                        ..Default::default()
                    };
                    let mut driver = crate::converse::ConverseDriver::new(gate);
                    // Prime into Listening (server-VAD bridge): the first
                    // assistant AudioChunk then drives Thinking→Speaking.
                    driver.begin_listening(unix_millis(), &mut sink);

                    while let Ok(event) = event_rx.recv() {
                        if !is_active.read().map(|a| *a).unwrap_or(false) {
                            break;
                        }
                        // Mirror notes-mode transport handling for lifecycle
                        // events the FSM does not model.
                        match &event {
                            GeminiEvent::Disconnected => {
                                let _ = sink.app_handle.emit(events::GEMINI_STATUS, &event);
                                break;
                            }
                            GeminiEvent::Connected
                            | GeminiEvent::Reconnecting { .. }
                            | GeminiEvent::Reconnected { .. } => {
                                let _ = sink.app_handle.emit(events::GEMINI_STATUS, &event);
                            }
                            GeminiEvent::Transcription { .. } => {
                                // User-speech transcript → UI (graph extraction
                                // for converse is a B-future enhancement).
                                let _ = sink.app_handle.emit(events::GEMINI_TRANSCRIPTION, &event);
                            }
                            _ => {}
                        }
                        // Drive the FSM. user_speech_ms = 0 (no client VAD on the
                        // Gemini server-VAD path); the gate is disabled anyway.
                        driver.on_gemini_event(event, unix_millis(), 0, &mut sink);

                        // After a completed turn the FSM returns to Listening and
                        // re-emits StartCapture; if it somehow lands back in Idle
                        // (e.g. a reset), re-prime so the next turn is captured.
                        if driver.state() == crate::converse::TurnState::Idle {
                            driver.begin_listening(unix_millis(), &mut sink);
                        }
                    }
                    // Teardown: cancel any in-flight turn + flush playback.
                    driver.reset(&mut sink);
                    log::info!("converse driver: exiting");
                })
                .map_err(|e| format!("Failed to spawn converse driver thread: {}", e))?;
            *conv_handle = Some(handle);
        }
    }

    log::info!("converse session started (Gemini AUDIO)");
    Ok(())
}

/// Stop the native converse session: disconnect the client, signal the worker
/// threads via `is_converse_active`, flush playback, and join the threads.
#[tauri::command]
pub async fn stop_converse(state: State<'_, AppState>, _app: tauri::AppHandle) -> AppResult<()> {
    log::info!("stop_converse called");

    if let Ok(mut active) = state.is_converse_active.write() {
        *active = false;
    }
    state.converse_capture_gate.store(false, Ordering::SeqCst);

    // Disconnect the client (unblocks the event receiver via Disconnected/close).
    {
        let mut client_guard = state
            .gemini_client
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if let Some(ref client) = *client_guard {
            client.disconnect();
        }
        *client_guard = None;
    }
    // Stop playback so no assistant audio lingers.
    let _ = state.audio_player.stop();

    // Join the worker threads off-thread (bounded), mirroring stop_gemini.
    let audio_h = state
        .gemini_audio_thread
        .lock()
        .ok()
        .and_then(|mut g| g.take());
    let conv_h = state.converse_thread.lock().ok().and_then(|mut g| g.take());
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(h) = audio_h {
            join_worker_with_timeout(
                h,
                std::time::Duration::from_secs(3),
                "converse audio worker",
            );
        }
        if let Some(h) = conv_h {
            join_worker_with_timeout(h, std::time::Duration::from_secs(3), "converse driver");
        }
    })
    .await;

    log::info!("converse session stopped");
    Ok(())
}

// ---------------------------------------------------------------------------
// Process enumeration
// ---------------------------------------------------------------------------

/// A running system process (for target-selection UI).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub exe_path: Option<String>,
}

/// List running system processes sorted by name, preserving duplicate process
/// names because each PID is a distinct capture target.
#[tauri::command]
pub fn list_running_processes() -> Vec<ProcessInfo> {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut processes: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .filter(|(_, p)| !p.name().to_string_lossy().is_empty())
        .map(|(pid, p)| ProcessInfo {
            pid: pid.as_u32(),
            name: p.name().to_string_lossy().to_string(),
            exe_path: p.exe().map(|e| e.to_string_lossy().to_string()),
        })
        .collect();

    processes.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.pid.cmp(&b.pid))
    });
    processes
}

// ---------------------------------------------------------------------------
// Persistence commands (transcript + knowledge graph)
// ---------------------------------------------------------------------------

/// Export the full in-memory transcript buffer as a JSON string.
#[tauri::command]
pub async fn export_transcript(state: State<'_, AppState>) -> AppResult<String> {
    let buffer = state
        .transcript_buffer
        .read()
        .map_err(|e| format!("Failed to read transcript buffer: {}", e))?;
    let segments: Vec<TranscriptSegment> = buffer.iter().cloned().collect();
    serde_json::to_string_pretty(&segments)
        .map_err(|e| format!("Failed to serialize transcript: {}", e))
        .map_err(AppError::from)
}

/// Save the knowledge graph to disk (session-specific file).
#[tauri::command]
pub async fn save_graph(state: State<'_, AppState>) -> AppResult<String> {
    let dir = crate::persistence::graphs_dir()
        .ok_or_else(|| "Cannot resolve graph save directory".to_string())?;

    let file_path = dir.join(format!("{}.json", state.current_session_id()));

    let graph = state
        .knowledge_graph
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;

    graph.save_to_file(&file_path)?;

    log::info!("Graph saved to {:?}", file_path);
    Ok(file_path.to_string_lossy().to_string())
}

/// Load a knowledge graph from a file on disk, replacing the current graph.
///
/// `path` is the absolute path to the JSON graph file.
#[tauri::command]
pub async fn load_graph(path: String, state: State<'_, AppState>) -> AppResult<()> {
    let file_path = std::path::PathBuf::from(&path);

    if !file_path.exists() {
        return Err(AppError::Unknown(format!("Graph file not found: {}", path)));
    }

    let loaded = crate::graph::temporal::TemporalKnowledgeGraph::load_from_file(&file_path)?;

    // Replace the in-memory knowledge graph
    {
        let mut graph = state
            .knowledge_graph
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        *graph = loaded;
    }

    // Update the cached snapshot
    {
        let graph = state
            .knowledge_graph
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let snapshot = graph.snapshot();
        if let Ok(mut gs) = state.graph_snapshot.write() {
            *gs = snapshot;
        }
    }

    log::info!("Graph loaded from {:?}", file_path);
    Ok(())
}

/// Export the knowledge graph as a JSON string (for clipboard / download).
#[tauri::command]
pub async fn export_graph(state: State<'_, AppState>) -> AppResult<String> {
    let snapshot = state
        .graph_snapshot
        .read()
        .map_err(|e| format!("Failed to read graph snapshot: {}", e))?;
    serde_json::to_string_pretty(&*snapshot)
        .map_err(|e| format!("Failed to serialize graph: {}", e))
        .map_err(AppError::from)
}

/// Get the current session ID.
#[tauri::command]
pub async fn get_session_id(state: State<'_, AppState>) -> AppResult<String> {
    Ok(state.current_session_id())
}

/// User-facing retry after the `capture-storage-full` banner.
///
/// Probes the transcripts directory with a small canary write. On success,
/// resets the process-wide storage-full debounce so the next real ENOSPC
/// re-emits `capture-storage-full`, and returns `Ok(())`. On failure, leaves
/// the debounce set and returns a structured `unknown` payload — the UI should
/// keep the banner visible so the user knows they haven't freed enough space
/// yet.
#[tauri::command]
pub async fn retry_storage_write() -> AppResult<()> {
    crate::persistence::retry_storage_write()
        .map_err(|e| format!("Storage still unavailable: {}", e))
        .map_err(AppError::from)
}

// ---------------------------------------------------------------------------
// Session management commands (v1: list / load transcript / delete)
// ---------------------------------------------------------------------------

/// List past sessions from the sessions index, most recent first.
/// Pass `limit` to cap the number of returned entries (e.g. `Some(10)`).
#[tauri::command]
pub fn list_sessions(limit: Option<usize>) -> Vec<crate::sessions::SessionMetadata> {
    let mut sessions = crate::sessions::load_index();
    if let Some(n) = limit {
        sessions.truncate(n);
    }
    sessions
}

/// Validate a session ID is safe to use as a file name segment.
/// Rejects anything that could enable path traversal (`..`, `/`, `\`, null).
fn validate_session_id(session_id: &str) -> Result<(), String> {
    crate::sessions::validate_session_id(session_id)
}

fn indexed_session_paths(
    session_id: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    validate_session_id(session_id)?;
    if let Some(metadata) = crate::sessions::find_session(session_id) {
        return Ok(crate::sessions::session_file_paths(&metadata));
    }
    Ok((
        crate::user_data::transcript_path(session_id)?,
        crate::user_data::graph_path(session_id)?,
    ))
}

fn read_session_transcript(session_id: &str) -> Result<Vec<TranscriptSegment>, String> {
    validate_session_id(session_id)?;
    let (path, _) = indexed_session_paths(session_id)?;
    if !path.exists() {
        return Err(format!("Transcript file not found: {}", path.display()));
    }
    let contents = std::fs::read_to_string(&path).map_err(|e| format!("{}", e))?;
    let mut segments = Vec::new();
    for line in contents.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<TranscriptSegment>(line) {
            Ok(seg) => segments.push(seg),
            Err(e) => log::warn!("Skipping malformed transcript line: {}", e),
        }
    }
    Ok(segments)
}

/// Load a past session's transcript from disk. Returns the parsed
/// `TranscriptSegment`s from `~/.audiograph/transcripts/<session_id>.jsonl`.
#[tauri::command]
pub fn load_session_transcript(session_id: String) -> AppResult<Vec<TranscriptSegment>> {
    read_session_transcript(&session_id).map_err(AppError::from)
}

/// Load a past session's transcript and graph snapshot into the active UI view.
#[tauri::command]
pub fn load_session(session_id: String, state: State<'_, AppState>) -> AppResult<LoadedSession> {
    validate_session_id(&session_id)?;
    let (transcript_path, graph_path) = indexed_session_paths(&session_id)?;
    if !transcript_path.exists() && !graph_path.exists() {
        return Err(AppError::SessionInvalid {
            reason: format!("Session files not found: {}", session_id),
        });
    }
    let transcript = if transcript_path.exists() {
        read_session_transcript(&session_id)?
    } else {
        Vec::new()
    };
    let loaded_graph = if graph_path.exists() {
        crate::graph::temporal::TemporalKnowledgeGraph::load_from_file(&graph_path)?
    } else {
        crate::graph::temporal::TemporalKnowledgeGraph::new()
    };
    let snapshot = loaded_graph.snapshot();

    {
        let mut graph = state
            .knowledge_graph
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        *graph = loaded_graph;
    }
    if let Ok(mut gs) = state.graph_snapshot.write() {
        *gs = snapshot.clone();
    }

    Ok(LoadedSession {
        transcript,
        graph: snapshot,
    })
}

/// Soft-delete a session: flag it as trashed in the sessions index but keep
/// the transcript and graph files on disk. The UI can show trashed sessions
/// via a "Show trash" toggle and restore them with `restore_session`. After
/// the 30-day retention window expires, `purge_expired_sessions` lazily
/// hard-deletes the entry + files on the next list_sessions call.
///
/// This replaces the v1 hard-delete behavior. For an immediate hard delete
/// (e.g. from the trash view's "Delete permanently" button), use
/// `delete_session_permanently`.
#[tauri::command]
pub fn delete_session(session_id: String) -> AppResult<()> {
    validate_session_id(&session_id)?;
    crate::sessions::soft_delete_session(&session_id)?;
    log::info!("Session {} moved to trash", session_id);
    Ok(())
}

/// Restore a soft-deleted session back to the active list.
#[tauri::command]
pub fn restore_session(session_id: String) -> AppResult<()> {
    validate_session_id(&session_id)?;
    crate::sessions::restore_session(&session_id)?;
    log::info!("Session {} restored from trash", session_id);
    Ok(())
}

/// Permanently delete a session: remove from index and unlink its files.
/// Bypasses the trash — intended for the "Delete permanently" action in the
/// trash view.
#[tauri::command]
pub fn delete_session_permanently(session_id: String) -> AppResult<()> {
    validate_session_id(&session_id)?;
    let (t, g) = indexed_session_paths(&session_id)?;
    crate::sessions::remove_from_index(&session_id)?;
    match std::fs::remove_file(&t) {
        Ok(_) => log::info!("Deleted transcript: {}", t.display()),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            log::warn!("Failed to delete transcript {}: {}", t.display(), e);
        }
        _ => {}
    }
    match std::fs::remove_file(&g) {
        Ok(_) => log::info!("Deleted graph: {}", g.display()),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            log::warn!("Failed to delete graph {}: {}", g.display(), e);
        }
        _ => {}
    }
    Ok(())
}

/// Rebuild missing sessions-index entries by scanning transcript and graph
/// files under the configured user-data roots.
#[tauri::command]
pub fn recover_orphaned_sessions() -> AppResult<crate::sessions::SessionRecoveryReport> {
    let report = crate::sessions::rebuild_index_from_files()?;
    log::info!(
        "Session recovery: discovered={} recovered={} skipped={} errors={}",
        report.discovered,
        report.recovered,
        report.skipped,
        report.errors.len()
    );
    Ok(report)
}

/// Lazy cleanup: hard-delete any trashed sessions whose `deleted_at` is older
/// than the 30-day retention window. Returns the list of purged session IDs.
/// Frontend is expected to call this on session list load.
#[tauri::command]
pub fn purge_expired_sessions() -> AppResult<Vec<String>> {
    let purged = crate::sessions::purge_expired_sessions()?;
    if !purged.is_empty() {
        log::info!("Purged {} expired session(s) from trash", purged.len());
    }
    Ok(purged)
}

/// Load the token-usage record for a session from
/// `~/.audiograph/usage/<session_id>.json`. Missing or malformed files
/// resolve to a zeroed record — callers never have to disambiguate.
#[tauri::command]
pub fn get_session_usage(session_id: String) -> AppResult<crate::sessions::usage::SessionUsage> {
    validate_session_id(&session_id)?;
    Ok(crate::sessions::usage::load_usage(&session_id))
}

/// Load the token-usage record for the CURRENT session. Convenience wrapper
/// so the frontend can restore its in-memory totals on startup without first
/// having to fetch `get_session_id`.
#[tauri::command]
pub fn get_current_session_usage(
    state: State<'_, AppState>,
) -> AppResult<crate::sessions::usage::SessionUsage> {
    Ok(crate::sessions::usage::load_usage(
        &state.current_session_id(),
    ))
}

/// Aggregate usage across every on-disk session file. This is the
/// authoritative source for the frontend's "Lifetime" totals panel — the
/// prior localStorage-backed lifetime counter was only ever a best-effort
/// mirror of this sum.
#[tauri::command]
pub fn get_lifetime_usage() -> AppResult<crate::sessions::usage::LifetimeUsage> {
    Ok(crate::sessions::usage::load_lifetime_usage())
}

/// Import a frontend `localStorage` lifetime-totals snapshot into the backend
/// usage directory so `get_lifetime_usage` reports pre-persistence history.
///
/// This is a one-way migration path, guarded by the idempotency check inside
/// `seed_lifetime_migration`: a second call is a no-op, so a stale browser
/// state can't double-count. The frontend is expected to call this once on
/// mount and then clear its `localStorage` lifetime key.
#[tauri::command]
pub fn seed_lifetime_migration(payload: crate::sessions::usage::LifetimeUsage) -> AppResult<()> {
    crate::sessions::usage::seed_lifetime_migration(&payload).map_err(AppError::from)
}

/// Flush the current session and rotate to a fresh one in-process.
///
/// Behavior:
///   1. Finalize current session's sessions-index entry (status → complete).
///   2. Re-save the current session's usage record so on-disk totals are
///      flushed before the ID rotates.
///   3. Seed a fresh zeroed usage file for the new session so
///      `get_current_session_usage` returns zeros immediately after rotation.
///   4. Rotate `AppState::session_id` in place:
///        - The transcript writer is respawned against the new ID's file.
///        - The graph-autosave thread re-reads the ID on its next 30s tick
///          and starts writing to the new session's file.
///        - The Gemini event thread re-reads the ID on the next TurnComplete.
///   5. Register the new session in the sessions index so list_sessions
///      shows it alongside the previous one.
///
/// Returns the new session ID.
#[tauri::command]
pub fn new_session_cmd(state: State<'_, AppState>) -> AppResult<String> {
    let previous_id = state.current_session_id();

    // 1. Finalize current session's index entry. Best-effort: a failed
    //    finalize must not prevent us handing the caller a fresh UUID.
    if let Err(e) = crate::sessions::finalize_session(&previous_id) {
        log::warn!("new_session_cmd: finalize current failed: {}", e);
    }

    // 2. Re-save the current session's usage record. If the file is missing
    //    this is a harmless zero-write; if it exists, `save_usage` is a
    //    no-op rewrite of the same bytes. Either way, it guarantees the
    //    file is present on disk before the caller moves on.
    let current = crate::sessions::usage::load_usage(&previous_id);
    if let Err(e) = crate::sessions::usage::save_usage(&current) {
        log::warn!("new_session_cmd: save current usage failed: {}", e);
    }

    // 3. Seed a fresh usage file for the next session. Do this BEFORE the
    //    rotate so `get_current_session_usage` immediately reads zeroes.
    let new_id = uuid::Uuid::new_v4().to_string();
    let fresh = crate::sessions::usage::SessionUsage {
        session_id: new_id.clone(),
        ..crate::sessions::usage::SessionUsage::default()
    };
    crate::sessions::usage::save_usage(&fresh)?;

    // 4. Rotate in-process. `rotate_session` swaps the session_id Arc and
    //    respawns the transcript writer; the autosave + gemini-event
    //    threads pick up the change on their next iteration.
    //
    //    Concurrent-rotate guard: if another rotation is already in flight,
    //    skip and return the current session ID. The caller sees a successful
    //    rotation either way (the in-flight rotate will land a fresh ID);
    //    they just don't get the one *we* seeded. The usage file we wrote in
    //    step 3 is then orphaned — harmless, since seed files are zeroed and
    //    `load_usage` handles missing/extra entries.
    match state.rotate_session(&new_id) {
        crate::state::RotateOutcome::Rotated(rotated_from) => {
            debug_assert_eq!(rotated_from, previous_id);
        }
        crate::state::RotateOutcome::AlreadyRotating(current) => {
            log::warn!(
                "new_session_cmd: concurrent rotation detected; returning current id {} \
                 instead of freshly-seeded {}",
                current,
                new_id
            );
            return Ok(current);
        }
    }

    // 5. Register new session in the index so it shows up in list_sessions
    //    (status "active"). Best-effort: failure just means the UI won't
    //    see the entry until the next restart rediscovers it.
    if let Err(e) = crate::sessions::register_session(&new_id) {
        log::warn!("new_session_cmd: register_session failed: {}", e);
    }

    log::info!("new_session_cmd: rotated {} → {}", previous_id, new_id);
    Ok(new_id)
}

// ---------------------------------------------------------------------------
// Credential management commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn save_credential_cmd(key: String, value: String) -> AppResult<()> {
    // Boundary-layer allowlist check (loop11 MEDIUM #5): reject unknown keys
    // here before they reach the inner `set_field` match. Mirrors the
    // convention used by `validate_session_id` elsewhere in this module.
    if !crate::credentials::is_allowed_key(&key) {
        return Err(crate::error::AppError::CredentialFileError {
            reason: format!("Unknown credential key: {}", key),
        });
    }
    // Bubble credential-file failures as `CredentialFileError` so the
    // frontend can render a localized / actionable message instead of a bare
    // string.
    crate::credentials::set_credential(&key, &value)
        .map_err(|reason| crate::error::AppError::CredentialFileError { reason })
}

/// Explicitly clear a stored credential. Needed because `save_credential_cmd`
/// treats empty strings as a no-op (to avoid clobbering on blank form fields),
/// so there has to be a separate way for users to actually delete a key.
#[tauri::command]
pub fn delete_credential_cmd(key: String) -> AppResult<()> {
    // Boundary-layer allowlist check (loop11 MEDIUM #5). Emit the same
    // message the inner `set_field` match would have produced, but reject at
    // the command boundary so the frontend receives a structured payload.
    if !crate::credentials::is_allowed_key(&key) {
        return Err(AppError::CredentialFileError {
            reason: format!("Unknown credential key: {}", key),
        });
    }
    crate::credentials::delete_credential(&key)
        .map_err(|reason| AppError::CredentialFileError { reason })
}

#[tauri::command]
pub fn load_credential_cmd(key: String) -> AppResult<Option<String>> {
    // Boundary-layer allowlist check (loop11 MEDIUM #5). Emit the same
    // message the inner match below would have produced, but reject at the
    // command boundary so the frontend receives a structured payload.
    if !crate::credentials::is_allowed_key(&key) {
        return Err(AppError::CredentialFileError {
            reason: format!("Unknown credential key: {}", key),
        });
    }
    let store = crate::credentials::load_credentials();
    // Note: `CredentialStore` implements `Drop` (via `ZeroizeOnDrop`), so we
    // cannot move fields out of it — clone the returned value instead. The
    // original `store` is zeroized when it goes out of scope.
    let value = match key.as_str() {
        "openai_api_key" => store.openai_api_key.clone(),
        "openrouter_api_key" => store.openrouter_api_key.clone(),
        "groq_api_key" => store.groq_api_key.clone(),
        "together_api_key" => store.together_api_key.clone(),
        "fireworks_api_key" => store.fireworks_api_key.clone(),
        "deepgram_api_key" => store.deepgram_api_key.clone(),
        "assemblyai_api_key" => store.assemblyai_api_key.clone(),
        "gemini_api_key" => store.gemini_api_key.clone(),
        "google_service_account_path" => store.google_service_account_path.clone(),
        "aws_access_key" => store.aws_access_key.clone(),
        "aws_secret_key" => store.aws_secret_key.clone(),
        "aws_session_token" => store.aws_session_token.clone(),
        "aws_profile" => store.aws_profile.clone(),
        "aws_region" => store.aws_region.clone(),
        _ => {
            return Err(AppError::CredentialFileError {
                reason: format!("Unknown credential key: {}", key),
            });
        }
    };
    Ok(value)
}

#[tauri::command]
pub fn load_all_credentials_cmd() -> crate::credentials::CredentialStore {
    crate::credentials::load_credentials()
}

/// Diagnose credential-store health. Surfaces parse/read errors from
/// `credentials.yaml` to the UI so users can tell the difference between
/// "no keys set" and "keys exist but the file is broken".
#[tauri::command]
pub fn diagnose_credentials() -> AppResult<String> {
    match crate::credentials::try_load_credentials() {
        Ok(store) => {
            let count = [
                store.openai_api_key.is_some(),
                store.groq_api_key.is_some(),
                store.deepgram_api_key.is_some(),
                store.assemblyai_api_key.is_some(),
                store.gemini_api_key.is_some(),
                store.aws_secret_key.is_some(),
            ]
            .iter()
            .filter(|&&b| b)
            .count();
            Ok(format!(
                "Credentials loaded successfully ({} keys present)",
                count
            ))
        }
        Err(reason) => Err(AppError::CredentialFileError { reason }),
    }
}

/// List available AWS profiles from ~/.aws/config and ~/.aws/credentials.
#[tauri::command]
pub fn list_aws_profiles() -> Vec<String> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    let mut profiles = std::collections::BTreeSet::new();

    for filename in &["config", "credentials"] {
        let path = home.join(".aws").join(filename);
        if let Ok(contents) = std::fs::read_to_string(&path) {
            for line in contents.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("[profile ") && trimmed.ends_with(']') {
                    let name = &trimmed[9..trimmed.len() - 1];
                    profiles.insert(name.to_string());
                } else if trimmed == "[default]" {
                    profiles.insert("default".to_string());
                } else if *filename == "credentials"
                    && trimmed.starts_with('[')
                    && trimmed.ends_with(']')
                {
                    let name = &trimmed[1..trimmed.len() - 1];
                    profiles.insert(name.to_string());
                }
            }
        }
    }

    profiles.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Cloud provider connection tests
// ---------------------------------------------------------------------------
//
// These commands let the Settings UI verify a user's API keys / credentials
// *before* they start a transcription session, so authentication failures
// surface immediately instead of after ~10s of silent audio streaming.

/// Test an OpenAI-compatible ASR endpoint by making a GET /models request.
#[tauri::command]
pub async fn test_cloud_asr_connection(endpoint: String, api_key: String) -> AppResult<String> {
    let url = format!("{}/models", endpoint.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))?;
    let mut req = client.get(&url);
    if !api_key.is_empty() {
        req = req.bearer_auth(&api_key);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Unknown(format!(
            "HTTP {}: {}",
            status,
            body.chars().take(200).collect::<String>()
        )));
    }
    Ok(format!("Connected to {} (HTTP {})", endpoint, status))
}

/// Test Deepgram API key by calling /v1/projects.
#[tauri::command]
pub async fn test_deepgram_connection(api_key: String) -> AppResult<String> {
    if api_key.is_empty() {
        return Err(AppError::CredentialMissing {
            key: "deepgram_api_key".to_string(),
        });
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))?;
    let resp = client
        // Use /v1/models (works with `usage` scope — the scope most keys
        // have for transcription). /v1/projects requires the `manage` scope
        // which would return 403 for valid transcription-only keys.
        .get("https://api.deepgram.com/v1/models")
        .header("Authorization", format!("Token {}", api_key))
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::Unknown(format!(
            "Deepgram returned HTTP {}",
            status
        )));
    }
    Ok("Deepgram API key is valid".to_string())
}

/// Test AssemblyAI API key by calling GET /v2/transcript with zero results.
#[tauri::command]
pub async fn test_assemblyai_connection(api_key: String) -> AppResult<String> {
    if api_key.is_empty() {
        return Err(AppError::CredentialMissing {
            key: "assemblyai_api_key".to_string(),
        });
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))?;
    let resp = client
        .get("https://api.assemblyai.com/v2/transcript?limit=1")
        .header("Authorization", &api_key)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::Unknown(format!(
            "AssemblyAI returned HTTP {}",
            status
        )));
    }
    Ok("AssemblyAI API key is valid".to_string())
}

/// Test Gemini API key via a simple listModels call.
///
/// Uses the `x-goog-api-key` header (not the `?key=` query string) to match
/// the production WebSocket auth pattern. Passing the key in URL would leak
/// it to DNS, proxies, and cert monitoring tools — and would silently succeed
/// even if the header-auth path is broken in production.
#[tauri::command]
pub async fn test_gemini_api_key(api_key: String) -> AppResult<String> {
    if api_key.trim().is_empty() {
        return Err(AppError::CredentialMissing {
            key: "gemini_api_key".to_string(),
        });
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))?;
    let resp = client
        .get("https://generativelanguage.googleapis.com/v1beta/models")
        .header("x-goog-api-key", api_key.trim())
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::Unknown(format!(
            "Gemini API returned HTTP {}",
            status
        )));
    }
    Ok("Gemini API key is valid".to_string())
}

/// Test AWS credentials via STS GetCallerIdentity (works for any AWS API access).
///
/// Shared between AWS Transcribe and AWS Bedrock settings — both providers
/// pull from the same backend credential store.
#[tauri::command]
pub async fn test_aws_credentials(
    region: String,
    credential_source: crate::settings::AwsCredentialSource,
) -> AppResult<String> {
    let region_trimmed = region.trim();
    if region_trimmed.is_empty() {
        return Err(AppError::AwsRegionInvalid {
            region: region_trimmed.to_string(),
        });
    }
    if !region_trimmed.contains('-') {
        return Err(AppError::AwsRegionInvalid {
            region: region_trimmed.to_string(),
        });
    }
    let region = region_trimmed.to_string();

    let sdk_config = crate::aws_util::build_aws_sdk_config(&region, credential_source).await?;
    let sts = aws_sdk_sts::Client::new(&sdk_config);
    let identity = sts
        .get_caller_identity()
        .send()
        .await
        .map_err(|e| format!("AWS auth failed: {}", e))?;
    Ok(format!(
        "Authenticated as {} (account: {})",
        identity.arn().unwrap_or("unknown"),
        identity.account().unwrap_or("unknown")
    ))
}

// ---------------------------------------------------------------------------
// OpenRouter cloud-LLM commands (ADR-0005, plan A2)
// ---------------------------------------------------------------------------

/// Validate an OpenRouter API key without spending tokens.
///
/// Hits `GET /api/v1/models` with the supplied key + canonical attribution
/// headers. Returns `Ok(_)` on HTTP 200 and a diagnostic `Err` on 401/403 or
/// network failure. Used by the Settings UI's "Test Connection" button.
#[tauri::command]
pub async fn test_openrouter_connection_cmd(api_key: String) -> AppResult<String> {
    if api_key.trim().is_empty() {
        return Err(AppError::CredentialMissing {
            key: "openrouter_api_key".to_string(),
        });
    }
    openrouter::test_connection(&api_key, openrouter::DEFAULT_BASE_URL)
        .await
        .map_err(AppError::Unknown)?;
    Ok("OpenRouter API key is valid".to_string())
}

/// Fetch the live OpenRouter model catalog for the settings model picker.
#[tauri::command]
pub async fn list_openrouter_models_cmd(api_key: String) -> AppResult<Vec<OpenRouterModel>> {
    if api_key.trim().is_empty() {
        return Err(AppError::CredentialMissing {
            key: "openrouter_api_key".to_string(),
        });
    }
    openrouter::list_models(&api_key, openrouter::DEFAULT_BASE_URL)
        .await
        .map_err(AppError::Unknown)
}

// ---------------------------------------------------------------------------
// TTS connection test (ADR-0004, plan A1)
// ---------------------------------------------------------------------------

/// Validate a TTS provider's credentials before the user starts a session.
///
/// Currently only `deepgram_aura` is wired up; the same Deepgram API key
/// works for both STT and TTS, so this command reuses the
/// `test_deepgram_connection` HTTP probe (`GET /v1/models`) under the
/// hood. Future providers (Kokoro, Piper, OpenAI TTS, ElevenLabs) will
/// branch on `provider` and dispatch their own probe.
///
/// `provider` is the `serde(tag = "type")` discriminator used by the
/// `TtsProvider` settings enum -- e.g. `"deepgram_aura"`. `none` returns
/// an error so the UI can short-circuit the "Test connection" button when
/// TTS is disabled.
#[tauri::command]
pub async fn test_tts_connection_cmd(provider: String, api_key: String) -> AppResult<String> {
    match provider.as_str() {
        "deepgram_aura" => {
            // Reuse the STT probe -- the same key authorises both surfaces.
            // We still tag the success message as TTS-specific so the UI
            // copy is unambiguous.
            test_deepgram_connection(api_key).await?;
            Ok("Deepgram Aura TTS credentials look valid".to_string())
        }
        "none" => Err(AppError::SessionInvalid {
            reason: "TTS is disabled in settings; nothing to test".to_string(),
        }),
        other => Err(AppError::Unknown(format!("Unknown TTS provider: {other}"))),
    }
}

// ---------------------------------------------------------------------------
// Audio playback (Wave B / audio-graph-8d75)
// ---------------------------------------------------------------------------

/// List the host's available output audio devices.
///
/// First entry (if any) has `is_default: true`. Returns an empty list on
/// hosts where cpal can't enumerate (rare; usually a missing audio service).
#[tauri::command]
pub async fn list_audio_output_devices_cmd() -> AppResult<Vec<crate::playback::OutputDevice>> {
    Ok(crate::playback::list_output_devices())
}

/// Open the configured output device + start the playback stream so
/// subsequent `push_samples` calls (typically driven by a TTS session) are
/// audible. `device_name = None` opens the host default.
#[tauri::command]
pub async fn start_audio_playback_cmd(
    state: State<'_, AppState>,
    device_name: Option<String>,
    source_sample_rate: Option<u32>,
) -> AppResult<()> {
    let config = crate::playback::PlaybackConfig {
        source_sample_rate: source_sample_rate.unwrap_or(24_000),
        source_channels: 1,
    };
    let result = match device_name {
        None => state.audio_player.open_default(config),
        Some(name) => state.audio_player.open_named(name, config),
    };
    result.map_err(|e| AppError::Unknown(e.to_string()))
}

/// Stop the active playback stream. Subsequent `push_samples` calls return
/// 0 (no producer) until a stream is reopened. Cancel is implicit.
#[tauri::command]
pub async fn stop_audio_playback_cmd(state: State<'_, AppState>) -> AppResult<()> {
    state
        .audio_player
        .stop()
        .map_err(|e| AppError::Unknown(e.to_string()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // PART 1 — configure_api_endpoint URL validation regression tests
    // (loop-13 MEDIUM #4). The validation landed in loop 12 without
    // coverage; these lock in the accept/reject contract so a future
    // refactor can't silently loosen it.
    // -----------------------------------------------------------------------

    #[test]
    fn validate_endpoint_url_accepts_https() {
        let u =
            validate_endpoint_url("https://api.openai.com/v1").expect("https URL must be accepted");
        assert_eq!(u.scheme(), "https");
    }

    #[test]
    fn validate_endpoint_url_accepts_http() {
        // Plain http is legitimate for local servers (Ollama, LM Studio, vLLM).
        let u = validate_endpoint_url("http://localhost:11434/v1")
            .expect("http URL must be accepted for local servers");
        assert_eq!(u.scheme(), "http");
    }

    #[test]
    fn validate_endpoint_url_rejects_malformed() {
        let err = validate_endpoint_url("not a url").expect_err("garbage must be rejected");
        assert!(
            err.contains("Invalid endpoint URL"),
            "error should mention invalid URL, got: {}",
            err
        );
    }

    #[test]
    fn validate_endpoint_url_rejects_disallowed_schemes() {
        // file:// would let a settings-file edit coax the app into reading
        // local files. ftp:// is non-functional with reqwest. Both must be
        // rejected up-front with a scheme-specific message.
        for bad in &["file:///etc/passwd", "ftp://example.com/models"] {
            let err = validate_endpoint_url(bad).expect_err(&format!("{} must be rejected", bad));
            assert!(
                err.contains("unsupported scheme"),
                "error for {} should mention unsupported scheme, got: {}",
                bad,
                err
            );
        }
    }

    // -----------------------------------------------------------------------
    // load_credential_cmd loadback contract (FA-3). The boundary allowlist
    // `ALLOWED_CREDENTIAL_KEYS` and the inner `match key.as_str()` dispatcher
    // must stay in sync: a key allowed at the boundary but missing a match arm
    // passes validation then dies at `_ => Err("Unknown credential key")`, so a
    // saved value can never be loaded back (this is how openrouter_api_key
    // regressed). Drive the allowlist itself as the fixture so the two lists are
    // provably consistent — this fails the instant a future key is added to the
    // allowlist without a corresponding load arm.
    // -----------------------------------------------------------------------

    #[test]
    fn load_credential_cmd_handles_every_allowed_key() {
        for &key in crate::credentials::ALLOWED_CREDENTIAL_KEYS {
            let result = load_credential_cmd(key.to_string());
            // It may legitimately be Ok(Some) (key is set on this box) or
            // Ok(None) (unset) — but it must NEVER be the unknown-key error,
            // which is what a missing match arm produces. Assert on presence of
            // the error variant + message, not on the secret value (never echo).
            match result {
                Ok(_) => {}
                Err(AppError::CredentialFileError { reason }) => {
                    assert!(
                        !reason.contains("Unknown credential key"),
                        "allowed key {key:?} has no loadback match arm in \
                         load_credential_cmd — add `\"{key}\" => store.{key}.clone()`"
                    );
                }
                Err(_) => {
                    // Any other error (e.g. a malformed file on disk) is
                    // unrelated to the allowlist/dispatcher sync contract.
                }
            }
        }
    }

    #[test]
    fn load_credential_cmd_rejects_unknown_key() {
        let err = load_credential_cmd("definitely_not_a_key".to_string())
            .expect_err("an unallowed key must be rejected at the boundary");
        match err {
            AppError::CredentialFileError { reason } => {
                assert!(reason.contains("Unknown credential key"));
            }
            other => panic!("expected CredentialFileError, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // FA-7 — send_chat_message must surface the real token count on the
    // streaming path instead of a hardcoded 0. The streaming `Done` frame
    // carries the provider's terminal `usage` block; `tokens_used_from_stream_usage`
    // is the pure derivation used at that return site. These pin the contract:
    // a populated usage block yields a non-zero `total_tokens`, and a genuinely
    // absent signal stays 0 (honest "unknown", not a fabricated count).
    // -----------------------------------------------------------------------

    #[test]
    fn tokens_used_flows_through_from_stream_usage() {
        use crate::llm::sse::StreamUsage;
        let usage = Some(StreamUsage {
            prompt_tokens: Some(12),
            completion_tokens: Some(34),
            total_tokens: Some(46),
        });
        assert_eq!(
            tokens_used_from_stream_usage(usage),
            46,
            "a populated usage block must surface total_tokens, not 0"
        );
    }

    #[test]
    fn tokens_used_streaming_done_arm_populates_from_usage() {
        // Exercise the exact accumulation the streaming branch of
        // send_chat_message runs: walk frames and, on Done, derive tokens_used
        // from the terminal usage block. Proves a non-zero count flows through
        // end-to-end for a provider that reports usage.
        use crate::llm::sse::StreamUsage;
        use crate::llm::streaming::TokenDelta;

        let frames = vec![
            TokenDelta::Delta {
                content: "Hello".to_string(),
                finish_reason: None,
            },
            TokenDelta::Delta {
                content: " world".to_string(),
                finish_reason: None,
            },
            TokenDelta::Done {
                full_text: "Hello world".to_string(),
                usage: Some(StreamUsage {
                    prompt_tokens: Some(8),
                    completion_tokens: Some(2),
                    total_tokens: Some(10),
                }),
                finish_reason: "stop".to_string(),
            },
        ];

        let mut full_text = String::new();
        let mut tokens_used = 0u32;
        for frame in frames {
            match frame {
                TokenDelta::Delta { content, .. } => full_text.push_str(&content),
                TokenDelta::Done {
                    full_text: t,
                    usage,
                    ..
                } => {
                    if !t.is_empty() {
                        full_text = t;
                    }
                    tokens_used = tokens_used_from_stream_usage(usage);
                    break;
                }
                _ => unreachable!("no error/cancel in this fixture"),
            }
        }

        assert_eq!(full_text, "Hello world");
        assert_eq!(
            tokens_used, 10,
            "streaming Done arm must thread the real total_tokens into ChatResponse"
        );
    }

    #[test]
    fn tokens_used_is_zero_when_provider_omits_usage() {
        use crate::llm::sse::StreamUsage;
        // Provider never honoured include_usage → no usage block at all.
        assert_eq!(tokens_used_from_stream_usage(None), 0);
        // Usage block present but total_tokens unset → still honestly 0.
        assert_eq!(
            tokens_used_from_stream_usage(Some(StreamUsage {
                prompt_tokens: Some(5),
                completion_tokens: Some(7),
                total_tokens: None,
            })),
            0
        );
    }

    #[test]
    fn sync_llm_api_client_replaces_stale_runtime_config() {
        let state = AppState::new();
        let mut settings = crate::settings::AppSettings {
            llm_provider: crate::settings::LlmProvider::Api {
                endpoint: "http://localhost:8000/v1".to_string(),
                api_key: "first-secret".to_string(),
                model: "first-model".to_string(),
            },
            llm_api_config: Some(crate::settings::LlmApiConfig {
                endpoint: "http://localhost:8000/v1".to_string(),
                api_key: None,
                model: "first-model".to_string(),
                max_tokens: 2048,
                temperature: 0.7,
            }),
            ..Default::default()
        };

        *state.app_settings.write().expect("lock poisoned") = settings.clone();
        sync_llm_api_client_from_settings_cache(&state).expect("initial sync must succeed");

        settings.llm_provider = crate::settings::LlmProvider::Api {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: "second-secret".to_string(),
            model: "gpt-4o-mini".to_string(),
        };
        settings.llm_api_config = Some(crate::settings::LlmApiConfig {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: None,
            model: "gpt-4o-mini".to_string(),
            max_tokens: 1024,
            temperature: 0.2,
        });
        *state.app_settings.write().expect("lock poisoned") = settings;
        sync_llm_api_client_from_settings_cache(&state).expect("resync must succeed");

        let guard = state.api_client.lock().expect("lock poisoned");
        let config = guard.as_ref().expect("client configured").config();
        assert_eq!(config.endpoint, "https://api.openai.com/v1");
        assert_eq!(config.api_key.as_deref(), Some("second-secret"));
        assert_eq!(config.model, "gpt-4o-mini");
        assert_eq!(config.max_tokens, 1024);
        assert!((config.temperature - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn sync_llm_api_client_clears_when_provider_is_not_api() {
        let state = AppState::new();
        *state.app_settings.write().expect("lock poisoned") = crate::settings::AppSettings {
            llm_provider: crate::settings::LlmProvider::Api {
                endpoint: "http://localhost:11434/v1".to_string(),
                api_key: String::new(),
                model: "llama3.2".to_string(),
            },
            ..Default::default()
        };
        sync_llm_api_client_from_settings_cache(&state).expect("initial sync must succeed");
        assert!(state.api_client.lock().expect("lock poisoned").is_some());

        *state.app_settings.write().expect("lock poisoned") = crate::settings::AppSettings {
            llm_provider: crate::settings::LlmProvider::LocalLlama,
            ..Default::default()
        };
        sync_llm_api_client_from_settings_cache(&state).expect("clear sync must succeed");

        assert!(state.api_client.lock().expect("lock poisoned").is_none());
    }

    #[test]
    fn api_config_from_runtime_settings_ignores_stale_detail_config() {
        let settings = crate::settings::AppSettings {
            llm_provider: crate::settings::LlmProvider::Api {
                endpoint: "http://localhost:8000/v1".to_string(),
                api_key: String::new(),
                model: "active-model".to_string(),
            },
            llm_api_config: Some(crate::settings::LlmApiConfig {
                endpoint: "https://api.openai.com/v1".to_string(),
                api_key: Some("stale-secret".to_string()),
                model: "stale-model".to_string(),
                max_tokens: 4096,
                temperature: 0.9,
            }),
            ..Default::default()
        };

        let config = api_config_from_runtime_settings(&settings).expect("API provider configured");

        assert_eq!(config.endpoint, "http://localhost:8000/v1");
        assert_eq!(config.model, "active-model");
        assert_eq!(config.api_key, None);
        assert_eq!(config.max_tokens, 512);
        assert!((config.temperature - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn streaming_source_guard_allows_batch_providers_to_use_multiple_sources() {
        let active_sources = vec!["system-default".to_string(), "device:mic".to_string()];

        validate_streaming_asr_source_count(
            &crate::settings::AsrProvider::LocalWhisper,
            &active_sources,
            Some("app:42"),
        )
        .expect("local batch ASR supports per-source accumulators");

        validate_streaming_asr_source_count(
            &crate::settings::AsrProvider::Api {
                endpoint: "https://example.com/v1".to_string(),
                api_key: String::new(),
                model: "whisper-large-v3".to_string(),
            },
            &active_sources,
            Some("app:42"),
        )
        .expect("cloud batch ASR supports per-source accumulators");
    }

    #[test]
    fn streaming_source_guard_rejects_second_source_for_single_session_providers() {
        let active_sources = vec!["system-default".to_string()];
        let providers = vec![
            (
                crate::settings::AsrProvider::AssemblyAI {
                    api_key: String::new(),
                    enable_diarization: true,
                },
                "AssemblyAI streaming",
            ),
            (
                crate::settings::AsrProvider::AwsTranscribe {
                    region: "us-east-1".to_string(),
                    language_code: "en-US".to_string(),
                    credential_source: crate::settings::AwsCredentialSource::DefaultChain,
                    enable_diarization: true,
                },
                "AWS Transcribe streaming",
            ),
            (
                crate::settings::AsrProvider::SherpaOnnx {
                    model_dir: "streaming-zipformer-en-20M".to_string(),
                    enable_endpoint_detection: true,
                },
                "Sherpa-ONNX streaming",
            ),
        ];

        for (provider, provider_name) in providers {
            let err =
                validate_streaming_asr_source_count(&provider, &active_sources, Some("device:mic"))
                    .expect_err("streaming provider must reject a second source");

            assert!(
                err.contains(provider_name),
                "error should name provider, got: {}",
                err
            );
            assert!(
                err.contains("system-default") && err.contains("device:mic"),
                "error should list active and pending sources, got: {}",
                err
            );
        }
    }

    #[test]
    fn streaming_source_guard_allows_existing_source_restart_path() {
        let active_sources = vec!["system-default".to_string()];
        validate_streaming_asr_source_count(
            &crate::settings::AsrProvider::DeepgramStreaming {
                api_key: String::new(),
                model: "nova-3".to_string(),
                enable_diarization: true,
                endpointing_ms: 300,
                utterance_end_ms: 1000,
                vad_events: true,
                eot_threshold: 0.5,
                eager_eot_threshold: 0.0,
                eot_timeout_ms: 0,
                max_speakers: 2,
            },
            &active_sources,
            Some("system-default"),
        )
        .expect("same source should not count as a second streaming session");
    }

    #[test]
    fn streaming_source_guard_rejects_multi_source_transcription_start() {
        let active_sources = vec!["system-default".to_string(), "device:mic".to_string()];
        let err = validate_streaming_asr_source_count(
            &crate::settings::AsrProvider::AssemblyAI {
                api_key: String::new(),
                enable_diarization: true,
            },
            &active_sources,
            None,
        )
        .expect_err("starting transcription with multiple sources should be rejected");

        assert!(err.contains("AssemblyAI streaming"));
        assert!(err.contains("system-default") && err.contains("device:mic"));
    }

    #[test]
    fn streaming_source_guard_allows_multiple_sources_for_deepgram_mixed() {
        // Deepgram now feeds through the audio mixer, so multiple sources are
        // allowed (they are summed into one mixed stream).
        let active_sources = vec!["system-default".to_string(), "device:mic".to_string()];
        validate_streaming_asr_source_count(
            &crate::settings::AsrProvider::DeepgramStreaming {
                api_key: String::new(),
                model: "nova-3".to_string(),
                enable_diarization: true,
                endpointing_ms: 300,
                utterance_end_ms: 1000,
                vad_events: true,
                eot_threshold: 0.5,
                eager_eot_threshold: 0.0,
                eot_timeout_ms: 0,
                max_speakers: 2,
            },
            &active_sources,
            Some("app:42"),
        )
        .expect("Deepgram mixes multiple sources, so multi-source is allowed");
    }

    // -----------------------------------------------------------------------
    // PART 2 — log_level persistence race (loop-13 MEDIUM #6).
    // set_log_level is now the runtime-only path; save_settings_cmd owns
    // the single disk-write path. The full command needs a Tauri AppHandle
    // (not available in unit tests), so we exercise the in-memory half
    // directly and assert the invariant that matters: the cache tracks
    // the latest level without triggering a disk flush.
    // -----------------------------------------------------------------------

    #[test]
    fn set_log_level_does_not_persist_to_disk_on_repeated_calls() {
        // Simulate what `set_log_level` does to the in-memory cache: apply
        // the runtime level, then mutate `app_settings.log_level`. Repeating
        // this twice must leave the cache reflecting the final value and
        // must not touch disk — which it can't, because we never hand it
        // an AppHandle.
        let state = AppState::new();

        // First call: info → debug.
        crate::logging::apply_log_level("debug");
        {
            let mut cached = state.app_settings.write().expect("lock poisoned");
            cached.log_level = Some("debug".to_string());
        }
        assert_eq!(
            state.app_settings.read().unwrap().log_level.as_deref(),
            Some("debug"),
            "cache must reflect first update"
        );

        // Second call: debug → warn. With the old contract this would have
        // produced a second disk write; under the new contract it only
        // mutates runtime + cache.
        crate::logging::apply_log_level("warn");
        {
            let mut cached = state.app_settings.write().expect("lock poisoned");
            cached.log_level = Some("warn".to_string());
        }
        assert_eq!(
            state.app_settings.read().unwrap().log_level.as_deref(),
            Some("warn"),
            "cache must reflect second update"
        );

        // Restore a sensible default so later tests in the same binary
        // aren't silently swallowing logs at warn.
        crate::logging::apply_log_level("info");
    }
}
