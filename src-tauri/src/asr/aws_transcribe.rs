//! AWS Transcribe Streaming ASR integration.
//!
//! Uses the aws-sdk-transcribestreaming crate to stream audio to AWS
//! and receive real-time transcription results with optional speaker diarization.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

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
const AWS_TRANSCRIBE_PROVIDER_ID: &str = "aws_transcribe";

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

struct AwsTranscribeDiagnostic<'a> {
    operation: &'static str,
    category: &'static str,
    error_kind: &'static str,
    code: Option<&'a str>,
    request_id: Option<&'a str>,
    status_code: Option<u16>,
    message_len: Option<usize>,
    body_len: Option<u64>,
}

fn char_len(value: &str) -> usize {
    value.chars().count()
}

fn safe_diagnostic_token(value: Option<&str>) -> String {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return "none".to_string();
    };

    if value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
    {
        value.to_string()
    } else {
        format!("present_len_{}", char_len(value))
    }
}

fn optional_usize(value: Option<usize>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string())
}

fn optional_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string())
}

fn optional_u16(value: Option<u16>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string())
}

fn format_aws_transcribe_diagnostic(diagnostic: AwsTranscribeDiagnostic<'_>) -> String {
    format!(
        "AWS Transcribe error provider={} operation={} category={} error_kind={} code={} request_id={} status_code={} message_len={} body_len={}",
        AWS_TRANSCRIBE_PROVIDER_ID,
        diagnostic.operation,
        diagnostic.category,
        diagnostic.error_kind,
        safe_diagnostic_token(diagnostic.code),
        safe_diagnostic_token(diagnostic.request_id),
        optional_u16(diagnostic.status_code),
        optional_usize(diagnostic.message_len),
        optional_u64(diagnostic.body_len)
    )
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

fn sdk_error_category<E, R>(error: &transcribe::error::SdkError<E, R>) -> &'static str {
    match error {
        transcribe::error::SdkError::ConstructionFailure(_) => "construction",
        transcribe::error::SdkError::TimeoutError(_) => "timeout",
        transcribe::error::SdkError::DispatchFailure(_) => "network_unreachable",
        transcribe::error::SdkError::ResponseError(_) => "response",
        transcribe::error::SdkError::ServiceError(_) => "service",
        _ => "unknown",
    }
}

fn start_stream_service_category(error: &StartStreamTranscriptionError) -> &'static str {
    match error {
        StartStreamTranscriptionError::BadRequestException(_) => "bad_request",
        StartStreamTranscriptionError::ConflictException(_) => "conflict",
        StartStreamTranscriptionError::InternalFailureException(_) => "internal_failure",
        StartStreamTranscriptionError::LimitExceededException(_) => "limit_exceeded",
        StartStreamTranscriptionError::ServiceUnavailableException(_) => "service_unavailable",
        _ => "unhandled_service_error",
    }
}

fn start_stream_service_code(error: &StartStreamTranscriptionError) -> Option<&str> {
    match error {
        StartStreamTranscriptionError::BadRequestException(_) => Some("BadRequestException"),
        StartStreamTranscriptionError::ConflictException(_) => Some("ConflictException"),
        StartStreamTranscriptionError::InternalFailureException(_) => {
            Some("InternalFailureException")
        }
        StartStreamTranscriptionError::LimitExceededException(_) => Some("LimitExceededException"),
        StartStreamTranscriptionError::ServiceUnavailableException(_) => {
            Some("ServiceUnavailableException")
        }
        _ => error.code(),
    }
}

fn start_stream_service_message(error: &StartStreamTranscriptionError) -> Option<&str> {
    match error {
        StartStreamTranscriptionError::BadRequestException(inner) => inner.message(),
        StartStreamTranscriptionError::ConflictException(inner) => inner.message(),
        StartStreamTranscriptionError::InternalFailureException(inner) => inner.message(),
        StartStreamTranscriptionError::LimitExceededException(inner) => inner.message(),
        StartStreamTranscriptionError::ServiceUnavailableException(inner) => inner.message(),
        _ => error.message(),
    }
}

fn transcript_stream_service_category(error: &TranscriptResultStreamError) -> &'static str {
    match error {
        TranscriptResultStreamError::BadRequestException(_) => "bad_request",
        TranscriptResultStreamError::ConflictException(_) => "conflict",
        TranscriptResultStreamError::InternalFailureException(_) => "internal_failure",
        TranscriptResultStreamError::LimitExceededException(_) => "limit_exceeded",
        TranscriptResultStreamError::ServiceUnavailableException(_) => "service_unavailable",
        _ => "unhandled_service_error",
    }
}

