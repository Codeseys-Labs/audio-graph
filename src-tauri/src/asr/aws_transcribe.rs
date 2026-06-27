//! AWS Transcribe Streaming ASR integration.
//!
//! Uses the aws-sdk-transcribestreaming crate to stream audio to AWS
//! and receive real-time transcription results with optional speaker diarization.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

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

use super::ProviderContentEgressPolicy;

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
) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Failed to create tokio runtime: {}", e))?;

    rt.block_on(async {
        run_streaming_session(audio_rx, is_transcribing, config, on_transcript, on_partial).await
    })
}

async fn run_streaming_session(
    audio_rx: Receiver<ProcessedAudioChunk>,
    is_transcribing: Arc<AtomicBool>,
    config: impl AwsTranscribeSessionConfig + Send + 'static,
    mut on_transcript: impl FnMut(AwsTranscribeFinal) + Send + 'static,
    mut on_partial: impl FnMut(AwsTranscribePartial) + Send + 'static,
) -> Result<(), String> {
    config
        .content_egress_policy()
        .check_audio("asr.aws_transcribe")?;

    let sdk_config =
        build_aws_sdk_config(config.region(), config.credential_source().clone()).await?;
    let client = transcribe::Client::new(&sdk_config);

    let (audio_tx, audio_stream_rx) = tokio::sync::mpsc::channel::<
        Result<AudioStream, transcribe::types::error::AudioStreamError>,
    >(16);

    let audio_stream: aws_smithy_http::event_stream::EventStreamSender<
        AudioStream,
        transcribe::types::error::AudioStreamError,
    > = aws_smithy_http::event_stream::EventStreamSender::from(
        tokio_stream::wrappers::ReceiverStream::new(audio_stream_rx),
    );

    let language_code = config
        .language_code()
        .parse::<transcribe::types::LanguageCode>()
        .unwrap_or(transcribe::types::LanguageCode::EnUs);

    let mut builder = client
        .start_stream_transcription()
        .language_code(language_code)
        .media_sample_rate_hertz(16000)
        .media_encoding(MediaEncoding::Pcm)
        .audio_stream(audio_stream);

    if config.enable_diarization() {
        builder = builder.show_speaker_label(true);
    }

    let mut output = builder
        .send()
        .await
        .map_err(|e| format_start_stream_error(&e))?;

    log::info!("AWS Transcribe: streaming session started");

    let source_id_hint = Arc::new(RwLock::new(None::<String>));
    let is_transcribing_sender = is_transcribing.clone();
    let source_id_hint_sender = Arc::clone(&source_id_hint);
    tokio::spawn(async move {
        loop {
            if !is_transcribing_sender.load(Ordering::Relaxed) {
                break;
            }

            match audio_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(chunk) => {
                    if let Ok(mut hint) = source_id_hint_sender.write() {
                        // Boundary: the hint is a persisted String, so materialize
                        // the chunk's Arc<str> id here (FA-4b).
                        *hint = Some(chunk.source_id.to_string());
                    }

                    let pcm_bytes = f32_to_pcm_bytes(&chunk.data);
                    let audio_event = AudioEvent::builder()
                        .audio_chunk(Blob::new(pcm_bytes))
                        .build();
                    if audio_tx
                        .send(Ok(AudioStream::AudioEvent(audio_event)))
                        .await
                        .is_err()
                    {
                        log::info!("AWS Transcribe: audio channel closed");
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

    while let Some(event) = output
        .transcript_result_stream
        .recv()
        .await
        .map_err(|e| format_transcript_stream_error(&e))?
    {
        if !is_transcribing.load(Ordering::Relaxed) {
            break;
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
    }

    log::info!("AWS Transcribe: streaming session ended");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
