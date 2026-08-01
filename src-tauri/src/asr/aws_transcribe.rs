//! AWS Transcribe Streaming ASR integration.
//!
//! Uses the aws-sdk-transcribestreaming crate to stream audio to AWS
//! and receive real-time transcription results with optional speaker diarization.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use audio_graph_ipc_contract::runtime_diagnostic::{
    RuntimeDiagnostic, RuntimeDiagnosticContext, RuntimeErrorCode, RuntimeErrorDiagnostic,
    RuntimeOperation, RuntimeSafeRecoveryAction, RuntimeStatusClass, RuntimeTransport,
};
use aws_sdk_transcribestreaming as transcribe;
use aws_sdk_transcribestreaming::error::ProvideErrorMetadata;
use aws_sdk_transcribestreaming::operation::start_stream_transcription::StartStreamTranscriptionError;
use aws_sdk_transcribestreaming::primitives::Blob;
use aws_sdk_transcribestreaming::types::error::TranscriptResultStreamError;
use aws_sdk_transcribestreaming::types::{Alternative, AudioEvent, AudioStream, MediaEncoding};
use crossbeam_channel::Receiver;
use uuid::Uuid;

use crate::audio::pcm::f32_mono_to_pcm_s16le_bytes;
use crate::audio::pipeline::ProcessedAudioChunk;
use crate::aws_util::build_aws_sdk_config;
use crate::settings::AwsCredentialSource;
use crate::state::TranscriptSegment;
use std::time::Duration;

use super::ProviderContentEgressPolicy;
#[cfg(test)]
use super::reconnect::backoff_for_attempt;
use super::reconnect::{ReconnectStep, next_reconnect_step};

const EXPLICIT_POLICY_REQUIRED: &str = "explicit_policy_required";

pub struct AwsTranscribeConfig {
    pub region: String,
    pub language_code: String,
    pub credential_source: AwsCredentialSource,
    pub enable_diarization: bool,
}

/// View over AWS Transcribe settings plus a content-egress policy.
pub trait AwsTranscribeSessionConfig {
    fn region(&self) -> &str;
    fn language_code(&self) -> &str;
    fn credential_source(&self) -> &AwsCredentialSource;
    fn enable_diarization(&self) -> bool;

    fn content_egress_policy(&self) -> ProviderContentEgressPolicy {
        ProviderContentEgressPolicy::block(EXPLICIT_POLICY_REQUIRED)
    }
}

impl AwsTranscribeSessionConfig for AwsTranscribeConfig {
    fn region(&self) -> &str {
        &self.region
    }

    fn language_code(&self) -> &str {
        &self.language_code
    }

    fn credential_source(&self) -> &AwsCredentialSource {
        &self.credential_source
    }

    fn enable_diarization(&self) -> bool {
        self.enable_diarization
    }
}

pub struct GuardedAwsTranscribeConfig {
    inner: AwsTranscribeConfig,
    content_egress_policy: ProviderContentEgressPolicy,
}

impl AwsTranscribeConfig {
    pub fn with_content_egress_policy(
        self,
        policy: ProviderContentEgressPolicy,
    ) -> GuardedAwsTranscribeConfig {
        GuardedAwsTranscribeConfig {
            inner: self,
            content_egress_policy: policy,
        }
    }
}

impl AwsTranscribeSessionConfig for GuardedAwsTranscribeConfig {
    fn region(&self) -> &str {
        self.inner.region()
    }

    fn language_code(&self) -> &str {
        self.inner.language_code()
    }

    fn credential_source(&self) -> &AwsCredentialSource {
        self.inner.credential_source()
    }

    fn enable_diarization(&self) -> bool {
        self.inner.enable_diarization()
    }