fn transcript_stream_service_code(error: &TranscriptResultStreamError) -> Option<&str> {
    match error {
        TranscriptResultStreamError::BadRequestException(_) => Some("BadRequestException"),
        TranscriptResultStreamError::ConflictException(_) => Some("ConflictException"),
        TranscriptResultStreamError::InternalFailureException(_) => {
            Some("InternalFailureException")
        }
        TranscriptResultStreamError::LimitExceededException(_) => Some("LimitExceededException"),
        TranscriptResultStreamError::ServiceUnavailableException(_) => {
            Some("ServiceUnavailableException")
        }
        _ => error.code(),
    }
}

fn transcript_stream_service_message(error: &TranscriptResultStreamError) -> Option<&str> {
    match error {
        TranscriptResultStreamError::BadRequestException(inner) => inner.message(),
        TranscriptResultStreamError::ConflictException(inner) => inner.message(),
        TranscriptResultStreamError::InternalFailureException(inner) => inner.message(),
        TranscriptResultStreamError::LimitExceededException(inner) => inner.message(),
        TranscriptResultStreamError::ServiceUnavailableException(inner) => inner.message(),
        _ => error.message(),
    }
}

fn metadata_request_id(error: &impl ProvideErrorMetadata) -> Option<&str> {
    error.meta().extra("aws_request_id")
}

fn format_start_stream_sdk_error<R>(
    error: &transcribe::error::SdkError<StartStreamTranscriptionError, R>,
    status_code: Option<u16>,
    body_len: Option<u64>,
    request_id: Option<&str>,
) -> String {
    let service_error = error.as_service_error();
    let category = service_error
        .map(start_stream_service_category)
        .unwrap_or_else(|| sdk_error_category(error));
    let code = service_error
        .and_then(start_stream_service_code)
        .or_else(|| error.code());
    let message_len = service_error
        .and_then(start_stream_service_message)
        .or_else(|| error.message())
        .map(char_len);
    let request_id = request_id.or_else(|| service_error.and_then(metadata_request_id));

    format_aws_transcribe_diagnostic(AwsTranscribeDiagnostic {
        operation: "start_stream_transcription",
        category,
        error_kind: sdk_error_kind(error),
        code,
        request_id,
        status_code,
        message_len,
        body_len,
    })
}

fn format_start_stream_error(
    error: &transcribe::error::SdkError<StartStreamTranscriptionError>,
) -> String {
    let response = error.raw_response();
    let status_code = response.map(|response| response.status().as_u16());
    let body_len = response.and_then(|response| {
        response
            .body()
            .content_length()
            .or_else(|| response.body().bytes().map(|bytes| bytes.len() as u64))
    });
    let request_id = response.and_then(|response| {
        response
            .headers()
            .get("x-amzn-requestid")
            .or_else(|| response.headers().get("x-amz-request-id"))
    });

    format_start_stream_sdk_error(error, status_code, body_len, request_id)
}

fn format_transcript_stream_sdk_error<R>(
    error: &transcribe::error::SdkError<TranscriptResultStreamError, R>,
    status_code: Option<u16>,
    body_len: Option<u64>,
    request_id: Option<&str>,
) -> String {
    let service_error = error.as_service_error();
    let category = service_error
        .map(transcript_stream_service_category)
        .unwrap_or_else(|| sdk_error_category(error));
    let code = service_error
        .and_then(transcript_stream_service_code)
        .or_else(|| error.code());
    let message_len = service_error
        .and_then(transcript_stream_service_message)
        .or_else(|| error.message())
        .map(char_len);
    let request_id = request_id.or_else(|| service_error.and_then(metadata_request_id));

    format_aws_transcribe_diagnostic(AwsTranscribeDiagnostic {
        operation: "transcript_result_stream_recv",
        category,
        error_kind: sdk_error_kind(error),
        code,
        request_id,
        status_code,
        message_len,
        body_len,
    })
}

fn format_transcript_stream_error<R>(
    error: &transcribe::error::SdkError<TranscriptResultStreamError, R>,
) -> String {
    format_transcript_stream_sdk_error(error, None, None, None)
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
    Recoverable(String),
    /// A non-recoverable error (service/construction) — surface and stop.
    Unrecoverable(String),
}

/// A single step returned by the shared reconnect ladder.
#[derive(Debug)]
enum LadderStep {
    /// Backoff elapsed; the caller should attempt to re-open the stream.
    Continue,
    /// `is_transcribing` was cleared during backoff — stop cleanly.
    Cancelled,
    /// The backoff schedule is exhausted; the session ends with this error.
    GiveUp(String),
}

