//! Generic cloud-ASR worker contract.
//!
//! [`CloudAsrConfig`] is the minimal configuration shape shared by the
//! plain HTTP/OpenAI-compatible streaming backends (Groq, OpenAI-style
//! Whisper endpoints, etc.) — provider-specific backends like Deepgram,
//! AssemblyAI, and AWS Transcribe Streaming each live in their own
//! sibling module because their wire protocols, auth, and session
//! lifetimes differ enough to warrant it.
//!
//! The `CloudAsrWorker` in this module takes a [`SpeechSegment`] off the
//! input channel, POSTs the PCM payload to `endpoint` with the API key,
//! and emits a [`TranscriptSegment`] downstream. Unlike the WebSocket
//! providers this worker is request/response per utterance; there is no
//! long-lived connection and no reconnect state machine.
//!
//! See also: [`crate::asr::deepgram`], [`crate::asr::assemblyai`],
//! [`crate::asr::aws_transcribe`].

use uuid::Uuid;

use crate::state::TranscriptSegment;

use super::{ProviderContentEgressPolicy, SpeechSegment};

const EXPLICIT_POLICY_REQUIRED: &str = "explicit_policy_required";

/// Cloud ASR provider configuration.
#[derive(Clone)]
pub struct CloudAsrConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub language: String,
}

/// View over cloud-ASR settings plus a content-egress policy.
pub trait CloudAsrRequestConfig {
    fn endpoint(&self) -> &str;
    fn api_key(&self) -> &str;
    fn model(&self) -> &str;
    fn language(&self) -> &str;

    fn content_egress_policy(&self) -> ProviderContentEgressPolicy {
        ProviderContentEgressPolicy::block(EXPLICIT_POLICY_REQUIRED)
    }
}

impl CloudAsrRequestConfig for CloudAsrConfig {
    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn api_key(&self) -> &str {
        &self.api_key
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn language(&self) -> &str {
        &self.language
    }
}

#[derive(Clone)]
pub struct GuardedCloudAsrConfig {
    inner: CloudAsrConfig,
    content_egress_policy: ProviderContentEgressPolicy,
}

impl CloudAsrConfig {
    pub fn with_content_egress_policy(
        self,
        policy: ProviderContentEgressPolicy,
    ) -> GuardedCloudAsrConfig {
        GuardedCloudAsrConfig {
            inner: self,
            content_egress_policy: policy,
        }
    }
}

impl std::fmt::Debug for GuardedCloudAsrConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardedCloudAsrConfig")
            .field("inner", &self.inner)
            .field("content_egress_policy", &self.content_egress_policy)
            .finish()
    }
}

impl CloudAsrRequestConfig for GuardedCloudAsrConfig {
    fn endpoint(&self) -> &str {
        self.inner.endpoint()
    }

    fn api_key(&self) -> &str {
        self.inner.api_key()
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    fn language(&self) -> &str {
        self.inner.language()
    }

    fn content_egress_policy(&self) -> ProviderContentEgressPolicy {
        self.content_egress_policy
    }
}

impl std::fmt::Debug for CloudAsrConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudAsrConfig")
            .field("endpoint", &self.endpoint)
            .field(
                "api_key",
                &crate::credentials::redacted_secret_presence(Some(&self.api_key)),
            )
            .field("model", &self.model)
            .field("language", &self.language)
            .finish()
    }
}

/// Result from a cloud ASR transcription call.
#[derive(Debug, serde::Deserialize)]
struct WhisperResponse {
    text: String,
    #[serde(default)]
    segments: Option<Vec<WhisperSegment>>,
}

#[derive(Debug, serde::Deserialize)]
struct WhisperSegment {
    #[serde(default)]
    start: f64,
    #[serde(default)]
    end: f64,
    text: String,
    #[serde(default)]
    no_speech_prob: Option<f64>,
}