    fn content_egress_policy(&self) -> ProviderContentEgressPolicy {
        self.content_egress_policy
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AwsTranscribePartial {
    pub source_id: String,
    pub provider_item_id: Option<String>,
    pub text: String,
    pub start_time: f64,
    pub end_time: f64,
    pub confidence: f32,
}

#[derive(Debug, Clone)]
pub struct AwsTranscribeFinal {
    pub segment: TranscriptSegment,
    pub provider_item_id: Option<String>,
}

const AWS_TRANSCRIBE_SOURCE_FALLBACK: &str = "aws-transcribe-stream";

fn f32_to_pcm_bytes(samples: &[f32]) -> Vec<u8> {
    f32_mono_to_pcm_s16le_bytes(samples)
}

fn source_hint_or_fallback(source_id_hint: &Arc<RwLock<Option<String>>>) -> String {
    source_id_hint
        .read()
        .ok()
        .and_then(|hint| hint.clone())
        .unwrap_or_else(|| AWS_TRANSCRIBE_SOURCE_FALLBACK.to_string())
}

fn transcript_text(alt: &Alternative) -> Option<String> {
    let text = alt.transcript()?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn speaker_label(alt: &Alternative) -> Option<String> {
    alt.items()
        .iter()
        .find_map(|item| item.speaker().map(str::to_string))
}

fn alternative_confidence(alt: &Alternative) -> Option<f32> {
    let mut sum = 0.0f32;
    let mut count = 0usize;

    for confidence in alt.items().iter().filter_map(|item| item.confidence()) {
        sum += confidence as f32;
        count += 1;
    }

    (count > 0).then_some(sum / count as f32)
}

fn sdk_error_kind<E, R>(error: &transcribe::error::SdkError<E, R>) -> &'static str {
    match error {
        transcribe::error::SdkError::ConstructionFailure(_) => "construction_failure",
        transcribe::error::SdkError::TimeoutError(_) => "timeout",
        transcribe::error::SdkError::DispatchFailure(_) => "dispatch_failure",
        transcribe::error::SdkError::ResponseError(_) => "response_error",
        transcribe::error::SdkError::ServiceError(_) => "service_error",
        _ => "unknown",
    }
}

fn sdk_diagnostic_context() -> RuntimeDiagnosticContext {
    RuntimeDiagnosticContext::new(RuntimeOperation::Transcription)
        .with_transport(RuntimeTransport::Sdk)
}

fn sdk_runtime_diagnostic(
    code: RuntimeErrorCode,
    retryable: bool,
    recovery_action: RuntimeSafeRecoveryAction,
    status_class: Option<RuntimeStatusClass>,
) -> RuntimeDiagnostic {
    let mut context = sdk_diagnostic_context();
    if let Some(status_class) = status_class {
        context = context.with_status_class(status_class);
    }
    RuntimeErrorDiagnostic::new(code, retryable, recovery_action, context).into()
}

fn unknown_sdk_diagnostic() -> RuntimeDiagnostic {
    RuntimeDiagnostic::internal(sdk_diagnostic_context())
}

#[derive(Debug, Clone, Copy)]
enum AwsServiceFailure {
    BadRequest,
    Conflict,
    InternalFailure,
    LimitExceeded,
    ServiceUnavailable,
}

fn service_failure_diagnostic(failure: AwsServiceFailure) -> RuntimeDiagnostic {
    match failure {
        AwsServiceFailure::BadRequest | AwsServiceFailure::Conflict => sdk_runtime_diagnostic(
            RuntimeErrorCode::RequestRejected,
            false,
            RuntimeSafeRecoveryAction::ReviewConfiguration,
            Some(RuntimeStatusClass::ClientError),
        ),
        AwsServiceFailure::InternalFailure => sdk_runtime_diagnostic(
            RuntimeErrorCode::Internal,
            true,
            RuntimeSafeRecoveryAction::Retry,
            Some(RuntimeStatusClass::ServerError),
        ),
        AwsServiceFailure::LimitExceeded => sdk_runtime_diagnostic(
            RuntimeErrorCode::CapacityExhausted,
            true,
            RuntimeSafeRecoveryAction::RetryAfterDelay,
            Some(RuntimeStatusClass::ClientError),
        ),
        AwsServiceFailure::ServiceUnavailable => sdk_runtime_diagnostic(
            RuntimeErrorCode::ProviderUnavailable,
            true,
            RuntimeSafeRecoveryAction::RetryAfterDelay,
            Some(RuntimeStatusClass::ServerError),
        ),
    }
}

/// Exact allowlist for metadata codes that correspond to generated, closed
/// Transcribe variants. Unknown metadata is never copied into the diagnostic.
fn allowlisted_service_failure(code: Option<&str>) -> Option<AwsServiceFailure> {
    match code {
        Some("BadRequestException") => Some(AwsServiceFailure::BadRequest),
        Some("ConflictException") => Some(AwsServiceFailure::Conflict),
        Some("InternalFailureException") => Some(AwsServiceFailure::InternalFailure),
        Some("LimitExceededException") => Some(AwsServiceFailure::LimitExceeded),
        Some("ServiceUnavailableException") => Some(AwsServiceFailure::ServiceUnavailable),
        _ => None,
    }
}

fn start_stream_service_diagnostic(error: &StartStreamTranscriptionError) -> RuntimeDiagnostic {
    let failure = match error {
        StartStreamTranscriptionError::BadRequestException(_) => AwsServiceFailure::BadRequest,
        StartStreamTranscriptionError::ConflictException(_) => AwsServiceFailure::Conflict,
        StartStreamTranscriptionError::InternalFailureException(_) => {
            AwsServiceFailure::InternalFailure
        }
        StartStreamTranscriptionError::LimitExceededException(_) => {
            AwsServiceFailure::LimitExceeded
        }
        StartStreamTranscriptionError::ServiceUnavailableException(_) => {
            AwsServiceFailure::ServiceUnavailable
        }
        _ => {
            return error
                .code()
                .and_then(|code| allowlisted_service_failure(Some(code)))
                .map(service_failure_diagnostic)
                .unwrap_or_else(unknown_sdk_diagnostic);
        }
    };
    service_failure_diagnostic(failure)
}

fn transcript_stream_service_diagnostic(error: &TranscriptResultStreamError) -> RuntimeDiagnostic {
    let failure = match error {
        TranscriptResultStreamError::BadRequestException(_) => AwsServiceFailure::BadRequest,
        TranscriptResultStreamError::ConflictException(_) => AwsServiceFailure::Conflict,
        TranscriptResultStreamError::InternalFailureException(_) => {
            AwsServiceFailure::InternalFailure
        }
        TranscriptResultStreamError::LimitExceededException(_) => AwsServiceFailure::LimitExceeded,
        TranscriptResultStreamError::ServiceUnavailableException(_) => {
            AwsServiceFailure::ServiceUnavailable
        }
        _ => {
            return error
                .code()
                .and_then(|code| allowlisted_service_failure(Some(code)))
                .map(service_failure_diagnostic)
                .unwrap_or_else(unknown_sdk_diagnostic);
        }
    };
    service_failure_diagnostic(failure)
}

fn sdk_error_diagnostic<E, R>(
    error: &transcribe::error::SdkError<E, R>,
    service_diagnostic: impl Fn(&E) -> RuntimeDiagnostic,
) -> RuntimeDiagnostic {
    match error {
        transcribe::error::SdkError::ConstructionFailure(_) => unknown_sdk_diagnostic(),
        transcribe::error::SdkError::TimeoutError(_) => sdk_runtime_diagnostic(
            RuntimeErrorCode::Timeout,
            true,
            RuntimeSafeRecoveryAction::Retry,
            None,
        ),
        transcribe::error::SdkError::DispatchFailure(_) => sdk_runtime_diagnostic(
            RuntimeErrorCode::NetworkUnreachable,
            true,
            RuntimeSafeRecoveryAction::CheckNetwork,
            None,
        ),
        transcribe::error::SdkError::ResponseError(_) => sdk_runtime_diagnostic(
            RuntimeErrorCode::InvalidResponse,
            true,
            RuntimeSafeRecoveryAction::Retry,
            None,
        ),
        transcribe::error::SdkError::ServiceError(service) => service_diagnostic(service.err()),
        _ => unknown_sdk_diagnostic(),
    }
}

fn start_stream_sdk_diagnostic<R>(
    error: &transcribe::error::SdkError<StartStreamTranscriptionError, R>,
) -> RuntimeDiagnostic {
    sdk_error_diagnostic(error, start_stream_service_diagnostic)
}

fn transcript_stream_sdk_diagnostic<R>(
    error: &transcribe::error::SdkError<TranscriptResultStreamError, R>,
) -> RuntimeDiagnostic {
    sdk_error_diagnostic(error, transcript_stream_service_diagnostic)
}

fn partial_from_result(
    result: &transcribe::types::Result,
    source_id: String,
) -> Option<AwsTranscribePartial> {
    if !result.is_partial() {
        return None;
    }

    result.alternatives().iter().find_map(|alt| {
        transcript_text(alt).map(|text| AwsTranscribePartial {
            source_id: source_id.clone(),
            provider_item_id: result.result_id().map(str::to_string),
            text,
            start_time: result.start_time(),
            end_time: result.end_time(),
            confidence: alternative_confidence(alt).unwrap_or(0.0),
        })
    })
}

fn final_segments_from_result(
    result: &transcribe::types::Result,
    source_id: &str,
) -> Vec<AwsTranscribeFinal> {
    if result.is_partial() {
        return Vec::new();
    }

    let result_start = result.start_time();
    let result_end = result.end_time();
    let provider_item_id = result.result_id().map(str::to_string);

    result
        .alternatives()
        .iter()
        .filter_map(|alt| {
            let text = transcript_text(alt)?;
            let speaker_label = speaker_label(alt);
            let confidence = alternative_confidence(alt).unwrap_or(0.9);

            Some(AwsTranscribeFinal {
                segment: TranscriptSegment {
                    id: Uuid::new_v4().to_string(),
                    source_id: source_id.to_string(),
                    speaker_id: speaker_label.clone(),
                    speaker_label,
                    text,
                    start_time: result_start,
                    end_time: result_end,
                    confidence,
                },
                provider_item_id: provider_item_id.clone(),
            })
        })
        .collect()
}

/// Reconnect lifecycle notification emitted while the streaming session runs.
///
/// Mirrors the `Reconnecting`/`Reconnected` events the WebSocket ASR siblings
/// push through their event channels — the callback-based AWS path surfaces the
/// same parity through this status callback so the speech processor can update
/// the pipeline `StageStatus` for the UI (M1 / audio-graph-35de).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AwsTranscribeStatus {
    /// A recoverable drop was detected; a reconnect is scheduled after
    /// `backoff_secs` (1-based `attempt` on the shared ladder).
    Reconnecting { attempt: u32, backoff_secs: u64 },
    /// The stream was successfully re-established.
    Reconnected,
}

/// Outcome of driving one connected transcription stream to completion.
#[derive(Debug)]
enum DriveOutcome {
    /// `is_transcribing` was cleared — user-initiated stop, do not reconnect.
    UserStopped,
    /// The result stream ended cleanly (server finalized after input close).
    Completed,
    /// A recoverable transport error (dispatch/timeout/response) or an
    /// unexpected server close while still transcribing — reconnect.
    Recoverable(RuntimeDiagnostic),
    /// A non-recoverable error (service/construction) — surface and stop.
    Unrecoverable(RuntimeDiagnostic),
}

/// A single step returned by the shared reconnect ladder.
#[derive(Debug)]
enum LadderStep {
    /// Backoff elapsed; the caller should attempt to re-open the stream.
    Continue,
    /// `is_transcribing` was cleared during backoff — stop cleanly.
    Cancelled,
    /// The backoff schedule is exhausted; the caller returns its last closed diagnostic.
    GiveUp,
}

/// Whether a `DriveOutcome` warrants a reconnect attempt. Recoverable outcomes
/// retry on the ladder; user-stop, clean completion, and unrecoverable errors
/// do not (M1 / audio-graph-35de). Pure so the retry policy is unit-testable
/// without a live AWS stream.
fn should_reconnect(outcome: &DriveOutcome) -> bool {
    matches!(outcome, DriveOutcome::Recoverable(_))
}

/// Parse a configured AWS language code, warning + falling back to `en-US` on an
/// unsupported value (review m4).
///
/// The SDK's `FromStr` for `LanguageCode` is `Infallible` — an unknown code
/// parses to `LanguageCode::Unknown(..)`, never `Err` — so the previous
/// `.parse().unwrap_or(EnUs)` fallback was dead code that silently forwarded a
/// typo'd code to AWS. To detect the misconfiguration we gate on the SDK's
/// generated [`transcribe::types::LanguageCode::values()`] list before
/// converting via `From<&str>`: both exist on every 1.10x release of the sealed
/// enum (unlike `try_parse`, which is absent from some releases), and this
/// avoids matching the `#[deprecated]`-against-matching `Unknown(_)` variant
/// directly. The language code is non-secret config, safe to log verbatim.
fn parse_language_code_or_warn(configured: &str) -> transcribe::types::LanguageCode {
    if transcribe::types::LanguageCode::values().contains(&configured) {
        transcribe::types::LanguageCode::from(configured)
    } else {
        log::warn!(
            "AWS Transcribe: unsupported language_code {configured:?}; \
             falling back to en-US (check the ASR language setting)"
        );
        transcribe::types::LanguageCode::EnUs
    }
}

/// Minimum time a re-established stream must stay healthy before the reconnect
/// budget resets. Deepgram-style "reset on success" resets the ladder as soon
/// as the socket ACCEPTS — on a flapping link that opens but cannot sustain,
/// that re-enters every failure at attempt 1 and loops forever at the 1s rung,
/// never reaching the documented 1/2/5/10 give-up. Requiring sustained health
/// before the reset closes that hole: accept-then-immediate-drop keeps
/// climbing the ladder and exhausts it (Codex P2 on PR #83).
const HEALTHY_STREAM_RESET_SECS: u64 = 30;

/// Whether a stream that just failed was healthy long enough to earn a fresh
/// reconnect budget. Pure so the flapping-link policy is unit-testable.
fn should_reset_reconnect_budget(healthy_for: Duration) -> bool {
    healthy_for >= Duration::from_secs(HEALTHY_STREAM_RESET_SECS)
}

/// Recoverable `SdkError` classes for stream re-establishment: transport-level
/// failures (`dispatch_failure`, `timeout`, `response_error`). Service and
/// construction failures are not retried — they will not clear on a retry.
fn is_recoverable_error_kind(kind: &str) -> bool {
    matches!(kind, "dispatch_failure" | "timeout" | "response_error")
}

fn is_recoverable_sdk_error<E, R>(error: &transcribe::error::SdkError<E, R>) -> bool {
    is_recoverable_error_kind(sdk_error_kind(error))
}

/// Chunks pulled off the shared capture channel that could NOT be delivered to
/// a live stream (stop raced the `recv`, or the stream died mid-send). They
/// survive the reconnect and are drained — in order, ahead of new capture —
/// into the next stream so a reconnect never opens an audio gap (Codex P2 on
/// PR #83).
type CarryoverQueue = Arc<Mutex<VecDeque<ProcessedAudioChunk>>>;

fn push_carryover(carryover: &CarryoverQueue, chunk: ProcessedAudioChunk) {
    carryover
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push_back(chunk);
}

fn pop_carryover(carryover: &CarryoverQueue) -> Option<ProcessedAudioChunk> {
    carryover
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .pop_front()
}

/// Background task handle that forwards captured PCM chunks into one SDK audio
/// stream. Dropped/stopped per-connection so a reconnect can spawn a fresh
/// forwarder against the new stream while `audio_rx` (the shared capture
/// channel) buffers chunks during the backoff window.
struct AudioForwarder {
    active: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}

impl AudioForwarder {
    fn stop(&self) {
        self.active.store(false, Ordering::Relaxed);
    }