/// Whether a `DriveOutcome` warrants a reconnect attempt. Recoverable outcomes
/// retry on the ladder; user-stop, clean completion, and unrecoverable errors
/// do not (M1 / audio-graph-35de). Pure so the retry policy is unit-testable
/// without a live AWS stream.
fn should_reconnect(outcome: &DriveOutcome) -> bool {
    matches!(outcome, DriveOutcome::Recoverable(_))
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
    let handle = tokio::spawn(async move {
        // Deliver one chunk to the stream, or return it for carryover when the
        // stream channel has died so the chunk reaches the NEXT stream instead
        // of vanishing with the abandoned one.
        async fn deliver(
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
                .send(Ok(AudioStream::AudioEvent(audio_event)))
                .await
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
                drop(audio_tx);
                return;
            }
            if let Err(chunk) = deliver(&audio_tx, &source_id_hint, chunk).await {
                log::info!("AWS Transcribe: audio channel closed during carryover drain");
                push_carryover(&carryover, chunk);
                drop(audio_tx);
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

                    if let Err(chunk) = deliver(&audio_tx, &source_id_hint, chunk).await {
                        // Stream channel died mid-send — same reasoning: the
                        // chunk must survive to the next connection.
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
        ReconnectStep::GiveUp { attempted } => LadderStep::GiveUp(format!(
            "AWS Transcribe reconnect attempts exhausted after {attempted}"
        )),
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
) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Failed to create tokio runtime: {}", e))?;

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
) -> Result<(), String> {
    config
        .content_egress_policy()
        .check_audio("asr.aws_transcribe")?;

    let sdk_config =
        build_aws_sdk_config(config.region(), config.credential_source().clone()).await?;
    let client = transcribe::Client::new(&sdk_config);

    let language_code = config
        .language_code()
        .parse::<transcribe::types::LanguageCode>()
        .unwrap_or(transcribe::types::LanguageCode::EnUs);
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

        let audio_stream: aws_smithy_http::event_stream::EventStreamSender<
            AudioStream,
            transcribe::types::error::AudioStreamError,
        > = aws_smithy_http::event_stream::EventStreamSender::from(
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
                let diagnostic = format_start_stream_error(&e);
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
                    LadderStep::GiveUp(message) => return Err(message),
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
                        DriveOutcome::Recoverable(
                            "result stream ended while transcribing".to_string(),
                        )
                    } else {
                        DriveOutcome::Completed
                    };
                }
                Err(e) => {
                    let diagnostic = format_transcript_stream_error(&e);
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
        // on the next iteration if we reconnect.
        forwarder.stop();
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

        if let DriveOutcome::Recoverable(diagnostic) = &outcome {
            log::warn!("AWS Transcribe: recoverable stream error, reconnecting {diagnostic}");
        }

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
            LadderStep::GiveUp(message) => return Err(message),
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
            data: vec![0.5, -0.25],
            sample_rate: 16_000,
            num_frames: 2,
            timestamp: Some(Duration::from_millis(32)),
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

        assert!(error.contains("Privacy policy blocked"));
        assert!(error.contains("asr.aws_transcribe"));
        assert!(error.contains(EXPLICIT_POLICY_REQUIRED));
        assert!(!error.contains("Failed to start AWS Transcribe stream"));
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

        assert!(error.contains("Privacy policy blocked"));
        assert!(error.contains("asr.aws_transcribe"));
        assert!(error.contains("local_only"));
        assert!(!error.contains("Failed to start AWS Transcribe stream"));
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

        for forbidden in [
            "0.5",
            "-0.25",
            "patient said private diagnosis",
            "mic-private-source",
        ] {
            assert!(
                !error.contains(forbidden),
                "privacy error leaked {forbidden}: {error}"
            );
        }
    }

    fn assert_aws_diagnostic_excludes_raw_values(diagnostic: &str) {
        for forbidden in [
            "provider raw text should not leak",
            "patient said private diagnosis",
            "mic-private-source",
            "0.5",
            "-0.25",
            "AKIA1234567890ABCDEF",
            "ASIA1234567890ABCDEF",
            "aws-secret-looking-value",
        ] {
            assert!(
                !diagnostic.contains(forbidden),
                "AWS diagnostic leaked {forbidden}: {diagnostic}"
            );
        }
    }

    #[test]
    fn start_stream_error_diagnostic_uses_metadata_only() {
        let raw_provider_message = concat!(
            "provider raw text should not leak; ",
            "patient said private diagnosis; ",
            "source=mic-private-source; samples=[0.5,-0.25]; ",
            "access_key=AKIA1234567890ABCDEF; secret=aws-secret-looking-value"
        );
        let error = transcribe::error::SdkError::<StartStreamTranscriptionError, ()>::service_error(
            StartStreamTranscriptionError::BadRequestException(
                transcribe::types::error::BadRequestException::builder()
                    .message(raw_provider_message)
                    .build(),
            ),
            (),
        );

        let diagnostic = format_start_stream_sdk_error(
            &error,
            Some(400),
            Some(raw_provider_message.len() as u64),
            Some("aws-req-123"),
        );

        assert!(diagnostic.contains("provider=aws_transcribe"));
        assert!(diagnostic.contains("operation=start_stream_transcription"));
        assert!(diagnostic.contains("category=bad_request"));
        assert!(diagnostic.contains("error_kind=service_error"));
        assert!(diagnostic.contains("code=BadRequestException"));
        assert!(diagnostic.contains("request_id=aws-req-123"));
        assert!(diagnostic.contains("status_code=400"));
        assert!(diagnostic.contains(&format!(
            "message_len={}",
            raw_provider_message.chars().count()
        )));
        assert!(diagnostic.contains(&format!("body_len={}", raw_provider_message.len())));
        assert_aws_diagnostic_excludes_raw_values(&diagnostic);
    }

    #[test]
    fn transcript_stream_error_diagnostic_uses_metadata_only() {
        let raw_provider_message = concat!(
            "provider raw text should not leak; ",
            "transcript=patient said private diagnosis; ",
            "source=mic-private-source; samples=[0.5,-0.25]; ",
            "session_key=ASIA1234567890ABCDEF"
        );
        let error = transcribe::error::SdkError::<TranscriptResultStreamError, ()>::service_error(
            TranscriptResultStreamError::ServiceUnavailableException(
                transcribe::types::error::ServiceUnavailableException::builder()
                    .message(raw_provider_message)
                    .build(),
            ),
            (),
        );

        let diagnostic = format_transcript_stream_sdk_error(&error, None, None, None);

        assert!(diagnostic.contains("provider=aws_transcribe"));
        assert!(diagnostic.contains("operation=transcript_result_stream_recv"));
        assert!(diagnostic.contains("category=service_unavailable"));
        assert!(diagnostic.contains("error_kind=service_error"));
        assert!(diagnostic.contains("code=ServiceUnavailableException"));
        assert!(diagnostic.contains("status_code=none"));
        assert!(diagnostic.contains(&format!(
            "message_len={}",
            raw_provider_message.chars().count()
        )));
        assert!(diagnostic.contains("body_len=none"));
        assert_aws_diagnostic_excludes_raw_values(&diagnostic);
    }

    #[test]
    fn diagnostic_tokens_fall_back_to_lengths_for_unsafe_metadata() {
        let unsafe_code = "BadRequestException provider raw text should not leak";
        let unsafe_request_id = "req AKIA1234567890ABCDEF patient said private diagnosis";
        let diagnostic = format_aws_transcribe_diagnostic(AwsTranscribeDiagnostic {
            operation: "start_stream_transcription",
            category: "bad_request",
            error_kind: "service_error",
            code: Some(unsafe_code),
            request_id: Some(unsafe_request_id),
            status_code: Some(400),
            message_len: Some(12),
            body_len: Some(34),
        });

        assert!(diagnostic.contains(&format!("code=present_len_{}", unsafe_code.len())));
        assert!(diagnostic.contains(&format!(
            "request_id=present_len_{}",
            unsafe_request_id.len()
        )));
        assert_aws_diagnostic_excludes_raw_values(&diagnostic);
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
        assert!(should_reconnect(&DriveOutcome::Recoverable("blip".into())));
        assert!(!should_reconnect(&DriveOutcome::UserStopped));
        assert!(!should_reconnect(&DriveOutcome::Completed));
        assert!(!should_reconnect(&DriveOutcome::Unrecoverable(
            "auth".into()
        )));
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
        // Ladder is [1,2,5,10] — the 5th step exhausts it.
        let mut attempts: u32 = 4;
        let mut statuses = Vec::new();

        let step =
            advance_reconnect_ladder(&mut attempts, &is_transcribing, &mut |s| statuses.push(s))
                .await;

        match step {
            LadderStep::GiveUp(message) => {
                assert!(message.contains("exhausted"));
                assert!(message.contains('4'));
            }
            other => panic!("expected GiveUp, got {other:?}"),
        }
        // No Reconnecting emitted once the schedule is exhausted.
        assert!(statuses.is_empty());
    }

    #[test]
    fn reconnect_ladder_backoff_matches_shared_schedule() {
        // The AWS path rides the same [1,2,5,10] ladder as the WS siblings.
        assert_eq!(backoff_for_attempt(1), Some(1));
        assert_eq!(backoff_for_attempt(2), Some(2));
        assert_eq!(backoff_for_attempt(3), Some(5));
        assert_eq!(backoff_for_attempt(4), Some(10));
        assert_eq!(backoff_for_attempt(5), None);
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
    /// stream drops immediately) must climb the ladder to the documented
    /// 1/2/5/10 give-up. Mirrors the production loop's budget policy: the
    /// budget resets only when the stream was healthy ≥ the threshold; each
    /// immediate drop advances the ladder exactly once.
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

        assert_eq!(backoffs, vec![1, 2, 5, 10], "ladder must climb, not loop");
        assert_eq!(attempted, 4, "give-up must report the exhausted budget");

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
}
