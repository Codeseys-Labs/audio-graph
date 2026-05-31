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

use super::SpeechSegment;

/// Cloud ASR provider configuration.
#[derive(Debug, Clone)]
pub struct CloudAsrConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub language: String,
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
pub fn transcribe_segment(
    config: &CloudAsrConfig,
    segment: &SpeechSegment,
) -> Result<Vec<TranscriptSegment>, String> {
    let call_start = std::time::Instant::now();
    let audio_secs = segment.audio.len() as f64 / 16_000.0;
    log::info!(
        "Cloud ASR: starting transcription request (audio={:.2}s, model={})",
        audio_secs,
        config.model
    );

    let wav_bytes = encode_wav(&segment.audio, 16000, 1);

    let url = format!(
        "{}/audio/transcriptions",
        config.endpoint.trim_end_matches('/')
    );

    let part = reqwest::blocking::multipart::Part::bytes(wav_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| format!("Failed to create multipart part: {}", e))?;

    let form = reqwest::blocking::multipart::Form::new()
        .part("file", part)
        .text("model", config.model.clone())
        .text("response_format", "verbose_json")
        .text("language", config.language.clone());

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let mut request = client.post(&url).multipart(form);
    if !config.api_key.is_empty() {
        request = request.bearer_auth(&config.api_key);
    }

    let response = request
        .send()
        .map_err(|e| format!("Cloud ASR request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .unwrap_or_else(|_| "unable to read response body".to_string());
        return Err(format!("Cloud ASR API error ({}): {}", status, body));
    }

    let body = response
        .text()
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    let whisper_resp: WhisperResponse =
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {}", e))?;

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

    let segment_start_secs = segment.start_time.as_secs_f64();

    if let Some(segments) = whisper_resp.segments {
        let transcripts: Vec<TranscriptSegment> = segments
            .into_iter()
            .filter(|s| !s.text.trim().is_empty())
            .map(|s| {
                let confidence = s.no_speech_prob.map(|p| (1.0 - p) as f32).unwrap_or(0.9);
                TranscriptSegment {
                    id: Uuid::new_v4().to_string(),
                    source_id: segment.source_id.clone(),
                    speaker_id: None,
                    speaker_label: None,
                    text: s.text.trim().to_string(),
                    start_time: segment_start_secs + s.start,
                    end_time: segment_start_secs + s.end,
                    confidence,
                }
            })
            .collect();
        Ok(transcripts)
    } else {
        let text = whisper_resp.text.trim().to_string();
        if text.is_empty() {
            return Ok(vec![]);
        }
        Ok(vec![TranscriptSegment {
            id: Uuid::new_v4().to_string(),
            source_id: segment.source_id.clone(),
            speaker_id: None,
            speaker_label: None,
            text,
            start_time: segment_start_secs,
            end_time: segment.end_time.as_secs_f64(),
            confidence: 0.9,
        }])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn le_u32(b: &[u8]) -> u32 {
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }
    fn le_u16(b: &[u8]) -> u16 {
        u16::from_le_bytes([b[0], b[1]])
    }
    fn le_i16(b: &[u8]) -> i16 {
        i16::from_le_bytes([b[0], b[1]])
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