    async fn join(self) {
        let _ = self.handle.await;
    }
}

fn spawn_audio_forwarder(
    audio_rx: Receiver<ProcessedAudioChunk>,
    audio_tx: tokio::sync::mpsc::Sender<
        Result<AudioStream, transcribe::types::error::AudioStreamError>,
    >,
    is_transcribing: Arc<AtomicBool>,
    source_id_hint: Arc<RwLock<Option<String>>>,
    carryover: CarryoverQueue,
) -> AudioForwarder {
    let active = Arc::new(AtomicBool::new(true));
    let active_task = Arc::clone(&active);
    // The loop is blocking by nature (crossbeam `recv_timeout`). The session
    // drives a CURRENT-THREAD tokio runtime, so running this inside
    // `tokio::spawn` would block the only runtime thread — stalling the
    // transcript stream and the AWS SDK I/O whenever audio idles (CodeRabbit
    // on PR #83). `spawn_blocking` moves it to the blocking pool; stream sends
    // use `blocking_send`, which is safe (and intended) off the async runtime.
    let handle = tokio::task::spawn_blocking(move || {
        // Deliver one chunk to the stream, or return it for carryover when the
        // stream channel has died so the chunk reaches the NEXT stream instead
        // of vanishing with the abandoned one.
        fn deliver(
            audio_tx: &tokio::sync::mpsc::Sender<
                Result<AudioStream, transcribe::types::error::AudioStreamError>,
            >,
            source_id_hint: &Arc<RwLock<Option<String>>>,
            chunk: ProcessedAudioChunk,
        ) -> Result<(), ProcessedAudioChunk> {
            if let Ok(mut hint) = source_id_hint.write() {
                // Boundary: the hint is a persisted String, so materialize
                // the chunk's Arc<str> id here (FA-4b).
                *hint = Some(chunk.source_id.to_string());
            }

            let pcm_bytes = f32_to_pcm_bytes(&chunk.data);
            let audio_event = AudioEvent::builder()
                .audio_chunk(Blob::new(pcm_bytes))
                .build();
            audio_tx
                .blocking_send(Ok(AudioStream::AudioEvent(audio_event)))
                .map_err(|_| chunk)
        }

        // Drain carryover from the previous connection FIRST so audio pulled
        // off the capture channel during the last stop window plays into the
        // fresh stream in order, ahead of new capture.
        while let Some(chunk) = pop_carryover(&carryover) {
            if !active_task.load(Ordering::Relaxed) || !is_transcribing.load(Ordering::Relaxed) {
                // Put it back for the next forwarder; user-stop teardown drops
                // the queue with the session.
                let mut queue = carryover
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                queue.push_front(chunk);
                return;
            }
            if let Err(chunk) = deliver(&audio_tx, &source_id_hint, chunk) {
                // The popped chunk is the OLDEST carryover item — requeue at
                // the FRONT so it stays ahead of newer carryover, preserving
                // capture order for the next connection (CodeRabbit on PR #83).
                log::info!("AWS Transcribe: audio channel closed during carryover drain");
                let mut queue = carryover
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                queue.push_front(chunk);
                return;
            }
        }

        loop {
            if !is_transcribing.load(Ordering::Relaxed) {
                break;
            }
            // Cleared by the drive loop on disconnect so the forwarder winds
            // down deterministically (within one poll) even when audio is idle.
            if !active_task.load(Ordering::Relaxed) {
                break;
            }

            match audio_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(chunk) => {
                    // Re-check the stop flag AFTER recv: a stop can race the
                    // blocking recv, and this chunk has already been consumed
                    // from the shared capture buffer. Sending it into the
                    // abandoned stream would silently open an audio gap around
                    // the reconnect — park it in the carryover queue for the
                    // next stream instead (Codex P2 on PR #83).
                    if !active_task.load(Ordering::Relaxed) {
                        push_carryover(&carryover, chunk);
                        break;
                    }

                    if let Err(chunk) = deliver(&audio_tx, &source_id_hint, chunk) {
                        // Stream channel died mid-send — same reasoning: the
                        // chunk must survive to the next connection. push_back
                        // is correct here (not push_front): the drain above
                        // emptied the queue before this loop started, so this
                        // chunk can only be the newest item.
                        log::info!("AWS Transcribe: audio channel closed");
                        push_carryover(&carryover, chunk);
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    log::info!("AWS Transcribe: audio source disconnected");
                    break;
                }
            }
        }
        drop(audio_tx);
    });

    AudioForwarder { active, handle }
}

/// Advance the shared reconnect ladder by one step: emit `Reconnecting`, then
/// sleep the backoff in 100ms increments so an `is_transcribing` clear cancels
/// promptly (matching the WebSocket siblings' cancellation semantics).
async fn advance_reconnect_ladder(
    reconnect_attempts: &mut u32,
    is_transcribing: &Arc<AtomicBool>,
    on_status: &mut impl FnMut(AwsTranscribeStatus),
) -> LadderStep {
    match next_reconnect_step(*reconnect_attempts) {
        ReconnectStep::Retry {
            attempt,
            backoff_secs,
        } => {
            *reconnect_attempts = attempt;
            on_status(AwsTranscribeStatus::Reconnecting {
                attempt,
                backoff_secs,
            });
            log::info!("AWS Transcribe: reconnecting (attempt {attempt}, backoff {backoff_secs}s)");

            let total = Duration::from_secs(backoff_secs);
            let mut slept = Duration::ZERO;
            while slept < total {
                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!("AWS Transcribe: user stopped during reconnect backoff");
                    return LadderStep::Cancelled;
                }
                let step = Duration::from_millis(100).min(total - slept);
                tokio::time::sleep(step).await;
                slept += step;
            }

            if !is_transcribing.load(Ordering::Relaxed) {
                return LadderStep::Cancelled;
            }
            LadderStep::Continue
        }
        ReconnectStep::GiveUp { .. } => LadderStep::GiveUp,
    }
}

/// Run an AWS Transcribe streaming session. Blocking — meant for a dedicated thread.
///
/// Reads ProcessedAudioChunks from the receiver, streams them to AWS Transcribe,
/// and returns TranscriptSegments via the provided callback.
pub fn run_aws_transcribe_session(
    audio_rx: Receiver<ProcessedAudioChunk>,
    is_transcribing: Arc<AtomicBool>,
    config: impl AwsTranscribeSessionConfig + Send + 'static,
    on_transcript: impl FnMut(AwsTranscribeFinal) + Send + 'static,
    on_partial: impl FnMut(AwsTranscribePartial) + Send + 'static,
    on_status: impl FnMut(AwsTranscribeStatus) + Send + 'static,
) -> Result<(), RuntimeDiagnostic> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_source| {
            RuntimeDiagnostic::internal(
                RuntimeDiagnosticContext::new(RuntimeOperation::Transcription)
                    .with_transport(RuntimeTransport::Native),
            )
        })?;

    rt.block_on(async {
        run_streaming_session(
            audio_rx,
            is_transcribing,
            config,
            on_transcript,
            on_partial,
            on_status,
        )
        .await
    })
}