/// Encode 16kHz mono f32 audio samples into a WAV byte buffer (PCM s16le).
fn encode_wav(samples: &[f32], sample_rate: u32, channels: u16) -> Vec<u8> {
    let num_samples = samples.len();
    let bytes_per_sample: u16 = 2;
    let data_size = (num_samples * bytes_per_sample as usize) as u32;
    let file_size = 36 + data_size;

    let mut buf = Vec::with_capacity(44 + data_size as usize);

    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&file_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // fmt chunk
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * channels as u32 * bytes_per_sample as u32;
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    let block_align = channels * bytes_per_sample;
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&(bytes_per_sample * 8).to_le_bytes()); // bits per sample

    // data chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let i16_val = (clamped * 32767.0) as i16;
        buf.extend_from_slice(&i16_val.to_le_bytes());
    }

    buf
}

/// Transcribe a speech segment using an OpenAI-compatible STT API.
///
/// Works with: OpenAI Whisper API, Groq, Together AI, Deepgram (REST),
/// and any provider that implements the `/v1/audio/transcriptions` endpoint.
///
/// NOTE: This call blocks the calling thread for the full round-trip to the
/// API (typically 0.5–5s depending on provider and audio length). Callers
/// that dispatch segments at real-time rates should budget for this latency
/// (the upstream `AccumulatedSegment` channel capacity must absorb the
/// in-flight segment plus any queued segments produced while the HTTP call
/// is in flight).
pub fn transcribe_segment<C: CloudAsrRequestConfig + ?Sized>(
    config: &C,
    segment: &SpeechSegment,
) -> Result<Vec<TranscriptSegment>, String> {
    config.content_egress_policy().check_audio("asr.cloud")?;

    let call_start = std::time::Instant::now();
    let audio_secs = segment.audio.len() as f64 / 16_000.0;
    log::info!(
        "Cloud ASR: starting transcription request (audio={:.2}s, model={})",
        audio_secs,
        config.model()
    );

    let wav_bytes = encode_wav(&segment.audio, 16000, 1);

    let url = format!(
        "{}/audio/transcriptions",
        config.endpoint().trim_end_matches('/')
    );

    let part = reqwest::blocking::multipart::Part::bytes(wav_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| format!("Failed to create multipart part: {}", e))?;

    let form = reqwest::blocking::multipart::Form::new()
        .part("file", part)
        .text("model", config.model().to_string())
        .text("response_format", "verbose_json")
        .text("language", config.language().to_string());

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let mut request = client.post(&url).multipart(form);
    if !config.api_key().is_empty() {
        request = request.bearer_auth(config.api_key());
    }

    let response = request
        .send()
        .map_err(|e| format!("Cloud ASR request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .unwrap_or_else(|_| "unable to read response body".to_string());
        return Err(cloud_asr_api_error_message(status, &body, config.api_key()));
    }

    let body = response
        .text()
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    let whisper_resp: WhisperResponse = serde_json::from_str(&body)
        .map_err(|e| cloud_asr_parse_error_message(&e, &body, config.api_key()))?;

    let elapsed_ms = call_start.elapsed().as_millis();
    let rtf = call_start.elapsed().as_secs_f64() / audio_secs.max(0.001);
    if elapsed_ms > 2_000 {
        log::warn!(
            "Cloud ASR: slow API response — elapsed={}ms, audio={:.2}s, RTF={:.2}x (API slower than real-time, segments may be dropped)",
            elapsed_ms,
            audio_secs,
            rtf
        );
    } else {
        log::info!(
            "Cloud ASR: transcription complete — elapsed={}ms, audio={:.2}s, RTF={:.2}x",
            elapsed_ms,
            audio_secs,
            rtf
        );
    }

    Ok(map_whisper_response(
        whisper_resp,
        &segment.source_id,
        segment.start_time.as_secs_f64(),
        segment.end_time.as_secs_f64(),
    ))
}

fn cloud_asr_api_error_message(status: reqwest::StatusCode, body: &str, _api_key: &str) -> String {
    // Anonymous, structured diagnostic (no-op unless analytics is enabled). Only
    // the controlled category/provider/status ride along — never the body.
    crate::analytics::capture_diagnostic(crate::analytics::DiagEvent {
        name: "asr.cloud.http_error",
        category: crate::analytics::Category::Asr,
        level: sentry::Level::Error,
        provider: Some("cloud_asr"),
        kind: Some("http_error"),
        http_status: Some(status.as_u16()),
        recoverable: None,
    });
    format!(
        "Cloud ASR API error: provider=cloud_asr status={} body_bytes={} body_chars={}",
        status,
        body.len(),
        body.chars().count()
    )
}

/// Build the parse-failure error for a malformed cloud-ASR (readiness /
/// transcription) response.
///
/// `serde_json::Error`'s `Display` reports the failure *position* (line/column),
/// not the input bytes, so the pre-existing `{e}` message did not itself echo
/// the body. This helper is defense-in-depth on the UI-visible `String` error:
/// it routes the detail through the shared redaction/safe-excerpt helper
/// (registering the request `api_key` as a known secret) — bounding length and
/// scrubbing credential shapes — and reports body byte/char counts instead of
/// the body, so a future change that interpolated the raw body here cannot leak
/// the transcript `text` or any credentials the endpoint echoed back.
fn cloud_asr_parse_error_message(error: &serde_json::Error, body: &str, api_key: &str) -> String {
    let detail = crate::error::redacted_error_excerpt(&error.to_string(), [api_key], 200);
    format!(
        "Failed to parse cloud ASR response: provider=cloud_asr body_bytes={} body_chars={} detail={}",
        body.len(),
        body.chars().count(),
        detail
    )
}

/// Map a parsed [`WhisperResponse`] into downstream [`TranscriptSegment`]s.
///
/// This is the pure (no-I/O) tail of [`transcribe_segment`]: it runs *after*
/// the HTTP response has been read and JSON-parsed. Pulling it out makes the
/// mapping rules unit-testable without a network round-trip.
///
/// Rules (behaviour-preserving):
/// - When the response carries per-`segments` timing, each segment is emitted
///   with empty-text segments filtered out; `confidence = 1.0 - no_speech_prob`
///   (defaulting to `0.9` when `no_speech_prob` is absent); and `start`/`end`
///   offsets are added to `segment_start_secs`.
/// - When there are no `segments`, the top-level `text` becomes a single
///   segment spanning `segment_start_secs..segment_end_secs` with confidence
///   `0.9`; an empty top-level `text` yields no segments.
fn map_whisper_response(
    whisper_resp: WhisperResponse,
    source_id: &str,
    segment_start_secs: f64,
    segment_end_secs: f64,
) -> Vec<TranscriptSegment> {
    if let Some(segments) = whisper_resp.segments {
        segments
            .into_iter()
            .filter(|s| !s.text.trim().is_empty())
            .map(|s| {
                let confidence = s.no_speech_prob.map(|p| (1.0 - p) as f32).unwrap_or(0.9);
                TranscriptSegment {
                    id: Uuid::new_v4().to_string(),
                    source_id: source_id.to_string(),
                    speaker_id: None,
                    speaker_label: None,
                    text: s.text.trim().to_string(),
                    start_time: segment_start_secs + s.start,
                    end_time: segment_start_secs + s.end,
                    confidence,
                }
            })
            .collect()
    } else {
        let text = whisper_resp.text.trim().to_string();
        if text.is_empty() {
            return vec![];
        }
        vec![TranscriptSegment {
            id: Uuid::new_v4().to_string(),
            source_id: source_id.to_string(),
            speaker_id: None,
            speaker_label: None,
            text,
            start_time: segment_start_secs,
            end_time: segment_end_secs,
            confidence: 0.9,
        }]
    }
}

#[cfg(test)]
mod tests_mapping {
    use super::*;

    fn seg(start: f64, end: f64, text: &str, no_speech_prob: Option<f64>) -> WhisperSegment {
        WhisperSegment {
            start,
            end,
            text: text.to_string(),
            no_speech_prob,
        }
    }

    #[test]
    fn empty_text_segments_are_filtered() {
        let resp = WhisperResponse {
            text: "ignored top-level".to_string(),
            segments: Some(vec![
                seg(0.0, 1.0, "  hello  ", Some(0.1)),
                seg(1.0, 2.0, "   ", Some(0.1)), // whitespace-only -> dropped
                seg(2.0, 3.0, "", Some(0.1)),    // empty -> dropped
                seg(3.0, 4.0, "world", Some(0.1)),
            ]),
        };

        let out = map_whisper_response(resp, "src-1", 0.0, 10.0);

        assert_eq!(out.len(), 2, "only the two non-empty segments survive");
        // text is trimmed
        assert_eq!(out[0].text, "hello");
        assert_eq!(out[1].text, "world");
        assert_eq!(out[0].source_id, "src-1");
        assert!(out[0].speaker_id.is_none());
        assert!(out[0].speaker_label.is_none());
    }

    #[test]
    fn confidence_is_one_minus_no_speech_prob_with_default() {
        let resp = WhisperResponse {
            text: String::new(),
            segments: Some(vec![
                seg(0.0, 1.0, "a", Some(0.25)), // 1.0 - 0.25 = 0.75
                seg(1.0, 2.0, "b", None),       // default 0.9
                seg(2.0, 3.0, "c", Some(0.0)),  // 1.0
            ]),
        };

        let out = map_whisper_response(resp, "src", 0.0, 3.0);

        assert_eq!(out.len(), 3);
        assert!((out[0].confidence - 0.75).abs() < 1e-6);
        assert!(
            (out[1].confidence - 0.9).abs() < 1e-6,
            "no_speech_prob=None must default to 0.9"
        );
        assert!((out[2].confidence - 1.0).abs() < 1e-6);
    }

    #[test]
    fn segment_times_are_offset_by_segment_start() {
        let resp = WhisperResponse {
            text: String::new(),
            segments: Some(vec![seg(0.5, 1.5, "offset me", Some(0.1))]),
        };

        // segment_start_secs offsets both start and end; segment_end_secs is
        // unused when per-segment timing is present.
        let out = map_whisper_response(resp, "src", 100.0, 999.0);

        assert_eq!(out.len(), 1);
        assert!((out[0].start_time - 100.5).abs() < 1e-9);
        assert!((out[0].end_time - 101.5).abs() < 1e-9);
    }

    #[test]
    fn text_only_no_segments_falls_back_to_single_span() {
        let resp = WhisperResponse {
            text: "  full utterance  ".to_string(),
            segments: None,
        };

        let out = map_whisper_response(resp, "src-2", 5.0, 8.0);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "full utterance"); // trimmed
        assert_eq!(out[0].source_id, "src-2");
        // spans the whole segment using the provided start/end bounds
        assert!((out[0].start_time - 5.0).abs() < 1e-9);
        assert!((out[0].end_time - 8.0).abs() < 1e-9);
        assert!((out[0].confidence - 0.9).abs() < 1e-6);
    }

    #[test]
    fn empty_top_level_text_with_no_segments_yields_nothing() {
        let resp = WhisperResponse {
            text: "   ".to_string(),
            segments: None,
        };

        let out = map_whisper_response(resp, "src", 0.0, 1.0);

        assert!(out.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn le_u32(b: &[u8]) -> u32 {
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }
    fn le_u16(b: &[u8]) -> u16 {
        u16::from_le_bytes([b[0], b[1]])
    }
    fn le_i16(b: &[u8]) -> i16 {
        i16::from_le_bytes([b[0], b[1]])
    }

    fn test_config() -> CloudAsrConfig {
        CloudAsrConfig {
            endpoint: "https://invalid.localhost.test/v1".into(),
            api_key: "sk-cloud-asr-private-test-key".into(),
            model: "whisper-1".into(),
            language: "en".into(),
        }
    }

    fn speech_segment(audio: Vec<f32>) -> SpeechSegment {
        let num_frames = audio.len();
        SpeechSegment {
            source_id: "mic-private-source".into(),
            audio,
            start_time: Duration::from_millis(0),
            end_time: Duration::from_millis(32),
            num_frames,
        }
    }

    #[test]
    fn encode_wav_header_is_44_bytes_and_well_formed() {
        let samples = [0.0_f32, 0.5, -0.5];
        let sample_rate = 16_000u32;
        let channels = 1u16;
        let wav = encode_wav(&samples, sample_rate, channels);

        // 44-byte header + 2 bytes per sample.
        assert_eq!(wav.len(), 44 + samples.len() * 2);

        // RIFF / WAVE / fmt / data tags at their canonical offsets.
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");

        // file_size = 36 + data_size.
        let data_size = (samples.len() * 2) as u32;
        assert_eq!(le_u32(&wav[4..8]), 36 + data_size);

        // fmt chunk: size 16, PCM(1), channels, sample_rate, byte_rate, block_align, bits.
        assert_eq!(le_u32(&wav[16..20]), 16, "fmt chunk size");
        assert_eq!(le_u16(&wav[20..22]), 1, "PCM format tag");
        assert_eq!(le_u16(&wav[22..24]), channels);
        assert_eq!(le_u32(&wav[24..28]), sample_rate);
        let bytes_per_sample = 2u32;
        assert_eq!(
            le_u32(&wav[28..32]),
            sample_rate * channels as u32 * bytes_per_sample,
            "byte_rate"
        );
        assert_eq!(le_u16(&wav[32..34]), channels * 2, "block_align");
        assert_eq!(le_u16(&wav[34..36]), 16, "bits per sample");

        // data chunk size.
        assert_eq!(le_u32(&wav[40..44]), data_size);
    }

    #[test]
    fn encode_wav_clamps_and_scales_samples() {
        // Out-of-range samples must clamp to ±1.0 before scaling by 32767.
        let samples = [1.0_f32, -1.0, 2.0, -2.0, 0.0];
        let wav = encode_wav(&samples, 16_000, 1);

        let pcm: Vec<i16> = wav[44..].chunks_exact(2).map(le_i16).collect();
        assert_eq!(pcm.len(), samples.len());
        assert_eq!(pcm[0], 32767, "1.0 → 32767");
        assert_eq!(pcm[1], -32767, "-1.0 → -32767");
        assert_eq!(pcm[2], 32767, "2.0 clamps to 1.0 → 32767");
        assert_eq!(pcm[3], -32767, "-2.0 clamps to -1.0 → -32767");
        assert_eq!(pcm[4], 0, "0.0 → 0");
    }

    #[test]
    fn encode_wav_empty_samples_is_just_the_header() {
        let wav = encode_wav(&[], 16_000, 1);
        assert_eq!(wav.len(), 44);
        assert_eq!(le_u32(&wav[40..44]), 0, "empty data chunk");
        assert_eq!(le_u32(&wav[4..8]), 36, "file_size = 36 + 0");
    }

    #[test]
    fn encode_wav_stereo_byte_rate_and_block_align() {
        // Exercise the channels arithmetic with a 2-channel, 44.1kHz buffer.
        let wav = encode_wav(&[0.0, 0.0], 44_100, 2);
        assert_eq!(le_u32(&wav[24..28]), 44_100);
        assert_eq!(le_u16(&wav[22..24]), 2);
        // byte_rate = 44100 * 2 * 2
        assert_eq!(le_u32(&wav[28..32]), 44_100 * 2 * 2);
        // block_align = channels * bytes_per_sample = 2 * 2
        assert_eq!(le_u16(&wav[32..34]), 4);
    }

    #[test]
    fn cloud_asr_content_policy_defaults_to_explicit_policy_required() {
        let config = test_config();

        let error = config
            .content_egress_policy()
            .check_audio("asr.cloud")
            .unwrap_err();

        assert!(error.contains("Privacy policy blocked audio egress"));
        assert!(error.contains("asr.cloud"));
        assert!(error.contains(EXPLICIT_POLICY_REQUIRED));
    }

    #[test]
    fn cloud_asr_explicit_allow_policy_permits_audio_guard() {
        let config = test_config().with_content_egress_policy(ProviderContentEgressPolicy::allow());

        assert!(
            config
                .content_egress_policy()
                .check_audio("asr.cloud")
                .is_ok()
        );
    }

    #[test]
    fn default_policy_rejects_audio_before_http_request() {
        let config = test_config();
        let segment = speech_segment(vec![0.5, -0.25]);

        let error = transcribe_segment(&config, &segment).unwrap_err();

        assert!(error.contains("Privacy policy blocked"));
        assert!(error.contains("asr.cloud"));
        assert!(error.contains(EXPLICIT_POLICY_REQUIRED));
        assert!(!error.contains("Cloud ASR request failed"));
    }

    #[test]
    fn blocked_policy_rejects_audio_before_http_request() {
        let config = test_config()
            .with_content_egress_policy(ProviderContentEgressPolicy::block("local_only"));
        let segment = speech_segment(vec![0.5, -0.25]);

        let error = transcribe_segment(&config, &segment).unwrap_err();

        assert!(error.contains("Privacy policy blocked"));
        assert!(error.contains("asr.cloud"));
        assert!(error.contains("local_only"));
        assert!(!error.contains("Cloud ASR request failed"));
    }

    #[test]
    fn blocked_policy_error_redacts_cloud_audio_and_secret_values() {
        let config = test_config()
            .with_content_egress_policy(ProviderContentEgressPolicy::block("local_only"));
        let segment = speech_segment(vec![0.5, -0.25]);

        let error = transcribe_segment(&config, &segment).unwrap_err();

        for forbidden in [
            "sk-cloud-asr-private-test-key",
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

    #[test]
    fn cloud_asr_error_omits_provider_body_and_secret() {
        let api_key = "sk-cloud-asr-secret";
        let provider_body = format!(r#"{{"error":"echoed bearer {api_key}; private transcript"}}"#);
        let message =
            cloud_asr_api_error_message(reqwest::StatusCode::UNAUTHORIZED, &provider_body, api_key);

        assert!(
            message.contains("status=401 Unauthorized"),
            "error must carry status, got: {message}"
        );
        assert!(
            message.contains(&format!("body_bytes={}", provider_body.len())),
            "error must carry body byte count, got: {message}"
        );
        assert!(
            message.contains(&format!("body_chars={}", provider_body.chars().count())),
            "error must carry body character count, got: {message}"
        );
        for forbidden in [api_key, "echoed bearer", "private transcript", "<redacted>"] {
            assert!(
                !message.contains(forbidden),
                "error must not echo provider body marker {forbidden}: {message}"
            );
        }
    }

    /// Contract guard for the malformed-response parse path (cloud ASR
    /// readiness/transcription): the UI-visible `String` error must carry
    /// provider + body byte/char context but never the body itself or any
    /// credential shape. `serde_json::Error`'s `Display` only reports line/column
    /// today, so this also locks in that a future change routing the raw `body`
    /// (or a body-echoing error) through this helper stays scrubbed. The metadata
    /// assertions fail if the helper stops emitting provider/byte-count context.
    #[test]
    fn cloud_asr_parse_error_redacts_body_and_secret_shapes() {
        let api_key = "sk-cloud-parse-provider-secret-12345";
        // A body that is NOT valid JSON (trailing garbage) but embeds every
        // credential shape plus transcript PII, so the serde error snippet /
        // any echoed value must be scrubbed.
        // `userinfo` is assembled at runtime so no contiguous
        // scheme://user:pass@host literal sits in source for a secret scanner to
        // flag; the runtime body is identical and still exercises URL-credential
        // scrubbing.
        let userinfo = format!("{}:{}", "svc-user", "svc-pass");
        let body = format!(
            concat!(
                r#"{{"text":"private patient transcript","api_key":"{api_key}","#,
                r#""authorization":"Bearer bearer-cloud-secret-12345","#,
                r#""aws":"AKIA1234567890ABCDEF","#,
                r#""url":"https://{userinfo}@example.com/v1?token=cloud-url-secret-12345"}}"#,
                " <<<not-json-trailer>>>",
            ),
            api_key = api_key,
            userinfo = userinfo,
        );
        let error = serde_json::from_str::<WhisperResponse>(&body)
            .expect_err("fixture body must fail to parse as WhisperResponse");
        let message = cloud_asr_parse_error_message(&error, &body, api_key);

        assert!(
            message.contains("Failed to parse cloud ASR response"),
            "parse error must name the cloud ASR parse failure, got: {message}"
        );
        assert!(
            message.contains("provider=cloud_asr"),
            "parse error must carry provider tag, got: {message}"
        );
        assert!(
            message.contains(&format!("body_bytes={}", body.len())),
            "parse error must carry body byte count, got: {message}"
        );
        assert!(
            message.contains(&format!("body_chars={}", body.chars().count())),
            "parse error must carry body char count, got: {message}"
        );
        for leaked in [
            api_key,
            "bearer-cloud-secret-12345",
            "AKIA1234567890ABCDEF",
            "svc-user:svc-pass",
            "cloud-url-secret-12345",
            "private patient transcript",
        ] {
            assert!(
                !message.contains(leaked),
                "cloud ASR parse error leaked {leaked}: {message}"
            );
        }
    }

    #[test]
    fn cloud_asr_config_debug_redacts_api_key() {
        let config = CloudAsrConfig {
            endpoint: "https://api.openai.com/v1".into(),
            api_key: "sk-cloud-asr-debug-secret".into(),
            model: "whisper-1".into(),
            language: "en".into(),
        };

        let debug = format!("{config:?}");

        assert!(!debug.contains("sk-cloud-asr-debug-secret"));
        assert!(debug.contains("<present>"));
        assert!(debug.contains("https://api.openai.com/v1"));
        assert!(debug.contains("whisper-1"));
    }

    #[test]
    fn whisper_response_deserializes_verbose_json_segments() {
        // Confirms the serde shape used by transcribe_segment: a verbose_json
        // body with per-segment timings + no_speech_prob.
        let body = serde_json::json!({
            "text": "hello world",
            "segments": [
                { "start": 0.0, "end": 1.0, "text": "hello", "no_speech_prob": 0.1 },
                { "start": 1.0, "end": 2.0, "text": " world" }
            ]
        })
        .to_string();
        let resp: WhisperResponse = serde_json::from_str(&body).expect("parse verbose_json");
        let segs = resp.segments.expect("segments present");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "hello");
        assert!((segs[0].no_speech_prob.unwrap() - 0.1).abs() < 1e-9);
        // Missing no_speech_prob defaults to None (→ 0.9 confidence in mapping).
        assert!(segs[1].no_speech_prob.is_none());
        assert!((segs[1].start - 1.0).abs() < 1e-9);
    }

    #[test]
    fn whisper_response_deserializes_text_only_body() {
        // No `segments` key → segments is None (top-level-text fallback path).
        let body = serde_json::json!({ "text": "just text" }).to_string();
        let resp: WhisperResponse = serde_json::from_str(&body).expect("parse text-only");
        assert_eq!(resp.text, "just text");
        assert!(resp.segments.is_none());
    }
}