async fn run_streaming_session(
    audio_rx: Receiver<ProcessedAudioChunk>,
    is_transcribing: Arc<AtomicBool>,
    config: impl AwsTranscribeSessionConfig + Send + 'static,
    mut on_transcript: impl FnMut(AwsTranscribeFinal) + Send + 'static,
    mut on_partial: impl FnMut(AwsTranscribePartial) + Send + 'static,
    mut on_status: impl FnMut(AwsTranscribeStatus) + Send + 'static,
) -> Result<(), RuntimeDiagnostic> {
    if config
        .content_egress_policy()
        .check_audio("asr.aws_transcribe")
        .is_err()
    {
        return Err(sdk_runtime_diagnostic(
            RuntimeErrorCode::PolicyBlocked,
            false,
            RuntimeSafeRecoveryAction::ReviewPolicy,
            None,
        ));
    }

    let sdk_config = build_aws_sdk_config(config.region(), config.credential_source().clone())
        .await
        .map_err(|_source| unknown_sdk_diagnostic())?;
    let client = transcribe::Client::new(&sdk_config);

    let language_code = parse_language_code_or_warn(config.language_code());
    let enable_diarization = config.enable_diarization();

    // Persisted across reconnects so the source hint learned before a drop
    // still labels transcripts that arrive on the re-established stream.
    let source_id_hint = Arc::new(RwLock::new(None::<String>));
    // Chunks the previous connection consumed from the capture channel but
    // could not deliver (stop raced recv, or the stream died mid-send). Drained
    // first into the next stream so a reconnect never opens an audio gap.
    let carryover: CarryoverQueue = Arc::new(Mutex::new(VecDeque::new()));

    let mut reconnect_attempts: u32 = 0;
    let mut connected_once = false;

    loop {
        // ---- OPEN (or re-open) the streaming transcription ----
        let (audio_tx, audio_stream_rx) = tokio::sync::mpsc::channel::<
            Result<AudioStream, transcribe::types::error::AudioStreamError>,
        >(16);

        // Import via the SDK's re-export, NOT aws_smithy_http directly: the
        // SDK enables smithy's `event-stream` feature and may resolve a
        // different (semver-incompatible 0.x) smithy-http than a direct dep,
        // splitting the graph into two crate instances (PR #97 CI incident).
        let audio_stream: transcribe::primitives::event_stream::EventStreamSender<
            AudioStream,
            transcribe::types::error::AudioStreamError,
        > = transcribe::primitives::event_stream::EventStreamSender::from(
            tokio_stream::wrappers::ReceiverStream::new(audio_stream_rx),
        );

        let mut builder = client
            .start_stream_transcription()
            .language_code(language_code.clone())
            .media_sample_rate_hertz(16000)
            .media_encoding(MediaEncoding::Pcm)
            .audio_stream(audio_stream);

        if enable_diarization {
            builder = builder.show_speaker_label(true);
        }

        let mut output = match builder.send().await {
            Ok(output) => output,
            Err(e) => {
                let diagnostic = start_stream_sdk_diagnostic(&e);
                if !connected_once {
                    // First connect failure surfaces immediately, matching the
                    // WebSocket siblings' connect() contract.
                    return Err(diagnostic);
                }
                if !is_recoverable_sdk_error(&e) {
                    log::error!("AWS Transcribe: unrecoverable reconnect open error {diagnostic}");
                    return Err(diagnostic);
                }
                log::warn!("AWS Transcribe: reconnect open failed (recoverable) {diagnostic}");
                match advance_reconnect_ladder(
                    &mut reconnect_attempts,
                    &is_transcribing,
                    &mut on_status,
                )
                .await
                {
                    LadderStep::Continue => continue,
                    LadderStep::Cancelled => return Ok(()),
                    LadderStep::GiveUp => return Err(diagnostic),
                }
            }
        };

        if connected_once {
            // Deliberately NOT resetting `reconnect_attempts` here: AWS merely
            // ACCEPTED the stream. On a flapping link (opens but cannot
            // sustain) a reset-on-accept would re-enter every failure at
            // attempt 1 and loop at the 1s rung forever, never reaching the
            // documented give-up. The budget resets below only after the
            // stream stays healthy for HEALTHY_STREAM_RESET_SECS (Codex P2).
            on_status(AwsTranscribeStatus::Reconnected);
            log::info!("AWS Transcribe: reconnected");
        } else {
            log::info!("AWS Transcribe: streaming session started");
        }
        connected_once = true;
        let stream_established_at = std::time::Instant::now();

        let forwarder = spawn_audio_forwarder(
            audio_rx.clone(),
            audio_tx,
            Arc::clone(&is_transcribing),
            Arc::clone(&source_id_hint),
            Arc::clone(&carryover),
        );

        // ---- DRIVE the connected stream ----
        let outcome = loop {
            let event = match output.transcript_result_stream.recv().await {
                Ok(Some(event)) => event,
                Ok(None) => {
                    // Stream ended: a clean stop if the user already cleared
                    // `is_transcribing`, otherwise an unexpected server close
                    // (idle/duration limit) that warrants re-establishment.
                    break if is_transcribing.load(Ordering::Relaxed) {
                        DriveOutcome::Recoverable(sdk_runtime_diagnostic(
                            RuntimeErrorCode::NetworkUnreachable,
                            true,
                            RuntimeSafeRecoveryAction::CheckNetwork,
                            None,
                        ))
                    } else {
                        DriveOutcome::Completed
                    };
                }
                Err(e) => {
                    let diagnostic = transcript_stream_sdk_diagnostic(&e);
                    break if is_recoverable_sdk_error(&e) {
                        DriveOutcome::Recoverable(diagnostic)
                    } else {
                        DriveOutcome::Unrecoverable(diagnostic)
                    };
                }
            };

            if !is_transcribing.load(Ordering::Relaxed) {
                break DriveOutcome::UserStopped;
            }

            if let transcribe::types::TranscriptResultStream::TranscriptEvent(ev) = event
                && let Some(transcript) = ev.transcript
            {
                for result in transcript.results.unwrap_or_default() {
                    let source_id = source_hint_or_fallback(&source_id_hint);

                    if result.is_partial() {
                        if let Some(partial) = partial_from_result(&result, source_id) {
                            on_partial(partial);
                        }
                        continue;
                    }

                    for segment in final_segments_from_result(&result, &source_id) {
                        on_transcript(segment);
                    }
                }
            }
        };

        // Wind the forwarder down before deciding — a fresh forwarder is spawned
        // on the next iteration if we reconnect. Ordering matters (CodeRabbit on
        // PR #83): stop flag first, then DROP the SDK stream, then join. If the
        // forwarder is parked in a backpressured `blocking_send` (channel full,
        // request-body receiver still alive), the stop flag alone never wakes
        // it and `join` would hang the session shutdown forever. Dropping
        // `output` tears down the in-flight request (and with it the
        // request-body receiver), so the blocked send errors out and the
        // mid-send-failure path parks the in-flight chunk in carryover.
        forwarder.stop();
        drop(output);
        forwarder.join().await;

        if !should_reconnect(&outcome) {
            return match outcome {
                DriveOutcome::UserStopped | DriveOutcome::Completed => {
                    log::info!("AWS Transcribe: streaming session ended ({outcome:?})");
                    Ok(())
                }
                DriveOutcome::Unrecoverable(diagnostic) => {
                    log::error!("AWS Transcribe: unrecoverable stream error {diagnostic}");
                    Err(diagnostic)
                }
                DriveOutcome::Recoverable(_) => unreachable!("guarded by should_reconnect"),
            };
        }

        if !is_transcribing.load(Ordering::Relaxed) {
            return Ok(());
        }

        let reconnect_diagnostic = match &outcome {
            DriveOutcome::Recoverable(diagnostic) => diagnostic.clone(),
            _ => unreachable!("reconnect path requires a recoverable outcome"),
        };
        log::warn!("AWS Transcribe: recoverable stream error, reconnecting {reconnect_diagnostic}");

        // Earn a fresh reconnect budget only after sustained health. An
        // accept-then-immediate-drop keeps climbing the ladder toward the
        // documented 1/2/5/10 give-up instead of looping at attempt 1.
        let healthy_for = stream_established_at.elapsed();
        if should_reset_reconnect_budget(healthy_for) {
            reconnect_attempts = 0;
        } else {
            log::warn!(
                "AWS Transcribe: stream dropped after {}s (< {HEALTHY_STREAM_RESET_SECS}s healthy threshold); keeping reconnect budget at attempt {reconnect_attempts}",
                healthy_for.as_secs()
            );
        }

        match advance_reconnect_ladder(&mut reconnect_attempts, &is_transcribing, &mut on_status)
            .await
        {
            LadderStep::Continue => continue,
            LadderStep::Cancelled => return Ok(()),
            LadderStep::GiveUp => return Err(reconnect_diagnostic),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::reconnect::DEFAULT_BACKOFF_SECONDS;
    use aws_sdk_transcribestreaming::types::Item;
    use crossbeam_channel::unbounded;
    use std::time::Duration;

    fn alt(text: &str, confidence: f64, speaker: Option<&str>) -> Alternative {
        let mut item = Item::builder().confidence(confidence);
        if let Some(speaker) = speaker {
            item = item.speaker(speaker);
        }

        Alternative::builder()
            .transcript(text)
            .items(item.build())
            .build()
    }

    fn test_config() -> AwsTranscribeConfig {
        AwsTranscribeConfig {
            region: "not-a-real-aws-region-private-test".to_string(),
            language_code: "en-US".to_string(),
            credential_source: AwsCredentialSource::DefaultChain,
            enable_diarization: true,
        }
    }

    fn test_chunk() -> ProcessedAudioChunk {
        ProcessedAudioChunk {
            source_id: Arc::<str>::from("mic-private-source"),
            data: vec![0.5; 7_919],
            sample_rate: 16_000,
            num_frames: 7_919,
            timestamp: Some(Duration::from_millis(32)),
        }
    }

    fn assert_runtime_diagnostic(
        diagnostic: &RuntimeDiagnostic,
        code: RuntimeErrorCode,
        retryable: bool,
        recovery_action: RuntimeSafeRecoveryAction,
        status_class: Option<RuntimeStatusClass>,
    ) {
        let RuntimeDiagnostic::Runtime(detail) = diagnostic else {
            panic!("expected runtime diagnostic, got {diagnostic:?}");
        };
        assert_eq!(detail.code, code);
        assert_eq!(detail.retryable, retryable);
        assert_eq!(detail.recovery_action, recovery_action);
        assert_eq!(detail.context.operation, RuntimeOperation::Transcription);
        assert_eq!(detail.context.transport, Some(RuntimeTransport::Sdk));
        assert_eq!(detail.context.status_class, status_class);
    }

    fn assert_diagnostic_excludes_canaries(diagnostic: &RuntimeDiagnostic) {
        let serialized = serde_json::to_string(diagnostic).expect("diagnostic JSON");
        let captured_log = format!("AWS Transcribe error diagnostic={diagnostic}");
        for forbidden in [
            "provider raw text should not leak",
            "patient said private diagnosis",
            "mic-private-source",
            "/private/aws/profile/path",
            "request-id-canary",
            "AKIA1234567890ABCDEF",
            "ASIA1234567890ABCDEF",
            "aws-secret-looking-value",
            "message_len",
            "body_len",
            "audio_len=7919",
        ] {
            for (sink, value) in [
                ("serialized event", &serialized),
                ("captured log", &captured_log),
            ] {
                assert!(
                    !value.contains(forbidden),
                    "AWS diagnostic leaked {forbidden} through {sink}: {value}"
                );
            }
        }
    }

    #[test]
    fn source_hint_uses_fallback_until_audio_source_arrives() {
        let hint = Arc::new(RwLock::new(None::<String>));
        assert_eq!(
            source_hint_or_fallback(&hint),
            AWS_TRANSCRIBE_SOURCE_FALLBACK
        );

        *hint.write().unwrap() = Some("process:123".to_string());
        assert_eq!(source_hint_or_fallback(&hint), "process:123");
    }

    #[test]
    fn f32_to_pcm_bytes_uses_shared_s16le_contract() {
        let bytes = f32_to_pcm_bytes(&[-1.0, 0.0, 1.0, f32::NAN]);
        let values: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();

        assert_eq!(values, vec![i16::MIN, 0, i16::MAX, 0]);
    }

    #[test]
    fn aws_transcribe_content_policy_defaults_to_explicit_policy_required() {
        let config = test_config();

        let error = config
            .content_egress_policy()
            .check_audio("asr.aws_transcribe")
            .unwrap_err();

        assert!(error.contains("Privacy policy blocked audio egress"));
        assert!(error.contains("asr.aws_transcribe"));
        assert!(error.contains(EXPLICIT_POLICY_REQUIRED));
    }

    #[test]
    fn aws_transcribe_explicit_allow_policy_permits_audio_guard() {
        let config = test_config().with_content_egress_policy(ProviderContentEgressPolicy::allow());

        assert!(
            config
                .content_egress_policy()
                .check_audio("asr.aws_transcribe")
                .is_ok()
        );
    }

    #[test]
    fn default_policy_rejects_audio_before_streaming_setup() {
        let (tx, rx) = unbounded();
        tx.send(test_chunk()).unwrap();
        let unread_rx = rx.clone();
        let config = test_config();

        let error = run_aws_transcribe_session(
            rx,
            Arc::new(AtomicBool::new(true)),
            config,
            |_: AwsTranscribeFinal| {},
            |_: AwsTranscribePartial| {},
            |_: AwsTranscribeStatus| {},
        )
        .unwrap_err();

        assert_runtime_diagnostic(
            &error,
            RuntimeErrorCode::PolicyBlocked,
            false,
            RuntimeSafeRecoveryAction::ReviewPolicy,
            None,
        );
        assert_diagnostic_excludes_canaries(&error);
        assert_eq!(
            unread_rx.len(),
            1,
            "default policy should return before consuming queued PCM"
        );
    }

    #[test]
    fn blocked_policy_rejects_audio_before_streaming_setup() {
        let (tx, rx) = unbounded();
        tx.send(test_chunk()).unwrap();
        let unread_rx = rx.clone();
        let config = test_config()
            .with_content_egress_policy(ProviderContentEgressPolicy::block("local_only"));

        let error = run_aws_transcribe_session(
            rx,
            Arc::new(AtomicBool::new(true)),
            config,
            |_: AwsTranscribeFinal| {},
            |_: AwsTranscribePartial| {},
            |_: AwsTranscribeStatus| {},
        )
        .unwrap_err();

        assert_runtime_diagnostic(
            &error,
            RuntimeErrorCode::PolicyBlocked,
            false,
            RuntimeSafeRecoveryAction::ReviewPolicy,
            None,
        );
        assert_diagnostic_excludes_canaries(&error);
        assert_eq!(
            unread_rx.len(),
            1,
            "blocked policy should return before consuming queued PCM"
        );
    }

    #[test]
    fn blocked_policy_error_redacts_aws_audio_and_source_values() {
        let (tx, rx) = unbounded();
        tx.send(test_chunk()).unwrap();
        let config = test_config()
            .with_content_egress_policy(ProviderContentEgressPolicy::block("local_only"));

        let error = run_aws_transcribe_session(
            rx,
            Arc::new(AtomicBool::new(true)),
            config,
            |_: AwsTranscribeFinal| {},
            |_: AwsTranscribePartial| {},
            |_: AwsTranscribeStatus| {},
        )
        .unwrap_err();

        let serialized = serde_json::to_string(&error).expect("policy diagnostic JSON");
        for forbidden in [
            "0.5",
            "-0.25",
            "7919",
            "patient said private diagnosis",
            "mic-private-source",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "privacy error leaked {forbidden}: {serialized}"
            );
        }
    }

    #[test]
    fn outer_sdk_variants_map_structurally_without_source_data() {
        type TestError = transcribe::error::SdkError<StartStreamTranscriptionError, ()>;
        let source = || {
            Box::<dyn std::error::Error + Send + Sync>::from(std::io::Error::other(
                "provider raw text should not leak /private/aws/profile/path request-id-canary audio_len=7919",
            ))
        };

        let cases = [
            (
                TestError::construction_failure(source()),
                RuntimeErrorCode::Internal,
                false,
                RuntimeSafeRecoveryAction::None,
            ),
            (
                TestError::timeout_error(source()),
                RuntimeErrorCode::Timeout,
                true,
                RuntimeSafeRecoveryAction::Retry,
            ),
            (
                TestError::dispatch_failure(transcribe::error::ConnectorError::io(source())),
                RuntimeErrorCode::NetworkUnreachable,
                true,
                RuntimeSafeRecoveryAction::CheckNetwork,
            ),
            (
                TestError::response_error(source(), ()),
                RuntimeErrorCode::InvalidResponse,
                true,
                RuntimeSafeRecoveryAction::Retry,
            ),
        ];

        for (error, code, retryable, recovery_action) in cases {
            let diagnostic = start_stream_sdk_diagnostic(&error);
            assert_runtime_diagnostic(&diagnostic, code, retryable, recovery_action, None);
            assert_diagnostic_excludes_canaries(&diagnostic);
        }
    }

    #[test]
    fn generated_start_service_variants_map_structurally() {
        let message = "provider raw text should not leak request-id-canary audio_len=7919";
        let cases = [
            (
                StartStreamTranscriptionError::BadRequestException(
                    transcribe::types::error::BadRequestException::builder()
                        .message(message)
                        .build(),
                ),
                RuntimeErrorCode::RequestRejected,
                false,
                RuntimeSafeRecoveryAction::ReviewConfiguration,
                Some(RuntimeStatusClass::ClientError),
            ),
            (
                StartStreamTranscriptionError::ConflictException(
                    transcribe::types::error::ConflictException::builder()
                        .message(message)
                        .build(),
                ),
                RuntimeErrorCode::RequestRejected,
                false,
                RuntimeSafeRecoveryAction::ReviewConfiguration,
                Some(RuntimeStatusClass::ClientError),
            ),
            (
                StartStreamTranscriptionError::InternalFailureException(
                    transcribe::types::error::InternalFailureException::builder()
                        .message(message)
                        .build(),
                ),
                RuntimeErrorCode::Internal,
                true,
                RuntimeSafeRecoveryAction::Retry,
                Some(RuntimeStatusClass::ServerError),
            ),
            (
                StartStreamTranscriptionError::LimitExceededException(
                    transcribe::types::error::LimitExceededException::builder()
                        .message(message)
                        .build(),
                ),
                RuntimeErrorCode::CapacityExhausted,
                true,
                RuntimeSafeRecoveryAction::RetryAfterDelay,
                Some(RuntimeStatusClass::ClientError),
            ),
            (
                StartStreamTranscriptionError::ServiceUnavailableException(
                    transcribe::types::error::ServiceUnavailableException::builder()
                        .message(message)
                        .build(),
                ),
                RuntimeErrorCode::ProviderUnavailable,
                true,
                RuntimeSafeRecoveryAction::RetryAfterDelay,
                Some(RuntimeStatusClass::ServerError),
            ),
        ];

        for (error, code, retryable, recovery_action, status_class) in cases {
            let diagnostic = start_stream_service_diagnostic(&error);
            assert_runtime_diagnostic(&diagnostic, code, retryable, recovery_action, status_class);
            assert_diagnostic_excludes_canaries(&diagnostic);
        }
    }

    #[test]
    fn generated_transcript_service_variants_map_structurally() {
        let message =
            "transcript=patient said private diagnosis ASIA1234567890ABCDEF audio_len=7919";
        let cases = [
            (
                TranscriptResultStreamError::BadRequestException(
                    transcribe::types::error::BadRequestException::builder()
                        .message(message)
                        .build(),
                ),
                RuntimeErrorCode::RequestRejected,
            ),
            (
                TranscriptResultStreamError::ConflictException(
                    transcribe::types::error::ConflictException::builder()
                        .message(message)
                        .build(),
                ),
                RuntimeErrorCode::RequestRejected,
            ),
            (
                TranscriptResultStreamError::InternalFailureException(
                    transcribe::types::error::InternalFailureException::builder()
                        .message(message)
                        .build(),
                ),
                RuntimeErrorCode::Internal,
            ),
            (
                TranscriptResultStreamError::LimitExceededException(
                    transcribe::types::error::LimitExceededException::builder()
                        .message(message)
                        .build(),
                ),
                RuntimeErrorCode::CapacityExhausted,
            ),
            (
                TranscriptResultStreamError::ServiceUnavailableException(
                    transcribe::types::error::ServiceUnavailableException::builder()
                        .message(message)
                        .build(),
                ),
                RuntimeErrorCode::ProviderUnavailable,
            ),
        ];

        for (error, expected_code) in cases {
            let diagnostic = transcript_stream_service_diagnostic(&error);
            let RuntimeDiagnostic::Runtime(detail) = &diagnostic else {
                panic!("expected runtime diagnostic");
            };
            assert_eq!(detail.code, expected_code);
            assert_diagnostic_excludes_canaries(&diagnostic);
        }
    }

    #[test]
    fn generic_service_metadata_uses_only_the_exact_code_allowlist() {
        let cases = [
            (
                "BadRequestException",
                RuntimeErrorCode::RequestRejected,
                false,
                RuntimeSafeRecoveryAction::ReviewConfiguration,
                Some(RuntimeStatusClass::ClientError),
            ),
            (
                "ConflictException",
                RuntimeErrorCode::RequestRejected,
                false,
                RuntimeSafeRecoveryAction::ReviewConfiguration,
                Some(RuntimeStatusClass::ClientError),
            ),
            (
                "InternalFailureException",
                RuntimeErrorCode::Internal,
                true,
                RuntimeSafeRecoveryAction::Retry,
                Some(RuntimeStatusClass::ServerError),
            ),
            (
                "LimitExceededException",
                RuntimeErrorCode::CapacityExhausted,
                true,
                RuntimeSafeRecoveryAction::RetryAfterDelay,
                Some(RuntimeStatusClass::ClientError),
            ),
            (
                "ServiceUnavailableException",
                RuntimeErrorCode::ProviderUnavailable,
                true,
                RuntimeSafeRecoveryAction::RetryAfterDelay,
                Some(RuntimeStatusClass::ServerError),
            ),
        ];

        for (metadata_code, code, retryable, recovery_action, status_class) in cases {
            let error = StartStreamTranscriptionError::generic(
                transcribe::error::ErrorMetadata::builder()
                    .code(metadata_code)
                    .message("provider raw text should not leak request-id-canary audio_len=7919")
                    .custom("request_id", "aws-secret-looking-value")
                    .build(),
            );

            let diagnostic = start_stream_service_diagnostic(&error);
            assert_runtime_diagnostic(&diagnostic, code, retryable, recovery_action, status_class);
            assert_diagnostic_excludes_canaries(&diagnostic);
            assert!(
                !serde_json::to_string(&diagnostic)
                    .expect("diagnostic JSON")
                    .contains(metadata_code)
            );
        }

        let near_match = "LimitExceededExceptionWithPrivateSuffix";
        let error = StartStreamTranscriptionError::generic(
            transcribe::error::ErrorMetadata::builder()
                .code(near_match)
                .message("provider raw text should not leak request-id-canary audio_len=7919")
                .custom("request_id", "aws-secret-looking-value")
                .build(),
        );
        let diagnostic = start_stream_service_diagnostic(&error);

        assert_runtime_diagnostic(
            &diagnostic,
            RuntimeErrorCode::Internal,
            false,
            RuntimeSafeRecoveryAction::None,
            None,
        );
        assert_diagnostic_excludes_canaries(&diagnostic);
        assert!(
            !serde_json::to_string(&diagnostic)
                .expect("diagnostic JSON")
                .contains(near_match)
        );
    }

    #[test]
    fn unknown_service_variant_is_conservative_and_discards_metadata() {
        let error = StartStreamTranscriptionError::unhandled(std::io::Error::other(
            "provider raw text should not leak /private/aws/profile/path request-id-canary audio_len=7919",
        ));

        let diagnostic = start_stream_service_diagnostic(&error);

        assert_runtime_diagnostic(
            &diagnostic,
            RuntimeErrorCode::Internal,
            false,
            RuntimeSafeRecoveryAction::None,
            None,
        );
        assert_diagnostic_excludes_canaries(&diagnostic);
    }

    #[test]
    fn partial_result_is_normalized_with_source_and_timing() {
        let result = transcribe::types::Result::builder()
            .is_partial(true)
            .result_id("result-partial-1")
            .start_time(1.25)
            .end_time(2.5)
            .alternatives(alt(" hello aws ", 0.75, None))
            .build();

        let partial = partial_from_result(&result, "mic".to_string()).unwrap();

        assert_eq!(partial.source_id, "mic");
        assert_eq!(
            partial.provider_item_id.as_deref(),
            Some("result-partial-1")
        );
        assert_eq!(partial.text, "hello aws");
        assert_eq!(partial.start_time, 1.25);
        assert_eq!(partial.end_time, 2.5);
        assert!((partial.confidence - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn final_result_preserves_source_and_speaker_label() {
        let result = transcribe::types::Result::builder()
            .is_partial(false)
            .result_id("result-final-1")
            .start_time(3.0)
            .end_time(4.0)
            .alternatives(alt(" final text ", 0.9, Some("spk_0")))
            .build();

        let segments = final_segments_from_result(&result, "system");

        assert_eq!(segments.len(), 1);
        assert_eq!(
            segments[0].provider_item_id.as_deref(),
            Some("result-final-1")
        );
        assert_eq!(segments[0].segment.source_id, "system");
        assert_eq!(segments[0].segment.speaker_id.as_deref(), Some("spk_0"));
        assert_eq!(segments[0].segment.speaker_label.as_deref(), Some("spk_0"));
        assert_eq!(segments[0].segment.text, "final text");
    }

    // -----------------------------------------------------------------------
    // Reconnect ladder (M1 / audio-graph-35de)
    // -----------------------------------------------------------------------

    #[test]
    fn recoverable_error_kinds_retry_transport_only() {
        // Transport-level failures are recoverable and warrant a reconnect.
        assert!(is_recoverable_error_kind("dispatch_failure"));
        assert!(is_recoverable_error_kind("timeout"));
        assert!(is_recoverable_error_kind("response_error"));
        // Service/construction/unknown errors will not clear on a retry.
        assert!(!is_recoverable_error_kind("service_error"));
        assert!(!is_recoverable_error_kind("construction_failure"));
        assert!(!is_recoverable_error_kind("unknown"));
    }

    #[test]
    fn should_reconnect_only_on_recoverable_outcome() {
        // Only the recoverable transport outcome retries; user-stop, clean
        // completion, and unrecoverable errors end the session.
        assert!(should_reconnect(&DriveOutcome::Recoverable(
            unknown_sdk_diagnostic()
        )));
        assert!(!should_reconnect(&DriveOutcome::UserStopped));
        assert!(!should_reconnect(&DriveOutcome::Completed));
        assert!(!should_reconnect(&DriveOutcome::Unrecoverable(
            unknown_sdk_diagnostic()
        )));
    }

    #[test]
    fn language_code_parses_known_and_falls_back_on_unknown() {
        // A supported code round-trips to its enum variant.
        assert_eq!(
            parse_language_code_or_warn("de-DE"),
            transcribe::types::LanguageCode::DeDe
        );
        assert_eq!(
            parse_language_code_or_warn("en-US"),
            transcribe::types::LanguageCode::EnUs
        );
        // A typo'd / unsupported code coerces to en-US instead of silently
        // forwarding an `Unknown(..)` variant to AWS (review m4). The previous
        // `.parse().unwrap_or(EnUs)` never fired because `FromStr` is Infallible.
        assert_eq!(
            parse_language_code_or_warn("de-DEE"),
            transcribe::types::LanguageCode::EnUs
        );
        assert_eq!(
            parse_language_code_or_warn(""),
            transcribe::types::LanguageCode::EnUs
        );
    }

    #[test]
    fn recoverable_sdk_error_classifies_transport_vs_service() {
        // A timeout (transport-level) is the canonical recoverable case.
        let timeout =
            transcribe::error::SdkError::<StartStreamTranscriptionError, ()>::timeout_error(Box::<
                dyn std::error::Error + Send + Sync,
            >::from(
                "read timed out",
            ));
        assert!(is_recoverable_sdk_error(&timeout));

        // A service error (e.g. BadRequest) is not recoverable.
        let service =
            transcribe::error::SdkError::<StartStreamTranscriptionError, ()>::service_error(
                StartStreamTranscriptionError::BadRequestException(
                    transcribe::types::error::BadRequestException::builder()
                        .message("bad request")
                        .build(),
                ),
                (),
            );
        assert!(!is_recoverable_sdk_error(&service));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reconnect_ladder_emits_backoff_then_continues() {
        let is_transcribing = Arc::new(AtomicBool::new(true));
        let mut attempts: u32 = 0;
        let mut statuses = Vec::new();

        let step =
            advance_reconnect_ladder(&mut attempts, &is_transcribing, &mut |s| statuses.push(s))
                .await;

        assert!(matches!(step, LadderStep::Continue));
        assert_eq!(attempts, 1);
        assert_eq!(
            statuses,
            vec![AwsTranscribeStatus::Reconnecting {
                attempt: 1,
                backoff_secs: 1
            }]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reconnect_ladder_cancels_when_user_stops_during_backoff() {
        let is_transcribing = Arc::new(AtomicBool::new(false));
        let mut attempts: u32 = 0;
        let mut statuses = Vec::new();

        // With `is_transcribing` already cleared, the very first backoff poll
        // must cancel rather than sleep out the full ladder step.
        let step =
            advance_reconnect_ladder(&mut attempts, &is_transcribing, &mut |s| statuses.push(s))
                .await;

        assert!(matches!(step, LadderStep::Cancelled));
        // The Reconnecting notification still fired before the cancel.
        assert_eq!(statuses.len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reconnect_ladder_gives_up_after_schedule_exhausted() {
        let is_transcribing = Arc::new(AtomicBool::new(true));
        // Start at the full budget so the next step exhausts the ladder (the
        // cold-restart tail lengthened it — review m1).
        let budget = DEFAULT_BACKOFF_SECONDS.len() as u32;
        let mut attempts: u32 = budget;
        let mut statuses = Vec::new();

        let step =
            advance_reconnect_ladder(&mut attempts, &is_transcribing, &mut |s| statuses.push(s))
                .await;

        assert!(matches!(step, LadderStep::GiveUp));
        // No Reconnecting emitted once the schedule is exhausted.
        assert!(statuses.is_empty());
    }

    #[test]
    fn reconnect_ladder_backoff_matches_shared_schedule() {
        // The AWS path rides the same shared ladder as the WS siblings: fast head
        // (review n2) plus the cold-restart tail (review m1).
        assert_eq!(backoff_for_attempt(1), Some(1));
        assert_eq!(backoff_for_attempt(4), Some(10));
        assert_eq!(backoff_for_attempt(5), Some(20));
        assert_eq!(backoff_for_attempt(11), None);
    }

    #[test]
    fn reconnect_budget_resets_only_after_sustained_health() {
        // Accepting the stream is not enough — the budget resets only once the
        // stream has stayed healthy for the threshold (Codex P2 on PR #83).
        assert!(!should_reset_reconnect_budget(Duration::ZERO));
        assert!(!should_reset_reconnect_budget(Duration::from_secs(1)));
        assert!(!should_reset_reconnect_budget(Duration::from_secs(
            HEALTHY_STREAM_RESET_SECS - 1
        )));
        assert!(should_reset_reconnect_budget(Duration::from_secs(
            HEALTHY_STREAM_RESET_SECS
        )));
        assert!(should_reset_reconnect_budget(Duration::from_secs(300)));
    }

    /// Codex P2 on PR #83: a flapping link (AWS accepts the reconnect but the
    /// stream drops immediately) must climb the ladder to give-up rather than
    /// looping at attempt 1. Mirrors the production loop's budget policy: the
    /// budget resets only when the stream was healthy ≥ the threshold; each
    /// immediate drop advances the ladder exactly once. The ladder is now the
    /// longer shared cold-restart schedule (review m1).
    #[test]
    fn flapping_link_exhausts_ladder_instead_of_looping_at_attempt_one() {
        let mut reconnect_attempts: u32 = 0;
        let mut backoffs = Vec::new();

        let attempted = loop {
            // Every accept is followed by an immediate drop: healthy for ~1s,
            // far below the reset threshold — the budget must NOT reset.
            let healthy_for = Duration::from_secs(1);
            if should_reset_reconnect_budget(healthy_for) {
                reconnect_attempts = 0;
            }
            match next_reconnect_step(reconnect_attempts) {
                ReconnectStep::Retry {
                    attempt,
                    backoff_secs,
                } => {
                    reconnect_attempts = attempt;
                    backoffs.push(backoff_secs);
                }
                ReconnectStep::GiveUp { attempted } => break attempted,
            }
            assert!(
                backoffs.len() <= DEFAULT_BACKOFF_SECONDS.len(),
                "flapping link looped past the ladder instead of giving up: {backoffs:?}"
            );
        };

        assert_eq!(
            backoffs,
            DEFAULT_BACKOFF_SECONDS.to_vec(),
            "ladder must climb the full shared schedule, not loop"
        );
        assert_eq!(
            attempted,
            DEFAULT_BACKOFF_SECONDS.len() as u32,
            "give-up must report the exhausted budget"
        );

        // Contrast: with a sustained-healthy stream the budget resets and the
        // next failure starts over at attempt 1.
        let mut reconnect_attempts: u32 = 3;
        if should_reset_reconnect_budget(Duration::from_secs(HEALTHY_STREAM_RESET_SECS)) {
            reconnect_attempts = 0;
        }
        assert_eq!(
            next_reconnect_step(reconnect_attempts),
            ReconnectStep::Retry {
                attempt: 1,
                backoff_secs: 1
            }
        );
    }

    /// Codex P2 on PR #83: a chunk pulled off the shared capture channel while
    /// the old forwarder is being stopped (or whose stream died mid-send) must
    /// NOT vanish into the abandoned stream — it must reach the next stream
    /// after reconnect via the carryover queue.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn chunk_consumed_during_stop_window_reaches_next_stream() {
        let (stream1_tx, mut stream1_rx) = tokio::sync::mpsc::channel::<
            Result<AudioStream, transcribe::types::error::AudioStreamError>,
        >(16);
        let (capture_tx, capture_rx) = unbounded();
        let is_transcribing = Arc::new(AtomicBool::new(true));
        let hint = Arc::new(RwLock::new(None::<String>));
        let carryover: CarryoverQueue = Arc::new(Mutex::new(VecDeque::new()));

        let forwarder1 = spawn_audio_forwarder(
            capture_rx.clone(),
            stream1_tx,
            Arc::clone(&is_transcribing),
            Arc::clone(&hint),
            Arc::clone(&carryover),
        );

        // Prove the forwarder is live: one chunk flows to stream 1 normally.
        capture_tx.send(test_chunk()).unwrap();
        let delivered = tokio::time::timeout(Duration::from_secs(2), stream1_rx.recv())
            .await
            .expect("first chunk should reach stream 1");
        assert!(delivered.is_some(), "stream 1 should receive the chunk");

        // Reconnect stop window: the stream is abandoned (receiver dropped)
        // while a fresh chunk races the stop flag.
        drop(stream1_rx);
        capture_tx.send(test_chunk()).unwrap();
        forwarder1.stop();
        forwarder1.join().await;

        // Invariant: the racing chunk is never lost — it is either still in
        // the shared capture channel (recv never consumed it) or parked in the
        // carryover queue (consumed during the stop window / dead stream).
        let in_capture = capture_rx.len();
        let in_carryover = carryover
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        assert_eq!(
            in_capture + in_carryover,
            1,
            "chunk lost in the stop window (capture={in_capture}, carryover={in_carryover})"
        );

        // After reconnect, a fresh forwarder must deliver that chunk to the
        // NEW stream (carryover drains first, ahead of new capture).
        let (stream2_tx, mut stream2_rx) = tokio::sync::mpsc::channel::<
            Result<AudioStream, transcribe::types::error::AudioStreamError>,
        >(16);
        let forwarder2 = spawn_audio_forwarder(
            capture_rx.clone(),
            stream2_tx,
            Arc::clone(&is_transcribing),
            hint,
            Arc::clone(&carryover),
        );
        let redelivered = tokio::time::timeout(Duration::from_secs(2), stream2_rx.recv())
            .await
            .expect("stop-window chunk should reach the reconnected stream");
        assert!(
            redelivered.is_some(),
            "reconnected stream must receive the surviving chunk"
        );

        forwarder2.stop();
        forwarder2.join().await;
    }

    /// Carryover chunks from the previous connection drain into the new stream
    /// FIRST, ahead of chunks still queued on the shared capture channel, so
    /// audio stays in order across a reconnect.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn carryover_drains_before_new_capture_after_reconnect() {
        let (stream_tx, mut stream_rx) = tokio::sync::mpsc::channel::<
            Result<AudioStream, transcribe::types::error::AudioStreamError>,
        >(16);
        let (capture_tx, capture_rx) = unbounded();
        let is_transcribing = Arc::new(AtomicBool::new(true));
        let hint = Arc::new(RwLock::new(None::<String>));
        let carryover: CarryoverQueue = Arc::new(Mutex::new(VecDeque::new()));

        // A chunk stranded by the previous connection (distinct amplitude so
        // the order is observable in the PCM payload) …
        let mut stranded = test_chunk();
        stranded.data = vec![1.0, 1.0];
        push_carryover(&carryover, stranded);
        // … and a newer chunk already waiting on the capture channel.
        let mut newer = test_chunk();
        newer.data = vec![-1.0, -1.0];
        capture_tx.send(newer).unwrap();

        let forwarder = spawn_audio_forwarder(
            capture_rx,
            stream_tx,
            Arc::clone(&is_transcribing),
            hint,
            Arc::clone(&carryover),
        );

        let first = tokio::time::timeout(Duration::from_secs(2), stream_rx.recv())
            .await
            .expect("first frame should arrive")
            .expect("stream should stay open");
        let second = tokio::time::timeout(Duration::from_secs(2), stream_rx.recv())
            .await
            .expect("second frame should arrive")
            .expect("stream should stay open");

        let pcm_of = |event: &AudioStream| -> Vec<u8> {
            match event {
                AudioStream::AudioEvent(ev) => ev
                    .audio_chunk()
                    .map(|b| b.as_ref().to_vec())
                    .unwrap_or_default(),
                _ => Vec::new(),
            }
        };
        assert_eq!(
            pcm_of(first.as_ref().expect("audio event")),
            f32_to_pcm_bytes(&[1.0, 1.0]),
            "carryover chunk must drain first"
        );
        assert_eq!(
            pcm_of(second.as_ref().expect("audio event")),
            f32_to_pcm_bytes(&[-1.0, -1.0]),
            "capture-channel chunk must follow the carryover"
        );
        assert!(
            carryover
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "carryover queue should be drained"
        );

        forwarder.stop();
        forwarder.join().await;
    }

    /// CodeRabbit on PR #83 round 3: a forwarder parked inside a backpressured
    /// `blocking_send` (stream channel full, request-body receiver still
    /// alive) cannot be woken by the stop flag alone — `join` would hang the
    /// session shutdown forever. The production stop path is
    /// stop flag → drop stream → join: dropping the receiver errors the
    /// blocked send, the mid-send-failure path parks the in-flight chunk in
    /// carryover, and join completes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn backpressured_forwarder_join_completes_after_stream_drop() {
        // Capacity-1 stream channel that nobody consumes: the first chunk
        // fills the buffer, the second parks the forwarder in blocking_send.
        let (stream_tx, stream_rx) = tokio::sync::mpsc::channel::<
            Result<AudioStream, transcribe::types::error::AudioStreamError>,
        >(1);
        let (capture_tx, capture_rx) = unbounded();
        let is_transcribing = Arc::new(AtomicBool::new(true));
        let hint = Arc::new(RwLock::new(None::<String>));
        let carryover: CarryoverQueue = Arc::new(Mutex::new(VecDeque::new()));

        let forwarder = spawn_audio_forwarder(
            capture_rx.clone(),
            stream_tx,
            Arc::clone(&is_transcribing),
            hint,
            Arc::clone(&carryover),
        );

        let mut filler = test_chunk();
        filler.data = vec![1.0, 1.0];
        capture_tx.send(filler).unwrap();
        let mut parked = test_chunk();
        parked.data = vec![-1.0, -1.0];
        capture_tx.send(parked).unwrap();

        // Wait until the forwarder has consumed both chunks off the capture
        // channel — with the buffer full, it is now parked inside (or about to
        // enter) the backpressured blocking_send for the second chunk.
        let drained = tokio::time::timeout(Duration::from_secs(2), async {
            while !capture_rx.is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            drained.is_ok(),
            "forwarder should drain the capture channel"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Production shutdown ordering: stop flag → drop stream → join.
        forwarder.stop();
        drop(stream_rx);
        tokio::time::timeout(Duration::from_secs(2), forwarder.join())
            .await
            .expect("join must not hang on a backpressured blocking_send");

        // The chunk that was in-flight through blocking_send must survive to
        // the next connection via carryover (mid-send-failure or stop-window
        // path — both park it; which one fires depends on the stop/send race).
        let queue = carryover
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let order: Vec<Vec<f32>> = queue.iter().map(|chunk| chunk.data.clone()).collect();
        assert_eq!(
            order,
            vec![vec![-1.0, -1.0]],
            "the blocked chunk must be parked in carryover, not lost"
        );
    }

    /// CodeRabbit on PR #83: when a deliver fails DURING the carryover drain,
    /// the popped chunk is the OLDEST item and must be requeued at the FRONT —
    /// push_back would put it behind newer carryover and reorder audio.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drain_failure_requeues_oldest_chunk_at_front() {
        // Stream receiver dropped up front: the very first drain deliver fails.
        let (stream_tx, stream_rx) = tokio::sync::mpsc::channel::<
            Result<AudioStream, transcribe::types::error::AudioStreamError>,
        >(16);
        drop(stream_rx);
        let (_capture_tx, capture_rx) = unbounded::<ProcessedAudioChunk>();
        let is_transcribing = Arc::new(AtomicBool::new(true));
        let hint = Arc::new(RwLock::new(None::<String>));
        let carryover: CarryoverQueue = Arc::new(Mutex::new(VecDeque::new()));

        // Two stranded chunks in capture order: oldest (1.0) then newer (-1.0).
        let mut oldest = test_chunk();
        oldest.data = vec![1.0, 1.0];
        push_carryover(&carryover, oldest);
        let mut newer = test_chunk();
        newer.data = vec![-1.0, -1.0];
        push_carryover(&carryover, newer);

        let forwarder = spawn_audio_forwarder(
            capture_rx,
            stream_tx,
            Arc::clone(&is_transcribing),
            hint,
            Arc::clone(&carryover),
        );
        forwarder.join().await;

        // The failed drain must leave the queue in the ORIGINAL order:
        // oldest still first, newer still behind it.
        let queue = carryover
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let order: Vec<Vec<f32>> = queue.iter().map(|chunk| chunk.data.clone()).collect();
        assert_eq!(
            order,
            vec![vec![1.0, 1.0], vec![-1.0, -1.0]],
            "drain failure must requeue the oldest chunk at the FRONT"
        );
    }
}
